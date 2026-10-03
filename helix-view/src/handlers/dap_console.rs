//! The debug console: one scratch document that collects everything a debug
//! session has to say -- evaluation results, program output, where execution
//! stopped -- and whose last line takes the next command, the way a terminal
//! pdb session does.

use crate::{
    align_view, editor::Action, tree::Direction, Align, Document, DocumentId, Editor, ViewId,
};
use helix_core::{Rope, RopeSlice, Selection, Transaction};
use std::path::Path;

/// Starts the input line, and every echoed command in the transcript.
pub const PROMPT: &str = "(hx) ";

#[derive(Debug, Default)]
pub struct DapConsole {
    /// The console document, once created. It outlives being hidden, so the
    /// transcript is still there when the console is shown again.
    pub doc: Option<DocumentId>,
    /// Where `Up`/`Down` are in the evaluation history; `None` while typing a new
    /// line.
    pub history_pos: Option<usize>,
    /// The last line run in the console, which `<ret>` on an empty input line runs
    /// again. Kept apart from the evaluation history, which outlives the session
    /// and is shared with the eval prompt.
    pub last_command: Option<String>,
    /// Program output that has not reached the end of a line yet, per category.
    /// Adapters forward writes as they happen, so `print("a")` may arrive as "a"
    /// and "\n" separately.
    pending_output: Vec<(String, String)>,
    /// Text on its way into the transcript: program output, gathered until the
    /// next frame so that a chatty program costs one edit per frame rather than
    /// one per line, and anything that arrives while the completion menu is open.
    /// Picking from the menu rewinds the document to where it opened, which would
    /// take any output added since with it.
    unwritten: String,
}

/// Where the input line -- always the last line -- starts.
pub fn input_line_start(text: RopeSlice) -> usize {
    text.line_to_char(text.len_lines().saturating_sub(1))
}

/// Where typing on the input line starts: just past the prompt, or at the start of
/// the line if the prompt was edited away.
pub fn input_start(text: RopeSlice) -> usize {
    let start = input_line_start(text);
    let prompt_len = PROMPT.chars().count();
    let has_prompt = text
        .slice(start..)
        .chars()
        .take(prompt_len)
        .eq(PROMPT.chars());
    start + if has_prompt { prompt_len } else { 0 }
}

/// What has been typed on the input line, without the prompt.
pub fn input_line(text: RopeSlice) -> String {
    let line: String = text.slice(input_line_start(text)..).into();
    line.strip_prefix(PROMPT.trim_end())
        .unwrap_or(&line)
        .trim()
        .to_string()
}

/// Program output for the transcript: stdout reads as it would in a terminal,
/// stderr is marked line by line.
fn format_output(category: &str, output: &str) -> String {
    match category {
        "stderr" => output
            .lines()
            .flat_map(|line| ["stderr: ", line, "\n"])
            .collect(),
        _ => output.to_string(),
    }
}

impl Editor {
    /// The console document, while it exists.
    pub fn dap_console_doc(&self) -> Option<DocumentId> {
        self.dap_console
            .doc
            .filter(|id| self.documents.contains_key(id))
    }

    /// Whether the focused view shows the console.
    pub fn is_dap_console_focused(&self) -> bool {
        let focused = self.tree.try_get(self.tree.focus).map(|view| view.doc);
        focused.is_some() && focused == self.dap_console_doc()
    }

    fn dap_console_views(&self, doc_id: DocumentId) -> Vec<ViewId> {
        self.tree
            .views()
            .filter(|(view, _)| view.doc == doc_id)
            .map(|(view, _)| view.id)
            .collect()
    }

    /// The console document, created with nothing but the prompt if there is none
    /// yet. It takes the language of the focused document, so the values it shows
    /// are highlighted like the source they came from.
    pub fn dap_console_ensure(&mut self) -> DocumentId {
        if let Some(doc_id) = self.dap_console_doc() {
            return doc_id;
        }

        let mut doc = Document::from(
            Rope::from(PROMPT),
            None,
            self.config.clone(),
            self.syn_loader.clone(),
        );
        doc.is_transcript = true;
        let language = self
            .tree
            .try_get(self.tree.focus)
            .and_then(|view| self.documents.get(&view.doc))
            .and_then(|doc| doc.language_name().map(str::to_owned));
        if let Some(language) = language {
            let _ = doc.set_language_by_language_id(&language, &self.syn_loader.load());
        }

        let doc_id = self.new_document(doc);
        self.dap_console.doc = Some(doc_id);
        doc_id
    }

    /// Puts the console on screen in a split under the focused view, unless it is
    /// already showing, and focuses it when asked to. Returns the console's view.
    pub fn dap_console_show(&mut self, focus: bool) -> ViewId {
        let doc_id = self.dap_console_ensure();
        let previous = self.tree.focus;

        let view_id = match self.dap_console_views(doc_id).first() {
            Some(&view_id) => view_id,
            None => {
                self.switch(doc_id, Action::HorizontalSplit);
                self.tree.focus
            }
        };

        if focus {
            self.focus(view_id);
        } else if self.tree.contains(previous) {
            self.focus(previous);
        }
        view_id
    }

