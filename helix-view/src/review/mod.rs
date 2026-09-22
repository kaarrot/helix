//! Inline review threads: comments a user leaves on a line, and the agent's
//! replies to them.
//!
//! Content lives here, on the editor. *Positions* live on the `Document`, as
//! [`ReviewAnchor`]s, because `DocumentDidChange` hands out a `&mut Document`
//! and no `&mut Editor` — the same constraint that put the colour-swatch state
//! on `Document` (see the note at `document.rs`). The two are joined by
//! [`ThreadId`].

use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    rc::Rc,
    time::SystemTime,
};

pub mod agent;
pub mod session;

use helix_core::{ChangeSet, Rope};
use serde::{Deserialize, Serialize};

use crate::annotations::rows::{Attention, CommentLine, CommentSpan, RowMark, VirtualRow};
use agent::AgentEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ThreadId(pub u32);

impl std::fmt::Display for ThreadId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    User,
    Agent,
}

/// Which pane of a split diff a thread is anchored to. A thread on the base
/// side comments on the old text, which is a different statement from the same
/// line number on the working side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiffSide {
    Base,
    Working,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub text: String,
    pub created_at: SystemTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thread {
    pub id: ThreadId,
    /// Always the canonicalized **working-tree** path, for both sides:
    /// `Document::from_git_revision` clears `path` on base documents, so a base
    /// pane cannot identify itself.
    pub file: PathBuf,
    pub side: DiffSide,
    /// 0-based line, as last known. Authoritative only until the document is
    /// open; while it is, the document's [`ReviewAnchor`] leads.
    pub line: u32,
    pub messages: Vec<Message>,
    /// Text typed but not yet sent. `Some` means this thread is a pending draft.
    pub draft: Option<String>,
    pub collapsed: bool,
    /// Which entry is currently shown. A thread renders one entry at a time, so
    /// its height is bounded by a single message rather than growing with the
    /// whole conversation.
    ///
    /// Not persisted: where you were looking is not worth restoring, and a
    /// reloaded thread should open on its newest entry.
    #[serde(skip)]
    pub view: usize,
    /// A reply is in flight: the request went out and nothing has come back
    /// yet, or is still arriving. Rendered as a spinner so a slow answer never
    /// looks like nothing happened.
    ///
    /// Not persisted: a turn in flight when the editor died will never land, so
    /// restoring this would leave a thread spinning forever.
    #[serde(skip)]
    pub awaiting: bool,
    /// First visible row of the current entry, when it is taller than the box
    /// is allowed to be. Not persisted: it is a reading position.
    ///
    /// A `Cell` because it follows [`Thread::cursor`], and only rendering knows
    /// how tall the box was allowed to be -- so rendering is what settles it.
    #[serde(skip)]
    pub scroll: Cell<usize>,
    /// Body rows last painted for [`Thread::view`], at the body width they were
    /// wrapped to. Copy and scrolling measure these so they agree with the
    /// screen. An agent reply is markdown, which does not wrap like its source;
    /// measuring the source again would copy text the box is not showing.
    #[serde(skip)]
    pub drawn: RefCell<Option<DrawnBody>>,
    /// Row of the current entry the in-box cursor is on, counted in wrapped
    /// body lines. Drawn only while the box is focused, and the thing `y`
    /// copies from. Not persisted: it is a reading position.
    #[serde(skip)]
    pub cursor: usize,
    /// Where a selection inside the box started, if one is being made. The
    /// selection runs between this row and the cursor, either way round.
    #[serde(skip)]
    pub select: Option<usize>,
    /// The conversation was rewound since the last send, so the agent is still
    /// holding replies this thread no longer shows. Cleared once it has been
    /// told.
    #[serde(default)]
    pub rewound: bool,
    /// The anchored line was deleted. The thread is kept and shown dimmed at the
    /// collapse point: silently discarding a conversation is worse than showing
    /// one that has lost its footing.
    pub orphaned: bool,
    /// The agent conversation for this thread alone.
    ///
    /// Created on the first send and persisted, so a later reply resumes it
    /// with `--resume`. Distinct from the review-session UUID, which only names
    /// the file these threads are saved in. `None` until the thread is sent.
    /// Absent in saves from before each comment had its own conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session: Option<String>,
}

/// Plain text of the body rows last drawn for one entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrawnBody {
    /// [`Thread::view`] these rows belong to.
    pub view: usize,
    /// Width after the marker column, the same value [`body_width`] returns.
    pub width: usize,
    pub lines: Vec<String>,
}

/// One addressable entry of a thread: a sent message, or the unsent draft.
///
/// The draft is an entry like any other so that the counter means "entries in
/// this conversation" with no special case to explain.
pub struct Entry<'a> {
    pub label: &'static str,
    pub text: &'a str,
    pub pending: bool,
}

impl Thread {
    /// A thread nobody has sent yet, and which therefore has no reply coming.
    pub fn is_pending(&self) -> bool {
        self.draft.is_some()
    }

    pub fn entry_count(&self) -> usize {
        self.messages.len() + usize::from(self.draft.is_some())
    }

    pub fn entry(&self, index: usize) -> Option<Entry<'_>> {
        if let Some(message) = self.messages.get(index) {
            return Some(Entry {
                label: match message.role {
                    Role::User => "you",
                    Role::Agent => "agent",
                },
                text: &message.text,
                pending: false,
            });
        }
        let draft = self.draft.as_deref()?;
        (index == self.messages.len()).then_some(Entry {
            label: "draft",
            text: draft,
            pending: true,
        })
    }

    /// A reply has been asked for and nothing of it has arrived yet.
    ///
    /// Rendered as an extra entry so the counter and the role already show the
    /// answer coming, rather than leaving a spinner on the reader's own comment
    /// with no sign anything is happening. Once the first text arrives this
    /// stops being true and the real entry takes its place.
    pub fn awaiting_reply(&self) -> bool {
        self.awaiting && !matches!(self.messages.last(), Some(last) if last.role == Role::Agent)
    }

    /// Index of the entry actually shown, clamped in case the thread shrank.
    pub fn view_index(&self) -> usize {
        self.view.min(self.entry_count().saturating_sub(1))
    }

    /// Step the view. Saturates rather than wrapping: reaching the newest reply
    /// and silently looping back to the oldest would be disorienting.
    pub fn step_view(&mut self, forward: bool) -> bool {
        let last = self.entry_count().saturating_sub(1);
        let current = self.view_index();
        let next = if forward {
            current.saturating_add(1).min(last)
        } else {
            current.saturating_sub(1)
        };
        self.view = next;
        next != current
    }

    pub fn summary(&self) -> &str {
        self.draft
            .as_deref()
            .or_else(|| self.messages.first().map(|message| message.text.as_str()))
            .unwrap_or("")
    }

    /// Line this thread occupies in an open document.
    ///
    /// The document's [`ReviewAnchor`] tracks edits; `self.line` is only the
    /// last persisted snapshot.
    pub fn line_in(&self, anchors: &[ReviewAnchor], text: &Rope) -> usize {
        anchors
            .iter()
            .find(|anchor| anchor.thread == self.id)
            .map_or(self.line as usize, |anchor| anchor.line(text))
    }

    /// Back to the top of the entry, with nothing selected. Called whenever the
    /// entry on show changes: a reading position in one entry means nothing in
    /// the next.
    pub fn reset_reading(&mut self) {
        self.scroll.set(0);
        self.cursor = 0;
        self.select = None;
    }

    /// The rows of the entry on show, wrapped exactly as they are drawn.
    ///
    /// The in-box cursor counts in these, so anything that moves it has to
    /// measure with the same width the box was drawn at.
    pub fn body_rows(&self, width: usize) -> Vec<String> {
        let text_width = body_width(width);
        if let Some(drawn) = self.drawn.borrow().as_ref() {
            if drawn.view == self.view_index()
                && drawn.width == text_width
                && !drawn.lines.is_empty()
            {
                return drawn.lines.clone();
            }
        }
        let entry = match self.entry(self.view_index()) {
            Some(entry) => entry,
            None => return Vec::new(),
        };
        crate::annotations::rows::wrap_text(entry.text, text_width)
    }

    /// The selected rows as an inclusive range, in either direction of travel.
    pub fn selected_rows(&self, width: usize) -> Option<(usize, usize)> {
        let anchor = self.select?;
        let last = self.body_rows(width).len().saturating_sub(1);
        let cursor = self.cursor.min(last);
        let anchor = anchor.min(last);
        Some((anchor.min(cursor), anchor.max(cursor)))
    }

    /// What `y` puts on the clipboard: the selected rows, or the whole entry
    /// when nothing is selected.
    ///
    /// A selection covering everything yields the entry verbatim rather than
    /// its wrapped rows, so copying a whole reply keeps the paragraphs it was
    /// written with. A partial selection can only be the rows as drawn -- that
    /// is what was pointed at.
    pub fn copy_text(&self, width: usize) -> Option<String> {
        let entry = self.entry(self.view_index())?;
        let rows = self.body_rows(width);
        match self.selected_rows(width) {
            Some((start, end)) if start > 0 || end + 1 < rows.len() => {
                Some(rows.get(start..=end)?.join("\n"))
            }
            _ => Some(entry.text.to_string()),
        }
    }
}

