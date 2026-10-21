use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    iter,
};

use anyhow::Result;
use arc_swap::access::DynAccess;
use helix_core::NATIVE_LINE_ENDING;

use crate::{
    clipboard::{ClipboardError, ClipboardProvider, ClipboardType},
    DocumentId, Editor,
};

/// A key-value store for saving sets of values.
///
/// Each register corresponds to a `char`. Most chars can be used to store any set of
/// values but a few chars are "special registers". Special registers have unique
/// behaviors when read or written to:
///
/// * Black hole (`_`): all values read and written are discarded
/// * Selection indices (`#`): index number of each selection starting at 1
/// * Selection contents (`.`)
/// * Document path (`%`): filename of the current buffer
/// * System clipboard (`*`)
/// * Primary clipboard (`+`)
pub struct Registers {
    /// The mapping of register to values.
    /// Values are stored in reverse order when inserted with `Registers::write`.
    /// The order is reversed again in `Registers::read`. This allows us to
    /// efficiently prepend new values in `Registers::push`.
    inner: HashMap<char, Vec<String>>,
    clipboard_provider: Box<dyn DynAccess<ClipboardProvider>>,
    pub last_search_register: char,
    /// The encrypted document focused while registers are written, kept up to
    /// date by `Editor::sync_register_source`.
    pub source: Option<DocumentId>,
    /// The encrypted documents each register holds text from, cleared by
    /// [`Registers::wipe_from`] when one of them closes.
    taint: HashMap<char, HashSet<DocumentId>>,
}

impl Registers {
    pub fn new(clipboard_provider: Box<dyn DynAccess<ClipboardProvider>>) -> Self {
        Self {
            inner: Default::default(),
            clipboard_provider,
            last_search_register: '/',
            source: None,
            taint: HashMap::new(),
        }
    }

    /// Records that `name` now holds text written while `source` was focused.
    /// A write (`replace`) drops what the register held before; a push adds to it.
    fn mark(&mut self, name: char, replace: bool) {
        // The command history: closing a file shouldn't erase it.
        if name == ':' {
            return;
        }
        match self.source {
            Some(doc_id) if replace => {
                self.taint.insert(name, HashSet::from([doc_id]));
            }
            Some(doc_id) => {
                self.taint.entry(name).or_default().insert(doc_id);
            }
            None if replace => {
                self.taint.remove(&name);
            }
            None => {}
        }
    }

    /// Clears every register holding text from the encrypted document
    /// `doc_id`, which is closing. The system clipboard is cleared only if it
    /// still holds that text, so something copied elsewhere since survives.
    pub fn wipe_from(&mut self, doc_id: DocumentId) {
        let names: Vec<char> = self
            .taint
            .iter()
            .filter(|(_, docs)| docs.contains(&doc_id))
            .map(|(&name, _)| name)
            .collect();
        for name in names {
            self.taint.remove(&name);
            let clipboard_type = match name {
                '+' => ClipboardType::Clipboard,
                '*' => ClipboardType::Selection,
                _ => {
                    self.inner.remove(&name);
                    continue;
                }
            };
            let Some(saved) = self.inner.remove(&name) else {
                continue;
            };
            let still_ours = match self.clipboard_provider.load().get_contents(&clipboard_type) {
                Ok(contents) => contents_are_saved(&saved, &contents),
                // Can't check, so clear it to be safe.
                Err(_) => true,
            };
            if still_ours {
                self.clear_clipboard(clipboard_type);
            }
        }
    }

    pub fn read<'a>(&'a self, name: char, editor: &'a Editor) -> Option<RegisterValues<'a>> {
        match name {
            '_' => Some(RegisterValues::new(iter::empty())),
            '#' => {
                let (view, doc) = current_ref!(editor);
                let selections = doc.selection(view.id).len();
                // ExactSizeIterator is implemented for Range<usize> but
                // not RangeInclusive<usize>.
                Some(RegisterValues::new(
                    (0..selections).map(|i| (i + 1).to_string().into()),
                ))
            }
            '.' => {
                let (view, doc) = current_ref!(editor);
                let text = doc.text().slice(..);
                Some(RegisterValues::new(doc.selection(view.id).fragments(text)))
            }
            '%' => {
                let path = doc!(editor).display_name();
                Some(RegisterValues::new(iter::once(path)))
            }
            '*' | '+' => Some(read_from_clipboard(
                &self.clipboard_provider.load(),
                self.inner.get(&name),
                match name {
                    '+' => ClipboardType::Clipboard,
                    '*' => ClipboardType::Selection,
                    _ => unreachable!(),
                },
            )),
            _ => self
                .inner
                .get(&name)
                .map(|values| RegisterValues::new(values.iter().map(Cow::from).rev())),
        }
    }

