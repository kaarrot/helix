//! Virtual rows a view reserves below document lines.
//!
//! Diff alignment spacers and inline review comments both want rows on the same
//! lines. They are computed once into a single [`VirtualRowPlan`], which the
//! reserving [`LineAnnotation`] and the painting `Decoration` share by `Rc` and
//! walk as the same slice. Keeping them in agreement matters: the formatter lays
//! text out around the reserved count, so a decoration that paints more rows
//! than were reserved overwrites real text, and the damage shows up wherever the
//! rows are rather than where the mistake was made.

use helix_core::text_annotations::LineAnnotation;
use helix_core::Position;
use helix_vcs::DiffHandle;
use std::{cell::Cell, collections::BTreeMap, rc::Rc};

use crate::review::ThreadId;

/// How much attention a thread currently has.
///
/// Two levels, because the cursor merely passing a line is a weaker statement
/// than stopping on its box. Only the stronger one takes the keys that walk a
/// thread's history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attention {
    /// Elsewhere in the buffer.
    Idle,
    /// The cursor is on the line this thread is anchored to.
    UnderCursor,
    /// The cursor has stopped on the box itself.
    Focused,
}

/// How a comment row should be painted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentRowKind {
    /// The single row a collapsed thread occupies.
    Summary,
    /// Text the user wrote.
    User,
    /// Text the agent replied.
    Agent,
    /// A draft that has not been sent yet.
    Pending,
    /// The anchored line is gone.
    Orphaned,
}

/// What the in-box cursor and its selection do to one row.
///
/// The cursor cannot land in virtual rows, so a box that wants a selection has
/// to draw its own. Only a focused box marks anything: an idle one has no
/// cursor to show.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RowMark {
    #[default]
    None,
    /// Within the selection being made inside the box.
    Selected,
    /// The row the in-box cursor is on.
    Cursor,
}

/// One virtual row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VirtualRow {
    /// Alignment padding on the short side of a side-by-side diff.
    Spacer,
    /// Space held for a comment box that is currently being typed into. The
    /// input draws over these, so they only have to exist and be the right
    /// number for the code below to sit in the right place.
    Composing,
    /// One screen row of a review thread, already wrapped to the pane width.
    Comment {
        thread: ThreadId,
        kind: CommentRowKind,
        text: String,
        attention: Attention,
        mark: RowMark,
        /// Which wrapped line of the entry this row shows, if it shows one at
        /// all: a header or a collapsed summary is not part of the body. This
        /// is what the mouse lands on, so it has to come from the same place
        /// the text did.
        body: Option<usize>,
    },
}

/// Every virtual row a view wants, computed once per frame.
#[derive(Debug, Default)]
pub struct VirtualRowPlan {
    /// Sorted by document line. Rows are emitted after their line, in slice order.
    rows: Vec<(usize, Vec<VirtualRow>)>,
}

impl VirtualRowPlan {
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The rows to emit after `doc_line`, in render order.
    pub fn rows_at(&self, doc_line: usize) -> &[VirtualRow] {
        self.rows
            .binary_search_by_key(&doc_line, |(line, _)| *line)
            .map(|idx| self.rows[idx].1.as_slice())
            .unwrap_or_default()
    }

    /// Document lines carrying at least one comment row, for gutter marking and
    /// for `]c`/`[c` navigation.
    pub fn comment_lines(&self) -> impl Iterator<Item = usize> + '_ {
        self.rows.iter().filter_map(|(line, rows)| {
            rows.iter()
                .any(|row| matches!(row, VirtualRow::Comment { .. }))
                .then_some(*line)
        })
    }

    /// The thread whose rows sit below `doc_line`, if any.
    pub fn thread_at(&self, doc_line: usize) -> Option<ThreadId> {
        self.rows_at(doc_line).iter().find_map(|row| match row {
            VirtualRow::Comment { thread, .. } => Some(*thread),
            VirtualRow::Spacer | VirtualRow::Composing => None,
        })
    }
}

/// Accumulates rows while a plan is being assembled from its several sources.
#[derive(Default)]
pub struct VirtualRowPlanBuilder {
    by_line: BTreeMap<usize, Vec<VirtualRow>>,
}