/// Width a comment body wraps at, once the marker column is taken out. The one
/// place that decides it, so drawing and copying cannot disagree.
pub fn body_width(width: usize) -> usize {
    width.saturating_sub(MARKER.chars().count() + 1).max(8)
}

/// Where a thread currently sits in an open document, as a char range covering
/// the anchored line. A range rather than a point so that a deletion of the
/// whole line is detectable: both ends collapse together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReviewAnchor {
    pub thread: ThreadId,
    pub start: usize,
    pub end: usize,
    pub orphaned: bool,
}

impl ReviewAnchor {
    pub fn for_line(thread: ThreadId, text: &Rope, line: usize) -> Self {
        let line = line.min(text.len_lines().saturating_sub(1));
        let start = text.line_to_char(line);
        let end = start + text.line(line).len_chars();
        Self {
            thread,
            start,
            end,
            orphaned: false,
        }
    }

    pub fn line(&self, text: &Rope) -> usize {
        text.char_to_line(self.start.min(text.len_chars()))
    }
}

/// Map anchors through an edit.
///
/// `map_pos` cannot report that a position was deleted — it collapses to the
/// edit point — so deletion is inferred from the line's two ends meeting. An
/// orphaned anchor is kept at that point rather than dropped.
pub fn remap_anchors(anchors: &mut [ReviewAnchor], changes: &ChangeSet) {
    for anchor in anchors {
        let start = changes.map_pos(anchor.start, helix_core::Assoc::After);
        let end = changes.map_pos(anchor.end, helix_core::Assoc::Before);
        if end <= start {
            anchor.orphaned = true;
            anchor.start = start;
            anchor.end = start;
        } else {
            anchor.start = start;
            anchor.end = end;
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct StoreSnapshot {
    threads: Vec<Thread>,
    next_id: u32,
}

/// Where a comment is being typed, and how much room it needs.
///
/// While this is set the thread there renders as the input rather than as
/// itself, so composing a one-line reply does not leave a tall answer sitting
/// underneath the box being typed into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Composing {
    pub file: PathBuf,
    pub side: DiffSide,
    pub line: u32,
    pub rows: usize,
}

/// One painted box row, as the mouse would find it.
///
/// Recorded while drawing rather than worked out afterwards: only the renderer
/// knows where a row ended up once soft wrap, spacers and other boxes above it
/// have had their say, and a second calculation would be a second chance to
/// disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoxHit {
    pub view: crate::ViewId,
    /// Screen row, in terminal coordinates.
    pub row: u16,
    /// Horizontal extent, so a click in the pane next door is not taken for a
    /// click on this box.
    pub x: u16,
    pub width: u16,
    pub thread: ThreadId,
    /// Which wrapped line of the entry sits here, if this row shows one.
    pub body: Option<usize>,
}

/// A press that landed in a box and may yet become a drag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoxPress {
    pub thread: ThreadId,
    /// The row pressed on, which becomes the selection's anchor if the pointer
    /// moves. A press on its own selects nothing: clicking into a box is asking
    /// to point at it, and only dragging is asking for a range.
    pub body: usize,
    pub dragged: bool,
}

#[derive(Debug, Default)]
pub struct ReviewStore {
    /// Boxes are hidden. The conversations are untouched -- this only stops
    /// them being drawn, and stops the keys that act on them from doing so, so
    /// that a hidden thread cannot be replied to or deleted by accident.
    pub hidden: bool,
    /// Set while a comment box is open for typing. Not persisted.
    pub composing: Option<Composing>,
    /// The thread the cursor has stopped on, if any. Not persisted: it is where
    /// the cursor happens to be, not part of the conversation.
    pub focused: Option<ThreadId>,
    /// Where every box row was last painted, filled in by the renderer so the
    /// mouse can find out what it is over. Shared and interior-mutable because
    /// drawing has only `&Editor` to work with.
    pub hits: Rc<RefCell<Vec<BoxHit>>>,
    /// A left button held down inside a box.
    pub press: Option<BoxPress>,
    /// The conversations on disk could not be read. Saving is refused while
    /// this is set, so the empty store we started instead can never be written
    /// over a file we merely failed to parse.
    pub save_blocked: bool,
    /// What to tell the reader about a failed load. Taken once, by whoever is
    /// in a position to show it.
    pub load_error: Option<String>,
    /// Ordered by id, which is insertion order, so listings are stable.
    threads: BTreeMap<ThreadId, Thread>,
    by_file: HashMap<PathBuf, Vec<ThreadId>>,
    next_id: u32,
}

impl ReviewStore {
    /// The box row under a screen position, if any.
    pub fn hit_at(&self, row: u16, column: u16) -> Option<BoxHit> {
        self.hits
            .borrow()
            .iter()
            .find(|hit| {
                hit.row == row && column >= hit.x && column < hit.x.saturating_add(hit.width)
            })
            .copied()
    }

    /// The nearest body row of `thread` at a screen row, for a drag that has
    /// left the box: dragging past the bottom should keep extending to the
    /// bottom rather than stop responding.
    pub fn drag_row(&self, thread: ThreadId, row: u16) -> Option<usize> {
        let hits = self.hits.borrow();
        let mut rows = hits
            .iter()
            .filter(|hit| hit.thread == thread)
            .filter_map(|hit| hit.body.map(|body| (hit.row, body)));
        let first = rows.next()?;
        let nearest = rows.fold(first, |best, candidate| {
            let distance = |(hit_row, _): (u16, usize)| hit_row.abs_diff(row);
            if distance(candidate) < distance(best) {
                candidate
            } else {
                best
            }
        });
        Some(nearest.1)
    }

    pub fn is_empty(&self) -> bool {
        self.threads.is_empty()
    }

    pub fn len(&self) -> usize {
        self.threads.len()
    }

    /// The conversation about a particular line, if one has been started.
    pub fn thread_at(&self, file: &Path, side: DiffSide, line: u32) -> Option<ThreadId> {
        self.by_file
            .get(file)?
            .iter()
            .find(|id| {
                self.threads
                    .get(id)
                    .is_some_and(|thread| thread.side == side && thread.line == line)
            })
            .copied()
    }

    /// Fold conversations that describe the same line into one.
    ///
    /// A line has one conversation about it, not several: two boxes stacked on
    /// one line are two halves of the same discussion, and reading them as
    /// separate threads makes the reader reconstruct an order the store already
    /// knows. Entries are interleaved by when they were written.
    fn merge_by_anchor(&mut self) {
        let mut keep: HashMap<(PathBuf, DiffSide, u32), ThreadId> = HashMap::new();
        let mut absorb: Vec<(ThreadId, ThreadId)> = Vec::new();

        for thread in self.threads.values() {
            let anchor = (thread.file.clone(), thread.side, thread.line);
            match keep.get(&anchor) {
                // Earliest id wins, so the conversation keeps the identity it
                // started with.
                Some(first) => absorb.push((*first, thread.id)),
                None => {
                    keep.insert(anchor, thread.id);
                }
            }
        }

        for (into, from) in absorb {
            let Some(extra) = self.remove(from) else {
                continue;
            };
            let Some(target) = self.threads.get_mut(&into) else {
                continue;
            };
            target.messages.extend(extra.messages);
            target.messages.sort_by_key(|message| message.created_at);
            // An unsent draft is worth more than a sent one is worth keeping
            // twice: prefer whichever is still unsent.
            target.draft = target.draft.take().or(extra.draft);
            target.orphaned &= extra.orphaned;
            if target.agent_session.is_none() {
                target.agent_session = extra.agent_session;
            }
            target.view = target.entry_count().saturating_sub(1);
        }
    }

    /// Open a thread with an unsent draft on it.
    ///
    /// Reuses the conversation already on that line if there is one, so a line
    /// cannot accumulate rival threads.
    pub fn draft(&mut self, file: PathBuf, side: DiffSide, line: u32, text: String) -> ThreadId {
        if let Some(existing) = self.thread_at(&file, side, line) {
            self.set_draft(existing, text);
            return existing;
        }
        let id = ThreadId(self.next_id);
        self.next_id += 1;
        self.by_file.entry(file.clone()).or_default().push(id);
        self.threads.insert(
            id,
            Thread {
                id,
                file,
                side,
                line,
                messages: Vec::new(),
                draft: Some(text),
                collapsed: false,
                view: 0,
                scroll: Cell::new(0),
                drawn: RefCell::new(None),
                cursor: 0,
                select: None,
                awaiting: false,
                rewound: false,
                orphaned: false,
                agent_session: None,
            },
        );
        id
    }