    /// Takes the focused console off screen, keeping its document, and returns
    /// focus to the view that shows the current frame.
    pub fn dap_console_hide(&mut self) {
        if !self.is_dap_console_focused() {
            return;
        }
        let console_view = self.tree.focus;
        let target = self.dap_frame_view();
        self.enter_normal_mode();
        // The last view cannot go, so a lone console stays on screen.
        if target.is_some() {
            self.close(console_view);
        }
        if let Some(target) = target {
            self.focus(target);
        }
    }

    /// The view to show the current frame's source in while the console has focus,
    /// see [`Self::dap_source_view`].
    pub fn dap_frame_view(&self) -> Option<ViewId> {
        let path = self
            .current_stack_frame()
            .and_then(|frame| frame.source.as_ref()?.path.clone());
        self.dap_source_view(path.as_deref())
    }

    /// The view to show `path` in without touching the focused view or the
    /// console: one already showing it, else the split above the focused view, else
    /// any other.
    pub fn dap_source_view(&self, path: Option<&Path>) -> Option<ViewId> {
        let console = self.dap_console_doc();
        let is_source = |view_id: ViewId| {
            view_id != self.tree.focus
                && self
                    .tree
                    .try_get(view_id)
                    .is_some_and(|view| Some(view.doc) != console)
        };
        let path = path.map(helix_stdx::path::canonicalize);

        let showing_path = self.tree.views().find(|(view, _)| {
            is_source(view.id)
                && self
                    .documents
                    .get(&view.doc)
                    .and_then(|doc| doc.path())
                    .is_some_and(|doc_path| Some(doc_path) == path.as_ref())
        });
        if let Some((view, _)) = showing_path {
            return Some(view.id);
        }

        let above = self
            .tree
            .find_split_in_direction(self.tree.focus, Direction::Up)
            .filter(|&view_id| is_source(view_id));
        above.or_else(|| {
            self.tree
                .views()
                .map(|(view, _)| view.id)
                .find(|&view_id| is_source(view_id))
        })
    }

    /// Whether writing to the console now would be undone: the completion menu is
    /// open on it, and inserting an item rewinds the document to where it opened.
    fn dap_console_on_hold(&self) -> bool {
        self.is_dap_console_focused() && !self.handlers.completions.active_completions.is_empty()
    }

    /// Adds `text` to the console transcript, just above the input line, so output
    /// that arrives while a command is being typed never lands in the middle of it.
    /// Does nothing when there is no console. Never moves focus.
    ///
    /// Returns where the entry starts, unless it has to wait for the completion
    /// menu to close; then it is written when the menu is gone, see
    /// [`Self::dap_console_write_unwritten`].
    pub fn dap_console_print(&mut self, text: &str) -> Option<usize> {
        self.dap_console_doc()?;
        if text.is_empty() {
            return None;
        }
        let mut entry = text.to_string();
        if !entry.ends_with('\n') {
            entry.push('\n');
        }

        if self.dap_console_on_hold() {
            self.dap_console.unwritten.push_str(&entry);
            return None;
        }
        // Whatever was already on its way goes first, to keep the order.
        let unwritten = std::mem::take(&mut self.dap_console.unwritten);
        let offset = unwritten.chars().count();
        self.write_console(unwritten + entry.as_str())
            .map(|start| start + offset)
    }

    /// Writes out the text gathered by [`Self::dap_console_output`], or held while
    /// the completion menu was open. Called before every frame.
    pub fn dap_console_write_unwritten(&mut self) {
        if self.dap_console.unwritten.is_empty() || self.dap_console_on_hold() {
            return;
        }
        let unwritten = std::mem::take(&mut self.dap_console.unwritten);
        self.write_console(unwritten);
    }

    /// Inserts `entry` above the input line and returns where it starts.
    ///
    /// The focused console follows its cursor, which stays on the input line. A
    /// console on screen but not focused is scrolled to the new entry instead: to
    /// its start when it is too long to show whole, so it reads from the top.
    fn write_console(&mut self, entry: String) -> Option<usize> {
        let doc_id = self.dap_console_doc()?;

        let scrolloff = self.config().scrolloff;
        let focus = self.tree.focus;
        let views = self.dap_console_views(doc_id);
        // Applying a change takes a view the document keeps a selection for, and a
        // hidden console may have none.
        let apply_view = views.first().copied().unwrap_or(focus);
        if !self.tree.contains(apply_view) {
            return None;
        }

        let doc = self.documents.get_mut(&doc_id).unwrap();
        doc.ensure_view_init(apply_view);
        let at = input_line_start(doc.text().slice(..));
        let entry_line = doc.text().char_to_line(at);
        let transaction =
            Transaction::change(doc.text(), [(at, at, Some(entry.into()))].into_iter());
        doc.apply(&transaction, apply_view);
        doc.append_changes_to_history(self.tree.get_mut(apply_view));

        for view_id in views {
            let view = self.tree.get(view_id);
            if view_id == focus {
                view.ensure_cursor_in_view(doc, scrolloff);
                continue;
            }

            let text = doc.text().slice(..);
            let height = view.inner_area(doc).height as usize;
            let entry_lines = text.len_lines().saturating_sub(entry_line);
            if entry_lines > height {
                doc.set_selection(view_id, Selection::point(text.line_to_char(entry_line)));
                align_view(doc, view, Align::Top);
            } else {
                doc.set_selection(view_id, Selection::point(text.len_chars()));
                view.ensure_cursor_in_view(doc, scrolloff);
            }
        }
        Some(at)
    }