    pub fn write(&mut self, name: char, mut values: Vec<String>) -> Result<()> {
        match name {
            '_' => Ok(()),
            '#' | '.' | '%' => Err(anyhow::anyhow!("Register {name} does not support writing")),
            '*' | '+' => {
                self.clipboard_provider.load().set_contents(
                    &values.join(NATIVE_LINE_ENDING.as_str()),
                    match name {
                        '+' => ClipboardType::Clipboard,
                        '*' => ClipboardType::Selection,
                        _ => unreachable!(),
                    },
                )?;
                values.reverse();
                self.inner.insert(name, values);
                self.mark(name, true);
                Ok(())
            }
            _ => {
                values.reverse();
                self.inner.insert(name, values);
                self.mark(name, true);
                Ok(())
            }
        }
    }

    pub fn push(&mut self, name: char, mut value: String) -> Result<()> {
        match name {
            '_' => Ok(()),
            '#' | '.' | '%' => Err(anyhow::anyhow!("Register {name} does not support pushing")),
            '*' | '+' => {
                let clipboard_type = match name {
                    '+' => ClipboardType::Clipboard,
                    '*' => ClipboardType::Selection,
                    _ => unreachable!(),
                };
                let contents = self
                    .clipboard_provider
                    .load()
                    .get_contents(&clipboard_type)?;
                let saved_values = self.inner.entry(name).or_default();

                if !contents_are_saved(saved_values, &contents) {
                    anyhow::bail!("Failed to push to register {name}: clipboard does not match register contents");
                }

                saved_values.push(value.clone());
                if !contents.is_empty() {
                    value.push_str(NATIVE_LINE_ENDING.as_str());
                }
                value.push_str(&contents);
                self.clipboard_provider
                    .load()
                    .set_contents(&value, clipboard_type)?;
                self.mark(name, false);

                Ok(())
            }
            _ => {
                self.inner.entry(name).or_default().push(value);
                self.mark(name, false);
                Ok(())
            }
        }
    }

    pub fn first<'a>(&'a self, name: char, editor: &'a Editor) -> Option<Cow<'a, str>> {
        self.read(name, editor).and_then(|mut values| values.next())
    }

    pub fn last<'a>(&'a self, name: char, editor: &'a Editor) -> Option<Cow<'a, str>> {
        self.read(name, editor)
            .and_then(|mut values| values.next_back())
    }

    pub fn iter_preview(&self) -> impl Iterator<Item = (char, &str)> {
        self.inner
            .iter()
            .filter(|(name, _)| !matches!(name, '*' | '+'))
            .map(|(name, values)| {
                let preview = values
                    .last()
                    .and_then(|s| s.lines().next())
                    .unwrap_or("<empty>");

                (*name, preview)
            })
            .chain(
                [
                    ('_', "<empty>"),
                    ('#', "<selection indices>"),
                    ('.', "<selection contents>"),
                    ('%', "<document path>"),
                    ('+', "<system clipboard>"),
                    ('*', "<primary clipboard>"),
                ]
                .iter()
                .copied(),
            )
    }

    pub fn clear(&mut self) {
        self.clear_clipboard(ClipboardType::Clipboard);
        self.clear_clipboard(ClipboardType::Selection);
        self.inner.clear();
        self.taint.clear();
    }

    pub fn remove(&mut self, name: char) -> bool {
        self.taint.remove(&name);
        match name {
            '*' | '+' => {
                self.clear_clipboard(match name {
                    '+' => ClipboardType::Clipboard,
                    '*' => ClipboardType::Selection,
                    _ => unreachable!(),
                });
                self.inner.remove(&name);

                true
            }
            '_' | '#' | '.' | '%' => false,
            _ => self.inner.remove(&name).is_some(),
        }
    }

    fn clear_clipboard(&mut self, clipboard_type: ClipboardType) {
        if let Err(err) = self
            .clipboard_provider
            .load()
            .set_contents("", clipboard_type)
        {
            log::error!(
                "Failed to clear {} clipboard: {err}",
                match clipboard_type {
                    ClipboardType::Clipboard => "system",
                    ClipboardType::Selection => "primary",
                }
            )
        }
    }

    pub fn clipboard_provider_name(&self) -> String {
        self.clipboard_provider.load().name().into_owned()
    }
}

fn read_from_clipboard<'a>(
    provider: &ClipboardProvider,
    saved_values: Option<&'a Vec<String>>,
    clipboard_type: ClipboardType,
) -> RegisterValues<'a> {
    match provider.get_contents(&clipboard_type) {
        Ok(contents) => {
            // If we're pasting the same values that we just yanked, re-use
            // the saved values. This allows pasting multiple selections
            // even when yanked to a clipboard.
            let Some(values) = saved_values else {
                return RegisterValues::new(iter::once(contents.into()));
            };

            if contents_are_saved(values, &contents) {
                RegisterValues::new(values.iter().map(Cow::from).rev())
            } else {
                RegisterValues::new(iter::once(contents.into()))
            }
        }
        Err(ClipboardError::ReadingNotSupported) => match saved_values {
            Some(values) => RegisterValues::new(values.iter().map(Cow::from).rev()),
            None => RegisterValues::new(iter::empty()),
        },
        Err(err) => {
            log::error!(
                "Failed to read {} clipboard: {err}",
                match clipboard_type {
                    ClipboardType::Clipboard => "system",
                    ClipboardType::Selection => "primary",
                }
            );

            RegisterValues::new(iter::empty())
        }
    }
}