    /// The agent conversation for this thread, created on the first send.
    ///
    /// Stable afterwards, including across a reload, so a follow-up resumes the
    /// same conversation instead of starting one that has forgotten it.
    pub fn ensure_agent_session(&mut self, id: ThreadId) -> Option<String> {
        let thread = self.threads.get_mut(&id)?;
        if thread.agent_session.is_none() {
            thread.agent_session = Some(session::random_uuid());
        }
        thread.agent_session.clone()
    }

    pub fn get(&self, id: ThreadId) -> Option<&Thread> {
        self.threads.get(&id)
    }

    pub fn get_mut(&mut self, id: ThreadId) -> Option<&mut Thread> {
        self.threads.get_mut(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Thread> {
        self.threads.values()
    }

    pub fn for_file<'a>(&'a self, file: &Path) -> impl Iterator<Item = &'a Thread> + 'a {
        self.by_file
            .get(file)
            .into_iter()
            .flatten()
            .filter_map(|id| self.threads.get(id))
    }

    /// Threads with unsent drafts, oldest first — the batch a send flushes.
    pub fn pending(&self) -> impl Iterator<Item = &Thread> {
        self.threads.values().filter(|thread| thread.is_pending())
    }

    pub fn pending_count(&self) -> usize {
        self.pending().count()
    }

    /// Promote a draft into the thread's first user message. Returns the text
    /// that was sent, or `None` if there was no draft to send.
    pub fn take_draft(&mut self, id: ThreadId) -> Option<String> {
        let thread = self.threads.get_mut(&id)?;
        let text = thread.draft.take()?;
        thread.messages.push(Message {
            role: Role::User,
            text: text.clone(),
            created_at: SystemTime::now(),
        });
        Some(text)
    }

    /// Start or replace the unsent draft on an existing thread, and show it.
    ///
    /// Always follows, unlike an incoming reply: you are composing this one, so
    /// looking at something else while typing would be absurd.
    pub fn set_draft(&mut self, id: ThreadId, text: String) {
        if let Some(thread) = self.threads.get_mut(&id) {
            thread.draft = Some(text);
            thread.view = thread.entry_count().saturating_sub(1);
        }
    }

    /// Drop everything after `index`, so the conversation continues from there.
    ///
    /// Returns how many entries were discarded. This is the one place the store
    /// throws text away, so the caller is expected to say so rather than let it
    /// happen quietly.
    pub fn rewind_to(&mut self, id: ThreadId, index: usize) -> usize {
        let Some(thread) = self.threads.get_mut(&id) else {
            return 0;
        };
        let before = thread.entry_count();
        if index + 1 >= before {
            return 0;
        }

        thread.draft = None;
        thread.messages.truncate(index + 1);
        thread.view = thread.entry_count().saturating_sub(1);
        // This thread's conversation still remembers what was dropped, so it has
        // to be told or it will answer as though it still stood.
        thread.rewound = true;
        before - thread.entry_count()
    }

    /// Remove one entry, leaving the rest of the conversation in place.
    ///
    /// Returns what remains, or `None` once nothing does and the thread has been
    /// removed with it. Like a rewind, the agent is still holding what was taken
    /// out, so it is flagged to be told.
    pub fn remove_entry(&mut self, id: ThreadId, index: usize) -> Option<usize> {
        let thread = self.threads.get_mut(&id)?;
        let messages = thread.messages.len();

        if index < messages {
            thread.messages.remove(index);
        } else if index == messages && thread.draft.is_some() {
            // The draft has never been sent, so nothing else knows about it and
            // there is nobody to tell.
            thread.draft = None;
            if thread.entry_count() == 0 {
                self.remove(id);
                return None;
            }
            thread.view = thread.entry_count().saturating_sub(1);
            thread.reset_reading();
            return Some(thread.entry_count());
        } else {
            return Some(thread.entry_count());
        }

        thread.rewound = true;
        thread.view = thread.view.min(thread.entry_count().saturating_sub(1));
        thread.reset_reading();

        if thread.entry_count() == 0 {
            self.remove(id);
            return None;
        }
        Some(self.threads.get(&id)?.entry_count())
    }

    pub fn push_message(&mut self, id: ThreadId, role: Role, text: String) {
        let Some(thread) = self.threads.get_mut(&id) else {
            return;
        };
        // Follow the newest entry only if the reader was already on it. Someone
        // part-way back through the history is reading; yanking them to a new
        // reply mid-sentence would lose their place. The counter's total grows
        // either way, which is how they learn something arrived.
        let was_at_newest = thread.view_index() + 1 >= thread.entry_count();
        thread.messages.push(Message {
            role,
            text,
            created_at: SystemTime::now(),
        });
        if was_at_newest {
            thread.view = thread.entry_count().saturating_sub(1);
        }
    }

    /// What is written to disk.
    ///
    /// An explicit shape rather than the store's own fields: `by_file` is an
    /// index that can be rebuilt, and persisting it would be one more thing
    /// able to disagree with the threads it points at.
    fn snapshot(&self) -> StoreSnapshot {
        StoreSnapshot {
            threads: self.threads.values().cloned().collect(),
            next_id: self.next_id,
        }
    }

    /// Persist the store for `uuid`, replacing whatever was there.
    ///
    /// Written to a temporary file and renamed, so an editor that dies
    /// mid-write leaves the previous conversation intact rather than a
    /// half-written file that will not parse.
    pub fn save_to(&self, dir: &Path, uuid: &str) -> std::io::Result<()> {
        // Starting empty because the file would not parse is not the same as
        // there being nothing to keep. Writing here would turn a file we could
        // not read into one there is nothing left to read.
        if self.save_blocked {
            return Err(std::io::Error::other(
                "refusing to overwrite review threads that could not be read",
            ));
        }
        std::fs::create_dir_all(dir)?;
        let raw = serde_json::to_string_pretty(&self.snapshot()).map_err(std::io::Error::other)?;
        let final_path = dir.join(format!("{uuid}.threads.json"));
        let temp_path = dir.join(format!("{uuid}.threads.json.tmp"));
        std::fs::write(&temp_path, raw)?;
        std::fs::rename(&temp_path, &final_path)
    }

    /// An empty store that knows it should not be written back.
    ///
    /// The unreadable file is moved aside first, which is what lets work carry
    /// on: the old conversation is kept for inspection and the session is free
    /// to save a new one. Only when it cannot be moved is saving refused
    /// outright, because then overwriting is the only other option.
    fn unreadable(path: &Path, why: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let kept = path.with_extension(format!("json.unreadable.{stamp}"));

        let (save_blocked, message) = match std::fs::rename(path, &kept) {
            Ok(()) => (
                false,
                format!(
                    "Review threads could not be read ({why}); kept as {} and starting a new file",
                    kept.display()
                ),
            ),
            Err(err) => (
                true,
                format!(
                    "Review threads could not be read ({why}) or moved aside ({err}); \
                     nothing will be saved until {} is dealt with",
                    path.display()
                ),
            ),
        };
        log::warn!("{message}");

        Self {
            save_blocked,
            load_error: Some(message),
            ..Self::default()
        }
    }

    /// Reload the conversations for `uuid`, or an empty store if there are none.
    ///
    /// A file that will not parse is treated as absent: losing the threads is
    /// bad, but refusing to start a review because of them would be worse.
    pub fn load_from(dir: &Path, uuid: &str) -> Self {
        let path = dir.join(format!("{uuid}.threads.json"));
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            // Nothing saved yet is the ordinary case, and the only one where
            // starting empty is safe. Any other read failure means there may be
            // a conversation there that we simply cannot see.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(err) => return Self::unreadable(&path, &err.to_string()),
        };
        let Ok(snapshot) = serde_json::from_str::<StoreSnapshot>(&raw) else {
            return Self::unreadable(&path, "the file is not valid review data");
        };

        let mut store = Self {
            next_id: snapshot.next_id,
            ..Self::default()
        };
        for mut thread in snapshot.threads {
            // `view` is not persisted, so it deserialises to zero -- the oldest
            // entry. A conversation should come back showing its latest word,
            // not its first.
            thread.view = thread.entry_count().saturating_sub(1);
            store
                .by_file
                .entry(thread.file.clone())
                .or_default()
                .push(thread.id);
            store.threads.insert(thread.id, thread);
        }
        // Older saves may hold several conversations about one line, from before
        // they were folded together.
        store.merge_by_anchor();
        store
    }

    /// Fold an agent event into the store when it belongs to `session`.
    ///
    /// A turn can outlive the review session it was started in: switching
    /// sessions drops the child only by refusing new work, and a reply already
    /// in flight still arrives. Thread ids are reused from 1 in every file, so
    /// without this check that reply would be written onto whichever thread now
    /// holds the same id. The session id stored on the thread is what the turn
    /// was actually answering.
    pub fn apply_agent_event_for(&mut self, session: &str, event: AgentEvent) {
        let id = match &event {
            AgentEvent::Started(id)
            | AgentEvent::Chunk(id, _)
            | AgentEvent::Completed(id, _)
            | AgentEvent::Failed(id, _) => *id,
        };
        let matches = self
            .threads
            .get(&id)
            .and_then(|thread| thread.agent_session.as_deref())
            == Some(session);
        if matches {
            self.apply_agent_event(event);
        }
    }