impl VirtualRowPlanBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Alignment padding for a side-by-side diff, from the hunks of `diff_handle`.
    ///
    /// Emitted after the line *preceding* the hunk, so padding sits above the
    /// hunk's first line rather than below it.
    pub fn add_diff_spacers(&mut self, diff_handle: &DiffHandle) {
        let diff = diff_handle.load();
        for i in 0..diff.len() {
            let hunk = diff.nth_hunk(i);
            let before_len = hunk.before.len() as usize;
            let after_len = hunk.after.len() as usize;
            if before_len <= after_len {
                continue;
            }
            let deficit = before_len - after_len;
            let emit_after = (hunk.after.start as usize).saturating_sub(1);
            self.by_line
                .entry(emit_after)
                .or_default()
                .extend(std::iter::repeat(VirtualRow::Spacer).take(deficit));
        }
    }

    pub fn add_comment_rows(&mut self, doc_line: usize, rows: Vec<VirtualRow>) {
        if rows.is_empty() {
            return;
        }
        self.by_line.entry(doc_line).or_default().extend(rows);
    }

    pub fn build(self) -> VirtualRowPlan {
        VirtualRowPlan {
            rows: self.by_line.into_iter().collect(),
        }
    }
}

/// Reserves the rows described by a [`VirtualRowPlan`].
pub struct VirtualRowLines {
    plan: Rc<VirtualRowPlan>,
    last_emitted_doc_line: Cell<Option<usize>>,
}

impl VirtualRowLines {
    pub fn new(plan: Rc<VirtualRowPlan>) -> Self {
        Self {
            plan,
            last_emitted_doc_line: Cell::new(None),
        }
    }
}

impl LineAnnotation for VirtualRowLines {
    fn reset_pos(&mut self, _char_idx: usize) -> usize {
        self.last_emitted_doc_line.set(None);
        usize::MAX
    }

    fn insert_virtual_lines(
        &mut self,
        _line_end_char_idx: usize,
        _line_end_visual_pos: Position,
        doc_line: usize,
    ) -> Position {
        // Called at the end of every visual line, so a soft-wrapped document
        // line asks several times. Emit on the first ask only; the painting
        // decoration's `first_visual_line` guard is the other half of this.
        if self.last_emitted_doc_line.get() == Some(doc_line) {
            return Position::default();
        }

        let rows = self.plan.rows_at(doc_line).len();
        if rows > 0 {
            self.last_emitted_doc_line.set(Some(doc_line));
            Position::new(rows, 0)
        } else {
            Position::default()
        }
    }
}

/// Greedy wrap of plain comment text to `width` columns.
///
/// Deliberately simple: comment bodies are prose, and running them through the
/// document formatter would mean building a `TextFormat` and a rope slice per
/// row for no visible gain. Always returns at least one line so an empty
/// message still occupies the row it reserved.
pub fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    let mut lines = Vec::new();

    for raw_line in text.split('\n') {
        let mut current = String::new();
        for word in raw_line.split_whitespace() {
            if current.is_empty() {
                current.push_str(word);
            } else if current.chars().count() + 1 + word.chars().count() <= width {
                current.push(' ');
                current.push_str(word);
            } else {
                lines.push(std::mem::take(&mut current));
                current.push_str(word);
            }
            // A single word longer than the line has to be broken somewhere.
            while current.chars().count() > width {
                let head: String = current.chars().take(width).collect();
                let tail: String = current.chars().skip(width).collect();
                lines.push(head);
                current = tail;
            }
        }
        lines.push(current);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn wrap_breaks_on_words() {
        assert_eq!(
            wrap_text("the quick brown fox jumps", 10),
            vec!["the quick", "brown fox", "jumps"]
        );
    }

    #[test]
    fn wrap_splits_a_word_too_long_to_fit() {
        assert_eq!(
            wrap_text("supercalifragilistic", 8),
            vec!["supercal", "ifragili", "stic"]
        );
    }

    #[test]
    fn wrap_keeps_explicit_newlines_and_never_returns_nothing() {
        assert_eq!(wrap_text("one\ntwo", 20), vec!["one", "two"]);
        assert_eq!(wrap_text("", 20), vec![""]);
    }

    #[test]
    fn plan_reports_rows_and_comment_lines() {
        let mut builder = VirtualRowPlanBuilder::new();
        builder.add_comment_rows(
            4,
            vec![VirtualRow::Comment {
                thread: ThreadId(1),
                kind: CommentRowKind::User,
                text: "why?".into(),
                attention: Attention::Idle,
                mark: RowMark::None,
                body: Some(0),
            }],
        );
        builder.add_comment_rows(9, vec![VirtualRow::Spacer]);
        let plan = builder.build();

        assert_eq!(plan.rows_at(4).len(), 1);
        assert_eq!(plan.rows_at(9).len(), 1);
        assert_eq!(plan.rows_at(5).len(), 0);
        assert_eq!(plan.comment_lines().collect::<Vec<_>>(), vec![4]);
        assert_eq!(plan.thread_at(4), Some(ThreadId(1)));
        assert_eq!(plan.thread_at(9), None);
    }
}