fn contents_are_saved(saved_values: &[String], mut contents: &str) -> bool {
    let line_ending = NATIVE_LINE_ENDING.as_str();
    let mut values = saved_values.iter().rev();

    match values.next() {
        Some(first) if contents.starts_with(first) => {
            contents = &contents[first.len()..];
        }
        None if contents.is_empty() => return true,
        _ => return false,
    }

    for value in values {
        if contents.starts_with(line_ending) && contents[line_ending.len()..].starts_with(value) {
            contents = &contents[line_ending.len() + value.len()..];
        } else {
            return false;
        }
    }

    true
}

// This is a wrapper of an iterator that is both double ended and exact size,
// and can return either owned or borrowed values. Regular registers can
// return borrowed values while some special registers need to return owned
// values.
pub struct RegisterValues<'a> {
    iter: Box<dyn DoubleEndedExactSizeIterator<Item = Cow<'a, str>> + 'a>,
}

impl<'a> RegisterValues<'a> {
    fn new(
        iter: impl DoubleEndedIterator<Item = Cow<'a, str>>
            + ExactSizeIterator<Item = Cow<'a, str>>
            + 'a,
    ) -> Self {
        Self {
            iter: Box::new(iter),
        }
    }
}

impl<'a> Iterator for RegisterValues<'a> {
    type Item = Cow<'a, str>;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl DoubleEndedIterator for RegisterValues<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.iter.next_back()
    }
}

impl ExactSizeIterator for RegisterValues<'_> {
    fn len(&self) -> usize {
        self.iter.len()
    }
}

// Each RegisterValues iterator is both double ended and exact size. We can't
// type RegisterValues as `Box<dyn DoubleEndedIterator + ExactSizeIterator>`
// because only one non-auto trait is allowed in trait objects. So we need to
// create a new trait that covers both. `RegisterValues` wraps that type so that
// trait only needs to live in this module and not be imported for all register
// callsites.
trait DoubleEndedExactSizeIterator: DoubleEndedIterator + ExactSizeIterator {}

impl<I: DoubleEndedIterator + ExactSizeIterator> DoubleEndedExactSizeIterator for I {}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_swap::access::Constant;
    use std::num::NonZeroUsize;

    fn doc(id: usize) -> DocumentId {
        DocumentId(NonZeroUsize::new(id).unwrap())
    }

    fn registers() -> Registers {
        Registers::new(Box::new(Constant(ClipboardProvider::None)))
    }

    fn write(registers: &mut Registers, source: Option<DocumentId>, name: char, value: &str) {
        registers.source = source;
        registers.write(name, vec![value.to_string()]).unwrap();
    }

    #[test]
    fn wipe_from_clears_what_the_document_wrote() {
        let (secret, other) = (doc(1), doc(2));
        let mut registers = registers();
        write(&mut registers, Some(secret), '"', "hunter2");
        write(&mut registers, Some(secret), '+', "hunter2");
        write(&mut registers, Some(other), 'a', "other secret");
        write(&mut registers, None, 'b', "plain");

        registers.wipe_from(secret);
        assert!(!registers.inner.contains_key(&'"'));
        assert!(!registers.inner.contains_key(&'+'));
        assert!(registers.inner.contains_key(&'a'));
        assert!(registers.inner.contains_key(&'b'));

        registers.wipe_from(other);
        assert!(!registers.inner.contains_key(&'a'));
        assert!(registers.inner.contains_key(&'b'));
    }

    #[test]
    fn a_plain_write_replaces_the_mark_and_a_push_adds_to_it() {
        let secret = doc(1);
        let mut registers = registers();
        write(&mut registers, Some(secret), 'a', "hunter2");
        write(&mut registers, None, 'a', "plain");
        write(&mut registers, None, 'b', "plain");
        registers.source = Some(secret);
        registers.push('b', "hunter2".to_string()).unwrap();

        registers.wipe_from(secret);
        assert!(registers.inner.contains_key(&'a'));
        assert!(!registers.inner.contains_key(&'b'));
    }

    #[test]
    fn command_history_is_kept() {
        let secret = doc(1);
        let mut registers = registers();
        registers.source = Some(secret);
        registers.push(':', "w".to_string()).unwrap();
        registers.wipe_from(secret);
        assert!(registers.inner.contains_key(&':'));
    }
}