    /// Fold an agent event into the store.
    ///
    /// Chunks accumulate into a single in-progress agent message rather than
    /// appending one message per chunk, so the thread reads as one reply being
    /// written rather than a stream of fragments.
    pub fn apply_agent_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::Started(id) => {
                if let Some(thread) = self.threads.get_mut(&id) {
                    thread.awaiting = true;
                }
            }
            AgentEvent::Chunk(id, text) => {
                let Some(thread) = self.threads.get_mut(&id) else {
                    return;
                };
                let was_at_newest = thread.view_index() + 1 >= thread.entry_count();
                match thread.messages.last_mut() {
                    // Extend the reply being written, if the last entry is one.
                    Some(last) if last.role == Role::Agent && thread.awaiting => {
                        last.text.push_str(&text);
                    }
                    _ => thread.messages.push(Message {
                        role: Role::Agent,
                        text,
                        created_at: SystemTime::now(),
                    }),
                }
                if was_at_newest {
                    thread.view = thread.entry_count().saturating_sub(1);
                }
            }
            AgentEvent::Completed(id, text) => {
                let Some(thread) = self.threads.get_mut(&id) else {
                    return;
                };
                // The final text is authoritative: chunks may have been
                // coalesced, dropped, or never sent at all.
                match thread.messages.last_mut() {
                    Some(last) if last.role == Role::Agent && thread.awaiting => {
                        last.text = text;
                    }
                    _ => thread.messages.push(Message {
                        role: Role::Agent,
                        text,
                        created_at: SystemTime::now(),
                    }),
                }
                thread.awaiting = false;
                thread.view = thread.entry_count().saturating_sub(1);
            }
            AgentEvent::Failed(id, error) => {
                if let Some(thread) = self.threads.get_mut(&id) {
                    thread.awaiting = false;
                    thread.messages.push(Message {
                        role: Role::Agent,
                        text: format!("(failed) {error}"),
                        created_at: SystemTime::now(),
                    });
                    thread.view = thread.entry_count().saturating_sub(1);
                }
            }
        }
    }

    /// Whether any thread is waiting on a reply.
    pub fn any_awaiting(&self) -> bool {
        self.threads.values().any(|thread| thread.awaiting)
    }

    pub fn remove(&mut self, id: ThreadId) -> Option<Thread> {
        let thread = self.threads.remove(&id)?;
        if let Some(ids) = self.by_file.get_mut(&thread.file) {
            ids.retain(|candidate| *candidate != id);
            if ids.is_empty() {
                self.by_file.remove(&thread.file);
            }
        }
        Some(thread)
    }

    /// Write back positions an open document has been maintaining, so the store
    /// stays correct once that document closes.
    pub fn sync_from_anchors(&mut self, anchors: &[ReviewAnchor], text: &Rope) {
        for anchor in anchors {
            if let Some(thread) = self.threads.get_mut(&anchor.thread) {
                thread.line = anchor.line(text) as u32;
                thread.orphaned = anchor.orphaned;
            }
        }
    }
}

/// Render a thread into the virtual rows it occupies.
///
/// One row per screen line, already wrapped, so reserving and painting can both
/// just count the slice. Agent replies are plain wrapped text here; the editor
/// passes the markdown preview layout through [`render_comment_rows`].
pub fn comment_rows(
    thread: &Thread,
    width: usize,
    spinner: Option<&str>,
    attention: Attention,
    max_body_rows: usize,
) -> Vec<VirtualRow> {
    render_comment_rows(thread, width, spinner, attention, max_body_rows, &mut None)
}

/// [`comment_rows`], with an optional layout for agent replies.
///
/// `layout_agent` is the markdown preview renderer. It is asked only for an
/// agent entry, and what it returns is what the box shows. The message text is
/// not written back, so rendering a reply does not make it editable.
///
/// The `Option` is behind a `&mut` so a caller can hand the same layout to
/// every thread in a loop. Passing the inner `&mut` directly makes the borrow
/// last for the whole loop.
pub(crate) fn render_comment_rows(
    thread: &Thread,
    width: usize,
    spinner: Option<&str>,
    attention: Attention,
    max_body_rows: usize,
    layout_agent: &mut Option<&mut dyn FnMut(&str, usize) -> Vec<CommentLine>>,
) -> Vec<VirtualRow> {
    use crate::annotations::rows::{wrap_text, CommentRowKind};

    // Leave room for the marker column so wrapped text lines up under itself.
    let text_width = body_width(width);

    if thread.orphaned {
        thread.drawn.take();
        return wrap_text(&format!("(line gone) {}", thread.summary()), text_width)
            .into_iter()
            .map(|text| VirtualRow::Comment {
                thread: thread.id,
                kind: CommentRowKind::Orphaned,
                text,
                spans: Vec::new(),
                attention,
                mark: RowMark::None,
                body: None,
            })
            .collect();
    }

    if thread.collapsed {
        thread.drawn.take();
        let replies = thread.messages.len().saturating_sub(1);
        let summary = match replies {
            0 => format!("{COLLAPSED} {}", thread.summary()),
            1 => format!("{COLLAPSED} 1 reply · {}", thread.summary()),
            n => format!("{COLLAPSED} {n} replies · {}", thread.summary()),
        };
        let summary: String = summary.chars().take(text_width).collect();
        return vec![VirtualRow::Comment {
            thread: thread.id,
            kind: CommentRowKind::Summary,
            text: summary,
            spans: Vec::new(),
            attention,
            mark: RowMark::None,
            body: None,
        }];
    }

    // One entry at a time: a header carrying the role and position, then that
    // entry's body. The thread's height is therefore bounded by a single
    // message, not by how long the conversation has grown.
    // A reply on its way counts toward the total, so `3/4` says an answer is
    // coming, but it does not take the view: the reader has just written the
    // entry they are looking at and hiding it behind a placeholder would answer
    // a question nobody asked. The spinner in the header carries the waiting.
    let total = thread.entry_count() + usize::from(thread.awaiting_reply());
    let index = thread.view_index();

    let Some(entry) = thread.entry(index) else {
        return Vec::new();
    };

    let kind = if entry.pending {
        CommentRowKind::Pending
    } else if entry.label == "agent" {
        CommentRowKind::Agent
    } else {
        CommentRowKind::User
    };

    let mut header = header_text(entry.label, index + 1, total, text_width);
    // While a reply is in flight the header carries the spinner, so the wait is
    // visible on the thread it belongs to rather than only in the statusline.
    if thread.awaiting {
        if let Some(frame) = spinner {
            header = header_text(
                &format!("{} {frame}", entry.label),
                index + 1,
                total,
                text_width,
            );
        }
    }

    let mut rows = vec![VirtualRow::Comment {
        thread: thread.id,
        kind: CommentRowKind::Summary,
        text: header,
        spans: Vec::new(),
        attention,
        mark: RowMark::None,
        body: None,
    }];

    // A box is never allowed to outgrow the window. Scrolling *through* one is
    // not possible: the cursor cannot be inside virtual rows, so any view that
    // scrolled into the middle of a box would be dragged straight back to the
    // cursor's line on the next frame. Capping it and scrolling *within* it
    // keeps the code being reviewed on screen, which is the point of the
    // exercise.
    //
    // An agent reply is laid out by the markdown preview renderer when one was
    // given. The source stays the source: deleting the entry is what removes
    // it, and nothing in this path writes the rendered text back.
    let body = if entry.label == "agent" {
        match layout_agent.as_deref_mut() {
            Some(layout) => {
                let lines = layout(entry.text, text_width);
                if lines.is_empty() {
                    vec![CommentLine::plain("")]
                } else {
                    lines
                }
            }
            None => wrap_text(entry.text, text_width)
                .into_iter()
                .map(CommentLine::plain)
                .collect(),
        }
    } else {
        wrap_text(entry.text, text_width)
            .into_iter()
            .map(CommentLine::plain)
            .collect()
    };
    thread.drawn.replace(Some(DrawnBody {
        view: index,
        width: text_width,
        lines: body.iter().map(CommentLine::text).collect(),
    }));
    let max_body_rows = max_body_rows.max(1);
    let last = body.len().saturating_sub(1);
    let cursor = thread.cursor.min(last);

    // The cursor drags the box: it scrolls only when the cursor would otherwise
    // leave it. Settled here rather than where the cursor moves because only
    // drawing knows how tall the box was allowed to be.
    let mut scroll = thread.scroll.get().min(last);
    if cursor < scroll {
        scroll = cursor;
    } else if cursor >= scroll + max_body_rows {
        scroll = cursor + 1 - max_body_rows;
    }
    thread.scroll.set(scroll);

    // Only a focused box draws a cursor. Marking every box would say the keys
    // act on all of them.
    let selected = (attention == Attention::Focused).then(|| {
        thread.select.map(|anchor| {
            let anchor = anchor.min(last);
            (anchor.min(cursor), anchor.max(cursor))
        })
    });
    let visible = body.len().min(max_body_rows);

    if body.len() > max_body_rows {
        // Say which part is on screen, in the header that is already there,
        // rather than spending a row on saying it.
        let shown = format!(
            "{}-{} of {}",
            scroll + 1,
            (scroll + visible).min(body.len()),
            body.len()
        );
        if let Some(VirtualRow::Comment { text, .. }) = rows.first_mut() {
            let room = text_width.saturating_sub(shown.chars().count() + 1);
            let trimmed: String = text.chars().take(room).collect();
            *text = format!("{trimmed} {shown}");
        }
    }

    rows.extend(
        body.into_iter()
            .enumerate()
            .skip(scroll)
            .take(max_body_rows)
            .map(|(row, line)| {
                let (text, spans) = styled_line(line);
                VirtualRow::Comment {
                    thread: thread.id,
                    kind,
                    text,
                    spans,
                    attention,
                    mark: match selected {
                        None => RowMark::None,
                        Some(_) if row == cursor => RowMark::Cursor,
                        Some(Some((start, end))) if (start..=end).contains(&row) => {
                            RowMark::Selected
                        }
                        Some(_) => RowMark::None,
                    },
                    body: Some(row),
                }
            }),
    );

    rows
}