    /// Scrolls the console's views to the entry starting at `at`, as
    /// [`Self::dap_console_print`] returned it.
    pub fn dap_console_reveal(&mut self, at: usize) {
        let Some(doc_id) = self.dap_console_doc() else {
            return;
        };
        let views = self.dap_console_views(doc_id);
        let doc = self.documents.get_mut(&doc_id).unwrap();
        let at = at.min(doc.text().len_chars());
        for view_id in views {
            doc.set_selection(view_id, Selection::point(at));
            align_view(doc, self.tree.get(view_id), Align::Top);
        }
    }

    /// Program output from the adapter. Whole lines are gathered and written before
    /// the next frame; the rest waits for its line to end, or for
    /// [`Self::dap_console_flush_output`].
    pub fn dap_console_output(&mut self, category: &str, output: &str) {
        let pending = &mut self.dap_console.pending_output;
        let index = match pending.iter().position(|(c, _)| c == category) {
            Some(index) => index,
            None => {
                pending.push((category.to_string(), String::new()));
                pending.len() - 1
            }
        };
        let buffered = &mut pending[index].1;
        buffered.push_str(output);

        let Some(end) = buffered.rfind('\n') else {
            return;
        };
        let complete: String = buffered.drain(..=end).collect();
        self.dap_console
            .unwritten
            .push_str(&format_output(category, &complete));
        helix_event::request_redraw();
    }

    /// Writes out all program output now, including lines that never ended --
    /// when execution stops or the program is gone, nothing more is coming to
    /// finish them.
    pub fn dap_console_flush_output(&mut self) {
        for (category, output) in std::mem::take(&mut self.dap_console.pending_output) {
            if !output.is_empty() {
                let unwritten = &mut self.dap_console.unwritten;
                unwritten.push_str(&format_output(&category, &output));
                if !unwritten.ends_with('\n') {
                    unwritten.push('\n');
                }
            }
        }
        if self.dap_console_doc().is_none() {
            // Nowhere to write it any more.
            self.dap_console.unwritten.clear();
            return;
        }
        self.dap_console_write_unwritten();
    }

    /// Where the current frame is, pdb style: `> path(line)function()` and the line
    /// about to run.
    pub fn dap_console_print_location(&mut self) {
        let Some(frame) = self.current_stack_frame() else {
            return;
        };
        let path = frame.source.as_ref().and_then(|source| source.path.clone());
        let mut location = format!(
            "> {}({}){}()\n",
            path.as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<unknown>".to_string()),
            frame.line,
            frame.name
        );

        let source_line = path
            .and_then(|path| self.document_by_path(helix_stdx::path::canonicalize(path)))
            .and_then(|doc| {
                let text = doc.text();
                let line = frame.line.checked_sub(1)?;
                (line < text.len_lines()).then(|| text.line(line).to_string())
            });
        if let Some(source_line) = source_line {
            location.push_str(&format!("-> {}\n", source_line.trim()));
        }

        self.dap_console_print(&location);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_line_is_the_last_line_without_its_prompt() {
        let text = Rope::from("(hx) x\n5\n(hx)  len(xs) ");
        assert_eq!(input_line(text.slice(..)), "len(xs)");
        assert_eq!(input_line_start(text.slice(..)), 9);
        assert_eq!(input_start(text.slice(..)), 14);

        // A prompt that lost its space, or was deleted, still yields the input.
        assert_eq!(input_line(Rope::from("(hx)n").slice(..)), "n");
        assert_eq!(input_line(Rope::from("5\nn").slice(..)), "n");
        assert_eq!(input_start(Rope::from("5\nn").slice(..)), 2);

        assert_eq!(input_line(Rope::from(PROMPT).slice(..)), "");
    }

    #[test]
    fn stderr_is_marked_per_line() {
        assert_eq!(format_output("stdout", "a\nb\n"), "a\nb\n");
        assert_eq!(
            format_output("stderr", "boom\nagain\n"),
            "stderr: boom\nstderr: again\n"
        );
    }
}