/// Keep markdown styling only when a span actually carries some. A plain row
/// stays a single string so it is painted with the comment's own colour.
fn styled_line(line: CommentLine) -> (String, Vec<CommentSpan>) {
    use crate::graphics::Style;

    let text = line.text();
    let spans = if line.spans.iter().any(|span| span.style != Style::default()) {
        line.spans
    } else {
        Vec::new()
    };
    (text, spans)
}

/// `agent 3/5 ─────────` filled to the pane width, so the block reads as one
/// object rather than as loose lines under the code.
fn header_text(label: &str, index: usize, total: usize, width: usize) -> String {
    let head = format!("{label} {index}/{total} ");
    let fill = width.saturating_sub(head.chars().count());
    let mut header = head;
    header.extend(std::iter::repeat('─').take(fill));
    header
}

/// Marker drawn in the left column of every comment row.
pub const MARKER: &str = "▏";
/// Leading glyph for a collapsed thread.
pub const COLLAPSED: &str = "▸";

#[cfg(test)]
mod test {
    use super::*;
    use helix_core::Transaction;

    fn rope() -> Rope {
        Rope::from("alpha\nbeta\ngamma\n")
    }

    fn anchor_on(text: &Rope, line: usize) -> ReviewAnchor {
        ReviewAnchor::for_line(ThreadId(0), text, line)
    }

    #[test]
    fn anchor_covers_its_line() {
        let text = rope();
        let anchor = anchor_on(&text, 1);
        assert_eq!(anchor.line(&text), 1);
        assert_eq!(
            text.slice(anchor.start..anchor.end).to_string(),
            "beta\n".to_string()
        );
    }

    #[test]
    fn insert_above_pushes_the_anchor_down() {
        let text = rope();
        let mut anchors = [anchor_on(&text, 1)];
        let transaction = Transaction::change(&text, [(0, 0, Some("zero\n".into()))].into_iter());
        let mut after = text.clone();
        transaction.apply(&mut after);

        remap_anchors(&mut anchors, transaction.changes());

        assert!(!anchors[0].orphaned);
        assert_eq!(anchors[0].line(&after), 2);
        assert_eq!(
            after.slice(anchors[0].start..anchors[0].end).to_string(),
            "beta\n".to_string()
        );
    }

    #[test]
    fn insert_below_leaves_the_anchor_alone() {
        let text = rope();
        let mut anchors = [anchor_on(&text, 1)];
        let end = text.len_chars();
        let transaction =
            Transaction::change(&text, [(end, end, Some("delta\n".into()))].into_iter());
        let mut after = text.clone();
        transaction.apply(&mut after);

        remap_anchors(&mut anchors, transaction.changes());

        assert!(!anchors[0].orphaned);
        assert_eq!(anchors[0].line(&after), 1);
    }

    #[test]
    fn editing_within_the_line_keeps_the_anchor() {
        let text = rope();
        let mut anchors = [anchor_on(&text, 1)];
        // "beta" -> "better"
        let start = text.line_to_char(1);
        let transaction = Transaction::change(
            &text,
            [(start, start + 4, Some("better".into()))].into_iter(),
        );
        let mut after = text.clone();
        transaction.apply(&mut after);

        remap_anchors(&mut anchors, transaction.changes());

        assert!(!anchors[0].orphaned);
        assert_eq!(anchors[0].line(&after), 1);
        assert_eq!(
            after.slice(anchors[0].start..anchors[0].end).to_string(),
            "better\n".to_string()
        );
    }

    #[test]
    fn deleting_the_line_orphans_rather_than_drops() {
        let text = rope();
        let mut anchors = [anchor_on(&text, 1)];
        let start = text.line_to_char(1);
        let end = text.line_to_char(2);
        let transaction = Transaction::change(&text, [(start, end, None)].into_iter());
        let mut after = text.clone();
        transaction.apply(&mut after);

        remap_anchors(&mut anchors, transaction.changes());

        assert!(
            anchors[0].orphaned,
            "deleting the anchored line should orphan the thread, not lose it"
        );
        assert_eq!(anchors[0].start, anchors[0].end);
    }

    #[test]
    fn drafts_become_messages_once_sent() {
        let mut store = ReviewStore::default();
        let id = store.draft(
            PathBuf::from("/repo/src/main.rs"),
            DiffSide::Working,
            41,
            "why this branch?".into(),
        );

        assert_eq!(store.pending_count(), 1);
        assert!(store.get(id).unwrap().is_pending());

        let sent = store.take_draft(id).unwrap();
        assert_eq!(sent, "why this branch?");
        assert_eq!(store.pending_count(), 0);

        store.push_message(id, Role::Agent, "because X".into());
        let thread = store.get(id).unwrap();
        assert_eq!(thread.messages.len(), 2);
        assert_eq!(thread.messages[0].role, Role::User);
        assert_eq!(thread.messages[1].role, Role::Agent);
        assert_eq!(thread.summary(), "why this branch?");
    }

    fn body_of(thread: &Thread, width: usize) -> Vec<String> {
        comment_rows(thread, width, None, Attention::Idle, usize::MAX)
            .into_iter()
            .skip(1) // header
            .map(|row| match row {
                VirtualRow::Comment { text, .. } => text,
                other => unreachable!("comment rows only, got {other:?}"),
            })
            .collect()
    }

    fn header_of(thread: &Thread, width: usize) -> String {
        match &comment_rows(thread, width, Some("⣾"), Attention::Idle, usize::MAX)[0] {
            VirtualRow::Comment { text, .. } => text.clone(),
            other => unreachable!("comment rows only, got {other:?}"),
        }
    }

    #[test]
    fn a_thread_shows_one_entry_with_a_counter() {
        let mut store = ReviewStore::default();
        let id = store.draft(
            PathBuf::from("/r/a.rs"),
            DiffSide::Working,
            1,
            "why?".into(),
        );
        store.take_draft(id);
        store.push_message(id, Role::Agent, "because X".into());
        store.push_message(id, Role::User, "and Y?".into());

        let thread = store.get(id).unwrap();
        assert_eq!(thread.entry_count(), 3);
        // Follows the newest, since the reader had not moved.
        assert_eq!(thread.view_index(), 2);
        assert!(header_of(thread, 40).starts_with("you 3/3 "));
        assert_eq!(body_of(thread, 40), vec!["and Y?"]);
    }

    #[test]
    fn copying_takes_the_selection_or_the_whole_entry() {
        let mut store = ReviewStore::default();
        let id = store.draft(
            PathBuf::from("/r/a.rs"),
            DiffSide::Working,
            1,
            "why?".into(),
        );
        store.take_draft(id);
        store.push_message(id, Role::Agent, "alpha\nbeta\ngamma".into());
        assert_eq!(
            body_of(store.get(id).unwrap(), 40),
            vec!["alpha", "beta", "gamma"]
        );

        // Nothing selected: the whole entry, as it was written.
        assert_eq!(
            store.get(id).unwrap().copy_text(40).unwrap(),
            "alpha\nbeta\ngamma"
        );

        // One row, pointed at with the in-box cursor.
        let thread = store.get_mut(id).unwrap();
        thread.cursor = 1;
        thread.select = Some(1);
        assert_eq!(thread.selected_rows(40), Some((1, 1)));
        assert_eq!(thread.copy_text(40).unwrap(), "beta");

        // Backwards from the anchor reads the same either way round.
        let thread = store.get_mut(id).unwrap();
        thread.cursor = 0;
        thread.select = Some(2);
        assert_eq!(thread.selected_rows(40), Some((0, 2)));
        // ... and a selection covering everything gives the entry verbatim
        // rather than the rows it happened to be drawn as.
        assert_eq!(thread.copy_text(40).unwrap(), "alpha\nbeta\ngamma");
    }

    #[test]
    fn a_box_scrolls_only_to_keep_its_cursor_in_view() {
        let mut store = ReviewStore::default();
        let id = store.draft(
            PathBuf::from("/r/a.rs"),
            DiffSide::Working,
            1,
            "why?".into(),
        );
        store.take_draft(id);
        let reply: String = (0..20).map(|n| format!("line {n}\n")).collect();
        store.push_message(id, Role::Agent, reply);

        let thread = store.get_mut(id).unwrap();
        thread.cursor = 2;
        // Drawing settles it: a cursor already inside the box moves nothing.
        let _ = comment_rows(thread, 40, None, Attention::Focused, 5);
        assert_eq!(thread.scroll.get(), 0);

        thread.cursor = 7;
        let _ = comment_rows(thread, 40, None, Attention::Focused, 5);
        assert_eq!(thread.scroll.get(), 3, "just far enough to keep it visible");

        thread.cursor = 1;
        let _ = comment_rows(thread, 40, None, Attention::Focused, 5);
        assert_eq!(thread.scroll.get(), 1, "and back the other way");
    }

    #[test]
    fn only_a_focused_box_draws_its_cursor() {
        use crate::annotations::rows::RowMark;

        let mut store = ReviewStore::default();
        let id = store.draft(
            PathBuf::from("/r/a.rs"),
            DiffSide::Working,
            1,
            "why?".into(),
        );
        store.take_draft(id);
        store.push_message(id, Role::Agent, "alpha\nbeta".into());
        let thread = store.get_mut(id).unwrap();
        thread.cursor = 1;
        thread.select = Some(0);

        let marks = |attention| -> Vec<RowMark> {
            comment_rows(thread, 40, None, attention, usize::MAX)
                .into_iter()
                .skip(1) // header
                .map(|row| match row {
                    VirtualRow::Comment { mark, .. } => mark,
                    other => unreachable!("comment rows only, got {other:?}"),
                })
                .collect()
        };

        assert_eq!(
            marks(Attention::Focused),
            vec![RowMark::Selected, RowMark::Cursor]
        );
        assert_eq!(
            marks(Attention::UnderCursor),
            vec![RowMark::None, RowMark::None],
            "a box the keys do not act on must not look like it has a cursor"
        );
    }

    #[test]
    fn an_agent_reply_is_rendered_without_changing_its_text() {
        use crate::graphics::{Modifier, Style};

        let mut store = ReviewStore::default();
        let id = store.draft(
            PathBuf::from("/r/a.rs"),
            DiffSide::Working,
            1,
            "**keep**".into(),
        );
        store.take_draft(id);
        store.push_message(id, Role::Agent, "**bold**\nmore".into());

        let mut layout = |_text: &str, _width: usize| {
            vec![
                CommentLine {
                    spans: vec![CommentSpan {
                        text: "bold".into(),
                        style: Style::default().add_modifier(Modifier::BOLD),
                    }],
                },
                CommentLine::plain("more"),
            ]
        };
        let thread = store.get(id).unwrap();
        let agent = render_comment_rows(
            thread,
            40,
            None,
            Attention::Idle,
            usize::MAX,
            &mut Some(&mut layout),
        );
        let body: Vec<_> = agent
            .into_iter()
            .skip(1)
            .map(|row| match row {
                VirtualRow::Comment { text, spans, .. } => (text, spans),
                other => unreachable!("comment rows only, got {other:?}"),
            })
            .collect();
        assert_eq!(body.len(), 2);
        assert_eq!(body[0].0, "bold");
        assert!(body[0].1.iter().any(|span| {
            span.text == "bold" && span.style.add_modifier.contains(Modifier::BOLD)
        }));
        // Rendering is not an edit.
        assert_eq!(
            thread.entry(thread.view_index()).unwrap().text,
            "**bold**\nmore"
        );

        // Nothing selected copies the source. A partial selection copies the
        // rendered rows, which is what is on screen.
        assert_eq!(thread.copy_text(40).unwrap(), "**bold**\nmore");
        let thread = store.get_mut(id).unwrap();
        thread.cursor = 0;
        thread.select = Some(0);
        assert_eq!(thread.copy_text(40).unwrap(), "bold");

        // The user's own comment is not run through the agent renderer.
        store.get_mut(id).unwrap().step_view(false);
        let user = comment_rows(
            store.get(id).unwrap(),
            40,
            None,
            Attention::Idle,
            usize::MAX,
        );
        let user_body: Vec<_> = user
            .into_iter()
            .skip(1)
            .map(|row| match row {
                VirtualRow::Comment { text, .. } => text,
                other => unreachable!("comment rows only, got {other:?}"),
            })
            .collect();
        assert_eq!(user_body, vec!["**keep**"]);

        // The rendered reply can still be removed. The comment beside it stays.
        assert_eq!(store.remove_entry(id, 1), Some(1));
        let thread = store.get(id).unwrap();
        assert_eq!(thread.entry_count(), 1);
        assert_eq!(thread.entry(0).unwrap().text, "**keep**");
    }

    #[test]
    fn stepping_back_shows_an_older_entry_and_saturates() {
        let mut store = ReviewStore::default();
        let id = store.draft(
            PathBuf::from("/r/a.rs"),
            DiffSide::Working,
            1,
            "why?".into(),
        );
        store.take_draft(id);
        store.push_message(id, Role::Agent, "because X".into());

        let thread = store.get_mut(id).unwrap();
        assert!(thread.step_view(false));
        assert_eq!(thread.view_index(), 0);
        assert!(header_of(thread, 40).starts_with("you 1/2 "));
        assert_eq!(body_of(thread, 40), vec!["why?"]);

        // Already oldest: no move, and no wrap around to the newest.
        assert!(!thread.step_view(false));
        assert_eq!(thread.view_index(), 0);
    }

    #[test]
    fn a_reply_does_not_yank_a_reader_who_paged_back() {
        let mut store = ReviewStore::default();
        let id = store.draft(
            PathBuf::from("/r/a.rs"),
            DiffSide::Working,
            1,
            "why?".into(),
        );
        store.take_draft(id);
        store.push_message(id, Role::Agent, "because X".into());
        store.get_mut(id).unwrap().step_view(false);
        assert_eq!(store.get(id).unwrap().view_index(), 0);

        store.push_message(id, Role::Agent, "and also Z".into());

        let thread = store.get(id).unwrap();
        assert_eq!(thread.view_index(), 0, "reader should keep their place");
        // The total grows, which is how the new reply announces itself.
        assert!(header_of(thread, 40).starts_with("you 1/3 "));
    }

    #[test]
    fn an_unsent_draft_is_the_last_entry() {
        let mut store = ReviewStore::default();
        let id = store.draft(
            PathBuf::from("/r/a.rs"),
            DiffSide::Working,
            1,
            "first".into(),
        );
        store.take_draft(id);
        store.push_message(id, Role::Agent, "reply".into());
        store.set_draft(id, "not sent yet".into());

        let thread = store.get(id).unwrap();
        assert_eq!(thread.entry_count(), 3);
        thread_view_is_last(thread);
        assert!(header_of(thread, 40).starts_with("draft 3/3 "));
        assert_eq!(body_of(thread, 40), vec!["not sent yet"]);
    }

    fn thread_view_is_last(thread: &Thread) {
        assert_eq!(thread.view_index(), thread.entry_count() - 1);
    }

    #[test]
    fn a_long_entry_does_not_multiply_with_thread_length() {
        let mut store = ReviewStore::default();
        let long = "word ".repeat(60);
        let id = store.draft(PathBuf::from("/r/a.rs"), DiffSide::Working, 1, long.clone());
        store.take_draft(id);
        let one_entry = comment_rows(
            store.get(id).unwrap(),
            40,
            None,
            Attention::Idle,
            usize::MAX,
        )
        .len();

        for _ in 0..5 {
            store.push_message(id, Role::Agent, long.clone());
        }
        assert_eq!(
            comment_rows(
                store.get(id).unwrap(),
                40,
                None,
                Attention::Idle,
                usize::MAX
            )
            .len(),
            one_entry,
            "height must not grow with the conversation"
        );
    }

    fn sent_thread() -> (ReviewStore, ThreadId) {
        let mut store = ReviewStore::default();
        let id = store.draft(
            PathBuf::from("/r/a.rs"),
            DiffSide::Working,
            1,
            "why?".into(),
        );
        store.take_draft(id);
        (store, id)
    }

    #[test]
    fn chunks_accumulate_into_one_reply() {
        let (mut store, id) = sent_thread();
        store.apply_agent_event(AgentEvent::Started(id));
        assert!(store.get(id).unwrap().awaiting);
        assert!(store.any_awaiting());

        store.apply_agent_event(AgentEvent::Chunk(id, "because ".into()));
        store.apply_agent_event(AgentEvent::Chunk(id, "X".into()));

        let thread = store.get(id).unwrap();
        assert_eq!(
            thread.entry_count(),
            2,
            "a streamed reply must be one entry, not one per chunk"
        );
        assert_eq!(thread.entry(1).unwrap().text, "because X");
    }

    #[test]
    fn the_final_text_replaces_what_was_streamed() {
        let (mut store, id) = sent_thread();
        store.apply_agent_event(AgentEvent::Started(id));
        store.apply_agent_event(AgentEvent::Chunk(id, "partial".into()));
        store.apply_agent_event(AgentEvent::Completed(id, "the whole answer".into()));

        let thread = store.get(id).unwrap();
        assert!(!thread.awaiting);
        assert!(!store.any_awaiting());
        assert_eq!(thread.entry_count(), 2);
        assert_eq!(thread.entry(1).unwrap().text, "the whole answer");
        assert_eq!(thread.view_index(), 1, "a completed reply is shown");
    }

    #[test]
    fn a_reply_with_no_streaming_still_lands() {
        let (mut store, id) = sent_thread();
        store.apply_agent_event(AgentEvent::Started(id));
        store.apply_agent_event(AgentEvent::Completed(id, "straight to the point".into()));

        let thread = store.get(id).unwrap();
        assert_eq!(thread.entry_count(), 2);
        assert_eq!(thread.entry(1).unwrap().text, "straight to the point");
    }

    #[test]
    fn each_thread_keeps_its_own_agent_session() {
        let dir = tempfile::tempdir().unwrap();
        let uuid = "12121212-3434-4545-8686-787878787878";
        let mut store = ReviewStore::default();
        let first = store.draft(PathBuf::from("/r/a.rs"), DiffSide::Working, 1, "one".into());
        let second = store.draft(PathBuf::from("/r/a.rs"), DiffSide::Working, 4, "two".into());

        let first_session = store.ensure_agent_session(first).unwrap();
        let second_session = store.ensure_agent_session(second).unwrap();
        assert_ne!(first_session, second_session);
        assert_eq!(
            store.ensure_agent_session(first).as_deref(),
            Some(first_session.as_str()),
            "a follow-up must resume the session the first send created"
        );

        store.save_to(dir.path(), uuid).unwrap();
        let back = ReviewStore::load_from(dir.path(), uuid);
        assert_eq!(
            back.get(first).unwrap().agent_session.as_deref(),
            Some(first_session.as_str())
        );
        assert_eq!(
            back.get(second).unwrap().agent_session.as_deref(),
            Some(second_session.as_str())
        );

        // A save from before comments had their own conversations omits the field.
        let legacy = r#"{
            "threads": [{
                "id": 1,
                "file": "/r/a.rs",
                "side": "working",
                "line": 1,
                "messages": [],
                "draft": "why?",
                "collapsed": false,
                "rewound": false,
                "orphaned": false
            }],
            "next_id": 2
        }"#;
        std::fs::write(dir.path().join(format!("{uuid}.threads.json")), legacy).unwrap();
        let legacy = ReviewStore::load_from(dir.path(), uuid);
        assert_eq!(legacy.get(ThreadId(1)).unwrap().agent_session, None);
    }

    #[test]
    fn an_event_for_another_conversation_is_ignored() {
        let (mut store, id) = sent_thread();
        let session = store.ensure_agent_session(id).unwrap();
        store.apply_agent_event_for(&session, AgentEvent::Started(id));
        assert!(store.get(id).unwrap().awaiting);

        store.apply_agent_event_for("someone-else", AgentEvent::Chunk(id, "nope".into()));
        assert_eq!(
            store.get(id).unwrap().entry_count(),
            1,
            "a reply from another conversation must not be written here"
        );

        store.apply_agent_event_for(&session, AgentEvent::Chunk(id, "yes".into()));
        assert_eq!(store.get(id).unwrap().entry(1).unwrap().text, "yes");
    }

    #[test]
    fn a_failure_is_shown_on_the_thread_that_asked() {
        let (mut store, id) = sent_thread();
        store.apply_agent_event(AgentEvent::Started(id));
        store.apply_agent_event(AgentEvent::Failed(id, "process ended".into()));

        let thread = store.get(id).unwrap();
        assert!(!thread.awaiting, "a failure must stop the spinner");
        assert!(thread.entry(1).unwrap().text.contains("process ended"));
    }

    #[test]
    fn a_conversation_survives_a_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let uuid = "11111111-2222-5333-8444-555555555555";

        let mut store = ReviewStore::default();
        let asked = store.draft(
            PathBuf::from("/r/a.rs"),
            DiffSide::Working,
            41,
            "why?".into(),
        );
        store.take_draft(asked);
        store.push_message(asked, Role::Agent, "because X".into());
        store.get_mut(asked).unwrap().collapsed = true;
        // A second thread, unsent: the draft is the thing most worth not losing.
        let unsent = store.draft(
            PathBuf::from("/r/b.rs"),
            DiffSide::Base,
            7,
            "typed, not sent".into(),
        );

        store.save_to(dir.path(), uuid).unwrap();
        let back = ReviewStore::load_from(dir.path(), uuid);

        assert_eq!(back.len(), 2);
        let asked_back = back.get(asked).unwrap();
        assert_eq!(asked_back.entry_count(), 2);
        assert_eq!(asked_back.entry(1).unwrap().text, "because X");
        assert_eq!(asked_back.line, 41);
        assert_eq!(asked_back.side, DiffSide::Working);
        assert!(asked_back.collapsed, "collapse state is worth keeping");

        let unsent_back = back.get(unsent).unwrap();
        assert_eq!(unsent_back.draft.as_deref(), Some("typed, not sent"));
        assert_eq!(back.pending_count(), 1);

        // The per-file index is rebuilt rather than persisted.
        assert_eq!(back.for_file(Path::new("/r/a.rs")).count(), 1);
        assert_eq!(back.for_file(Path::new("/r/b.rs")).count(), 1);
    }

    #[test]
    fn a_reloaded_conversation_opens_on_its_newest_entry() {
        let dir = tempfile::tempdir().unwrap();
        let uuid = "55555555-6666-5777-8888-999999999999";

        let mut store = ReviewStore::default();
        let id = store.draft(
            PathBuf::from("/r/a.rs"),
            DiffSide::Working,
            1,
            "why?".into(),
        );
        store.take_draft(id);
        store.push_message(id, Role::Agent, "because X".into());
        store.save_to(dir.path(), uuid).unwrap();

        let back = ReviewStore::load_from(dir.path(), uuid);
        let thread = back.get(id).unwrap();
        assert_eq!(
            thread.view_index(),
            thread.entry_count() - 1,
            "a conversation should come back showing its latest word"
        );
        assert_eq!(thread.entry(thread.view_index()).unwrap().text, "because X");
    }

    #[test]
    fn a_reply_in_flight_does_not_come_back_spinning() {
        let dir = tempfile::tempdir().unwrap();
        let uuid = "22222222-3333-5444-8555-666666666666";

        let mut store = ReviewStore::default();
        let id = store.draft(
            PathBuf::from("/r/a.rs"),
            DiffSide::Working,
            1,
            "why?".into(),
        );
        store.take_draft(id);
        store.apply_agent_event(AgentEvent::Started(id));
        assert!(store.any_awaiting());

        store.save_to(dir.path(), uuid).unwrap();
        let back = ReviewStore::load_from(dir.path(), uuid);

        assert!(
            !back.any_awaiting(),
            "a turn in flight when the editor died will never land"
        );
    }

    #[test]
    fn new_threads_do_not_reuse_reloaded_ids() {
        let dir = tempfile::tempdir().unwrap();
        let uuid = "33333333-4444-5555-8666-777777777777";

        let mut store = ReviewStore::default();
        let first = store.draft(PathBuf::from("/r/a.rs"), DiffSide::Working, 1, "one".into());
        store.save_to(dir.path(), uuid).unwrap();

        let mut back = ReviewStore::load_from(dir.path(), uuid);
        let second = back.draft(PathBuf::from("/r/a.rs"), DiffSide::Working, 2, "two".into());
        assert_ne!(
            first, second,
            "a reloaded store must not hand out a used id"
        );
        assert_eq!(back.len(), 2);
    }

    #[test]
    fn an_unreadable_file_starts_empty_rather_than_failing() {
        let dir = tempfile::tempdir().unwrap();
        let uuid = "44444444-5555-5666-8777-888888888888";
        std::fs::write(
            dir.path().join(format!("{uuid}.threads.json")),
            "{ not json",
        )
        .unwrap();

        let store = ReviewStore::load_from(dir.path(), uuid);
        assert!(store.is_empty(), "a corrupt file must not stop a review");
    }

    #[test]
    fn a_question_stays_visible_while_its_answer_is_awaited() {
        let mut store = ReviewStore::default();
        let id = store.draft(
            PathBuf::from("/r/a.rs"),
            DiffSide::Working,
            1,
            "why?".into(),
        );
        store.take_draft(id);
        store.apply_agent_event(AgentEvent::Started(id));

        let thread = store.get(id).unwrap();
        assert!(thread.awaiting_reply());
        // The question just written stays on screen; the total says an answer is
        // on its way and the spinner says it has not arrived.
        let header = header_of(thread, 40);
        assert!(header.starts_with("you ⣾ 1/2 "), "got {header:?}");
        assert_eq!(body_of(thread, 40), vec!["why?"]);
    }

    #[test]
    fn the_view_follows_the_answer_once_it_starts_arriving() {
        let mut store = ReviewStore::default();
        let id = store.draft(
            PathBuf::from("/r/a.rs"),
            DiffSide::Working,
            1,
            "why?".into(),
        );
        store.take_draft(id);
        store.apply_agent_event(AgentEvent::Started(id));
        store.apply_agent_event(AgentEvent::Chunk(id, "because".into()));

        let thread = store.get(id).unwrap();
        assert!(
            !thread.awaiting_reply(),
            "text has arrived, so nothing is pending"
        );
        assert_eq!(thread.entry_count(), 2, "the pending entry was never real");
        assert!(header_of(thread, 40).starts_with("agent ⣾ 2/2 "));
        assert_eq!(body_of(thread, 40), vec!["because"]);
    }

    #[test]
    fn waiting_does_not_yank_a_reader_who_paged_back() {
        let mut store = ReviewStore::default();
        let id = store.draft(
            PathBuf::from("/r/a.rs"),
            DiffSide::Working,
            1,
            "why?".into(),
        );
        store.take_draft(id);
        store.push_message(id, Role::Agent, "because X".into());
        store.set_draft(id, "and Y?".into());
        store.take_draft(id);
        store.get_mut(id).unwrap().view = 0;
        store.apply_agent_event(AgentEvent::Started(id));

        // Still reading the first entry, so nothing should move the view. The
        // total counts the reply on its way, which is how the reader learns one
        // is coming.
        let thread = store.get(id).unwrap();
        let header = header_of(thread, 40);
        assert!(header.starts_with("you "), "got {header:?}");
        assert!(header.contains("1/4"), "got {header:?}");
        assert_eq!(body_of(thread, 40), vec!["why?"]);
    }

    #[test]
    fn one_line_has_one_conversation() {
        let mut store = ReviewStore::default();
        let file = PathBuf::from("/r/a.rs");
        let first = store.draft(file.clone(), DiffSide::Working, 7, "why?".into());
        store.take_draft(first);
        store.push_message(first, Role::Agent, "because X".into());

        // A second comment on the same line joins the conversation rather than
        // starting a rival one beside it.
        let second = store.draft(file.clone(), DiffSide::Working, 7, "and this?".into());
        assert_eq!(second, first);
        assert_eq!(store.len(), 1);
        assert_eq!(store.get(first).unwrap().entry_count(), 3);

        // A different line, and the other side of a diff, stay separate.
        let elsewhere = store.draft(file.clone(), DiffSide::Working, 8, "over here".into());
        let other_side = store.draft(file, DiffSide::Base, 7, "old side".into());
        assert_ne!(elsewhere, first);
        assert_ne!(other_side, first);
        assert_eq!(store.len(), 3);
    }

    #[test]
    fn an_unreadable_save_is_kept_and_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let uuid = "11111111-2222-5333-8444-555555555555";
        let path = dir.path().join(format!("{uuid}.threads.json"));
        std::fs::write(&path, "{ this is not review data").unwrap();

        let store = ReviewStore::load_from(dir.path(), uuid);
        assert!(store.is_empty());
        assert!(store.load_error.is_some(), "the reader should be told");

        // The unreadable file is kept under another name rather than left in
        // place to be written over.
        assert!(!path.exists());
        let kept: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .contains("json.unreadable.")
            })
            .collect();
        assert_eq!(kept.len(), 1, "the original should still be on disk");
        assert_eq!(
            std::fs::read_to_string(kept[0].path()).unwrap(),
            "{ this is not review data"
        );

        // And having been moved aside, saving is free to carry on.
        assert!(!store.save_blocked);
        assert!(store.save_to(dir.path(), uuid).is_ok());
    }

    #[test]
    fn a_store_that_could_not_be_moved_aside_refuses_to_save() {
        let dir = tempfile::tempdir().unwrap();
        let store = ReviewStore {
            save_blocked: true,
            ..ReviewStore::default()
        };
        assert!(
            store
                .save_to(dir.path(), "11111111-2222-5333-8444-555555555555")
                .is_err(),
            "an empty store must never replace threads we merely failed to read"
        );
    }

    #[test]
    fn older_saves_with_rival_threads_are_folded_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let uuid = "66666666-7777-5888-8999-aaaaaaaaaaaa";
        let file = PathBuf::from("/r/a.rs");

        // Built the way the duplicate bug used to leave them: two conversations
        // describing the same line.
        let mut store = ReviewStore::default();
        let first = store.draft(file.clone(), DiffSide::Working, 7, "why?".into());
        store.take_draft(first);
        store.push_message(first, Role::Agent, "because X".into());
        let second = ThreadId(99);
        store.by_file.entry(file.clone()).or_default().push(second);
        store.threads.insert(
            second,
            Thread {
                id: second,
                file: file.clone(),
                side: DiffSide::Working,
                line: 7,
                messages: vec![Message {
                    role: Role::User,
                    text: "a test".into(),
                    created_at: SystemTime::now(),
                }],
                draft: None,
                collapsed: false,
                view: 0,
                scroll: Cell::new(0),
                drawn: RefCell::new(None),
                cursor: 0,
                select: None,
                awaiting: false,
                rewound: false,
                orphaned: false,
                agent_session: None,
            },
        );
        store.save_to(dir.path(), uuid).unwrap();

        let back = ReviewStore::load_from(dir.path(), uuid);
        assert_eq!(back.len(), 1, "one line, one conversation");
        let thread = back.get(first).unwrap();
        assert_eq!(thread.entry_count(), 3, "every message should survive");
        assert_eq!(
            thread.view_index(),
            2,
            "a folded conversation opens on its newest entry"
        );
        // Interleaved by when they were written, not by which thread they were in.
        let texts: Vec<_> = (0..thread.entry_count())
            .map(|i| thread.entry(i).unwrap().text.to_string())
            .collect();
        assert_eq!(texts, vec!["why?", "because X", "a test"]);
    }

    #[test]
    fn threads_are_indexed_by_file_and_removable() {
        let mut store = ReviewStore::default();
        let a = PathBuf::from("/repo/a.rs");
        let b = PathBuf::from("/repo/b.rs");
        let first = store.draft(a.clone(), DiffSide::Working, 1, "one".into());
        let second = store.draft(a.clone(), DiffSide::Base, 2, "two".into());
        let third = store.draft(b.clone(), DiffSide::Working, 3, "three".into());

        assert_eq!(store.for_file(&a).count(), 2);
        assert_eq!(store.for_file(&b).count(), 1);

        store.remove(second);
        assert_eq!(store.for_file(&a).count(), 1);
        assert_eq!(store.len(), 2);
        assert!(store.get(second).is_none());
        assert!(store.get(first).is_some());
        assert!(store.get(third).is_some());
    }

    #[test]
    fn sync_writes_positions_back_to_the_store() {
        let text = rope();
        let mut store = ReviewStore::default();
        let id = store.draft(
            PathBuf::from("/repo/a.rs"),
            DiffSide::Working,
            1,
            "q".into(),
        );
        let mut anchors = [ReviewAnchor::for_line(id, &text, 1)];

        let transaction = Transaction::change(&text, [(0, 0, Some("zero\n".into()))].into_iter());
        let mut after = text.clone();
        transaction.apply(&mut after);
        remap_anchors(&mut anchors, transaction.changes());

        store.sync_from_anchors(&anchors, &after);
        assert_eq!(store.get(id).unwrap().line, 2);
        assert!(!store.get(id).unwrap().orphaned);
    }

    #[test]
    fn removing_a_draft_only_thread_does_not_leave_a_ghost() {
        let mut store = ReviewStore::default();
        let file = PathBuf::from("/r/a.rs");
        let id = store.draft(file.clone(), DiffSide::Working, 7, "why?".into());

        assert_eq!(store.remove_entry(id, 0), None);
        assert!(store.is_empty(), "the empty thread should be gone");
        assert!(store.get(id).is_none());
        assert_eq!(store.thread_at(&file, DiffSide::Working, 7), None);
    }

    #[test]
    fn removing_a_draft_keeps_the_sent_messages() {
        let mut store = ReviewStore::default();
        let file = PathBuf::from("/r/a.rs");
        let id = store.draft(file.clone(), DiffSide::Working, 7, "why?".into());
        store.take_draft(id);
        store.push_message(id, Role::Agent, "because X".into());
        store.set_draft(id, "and Y?".into());

        assert_eq!(store.remove_entry(id, 2), Some(2));
        let thread = store.get(id).unwrap();
        assert_eq!(thread.entry_count(), 2);
        assert!(thread.draft.is_none());
        assert!(
            !thread.rewound,
            "the agent never saw the draft, so there is nothing to tell"
        );
        assert_eq!(store.thread_at(&file, DiffSide::Working, 7), Some(id));
    }
}
