use helix_core::text_annotations::LineAnnotation;
use helix_core::Position;
use helix_vcs::DiffHandle;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
};

/// LineAnnotation that inserts virtual empty rows on the "short" side of a
/// side-by-side diff so the two panes stay visually aligned hunk-by-hunk.
pub struct DiffAlignment {
    diff_handle: DiffHandle,
    last_emitted_doc_line: Cell<Option<usize>>,
    padding_rows_by_doc_line: RefCell<Vec<(usize, usize)>>,
}

impl DiffAlignment {
    pub fn new(diff_handle: DiffHandle) -> Self {
        Self {
            diff_handle,
            last_emitted_doc_line: Cell::new(None),
            padding_rows_by_doc_line: RefCell::new(Vec::new()),
        }
    }
}

impl LineAnnotation for DiffAlignment {
    fn reset_pos(&mut self, _char_idx: usize) -> usize {
        self.last_emitted_doc_line.set(None);
        let diff = self.diff_handle.load();
        let mut padding = BTreeMap::<usize, usize>::new();

        for i in 0..diff.len() {
            let hunk = diff.nth_hunk(i);
            let before_len = hunk.before.len() as usize;
            let after_len = hunk.after.len() as usize;
            if before_len <= after_len {
                continue;
            }
            let deficit = before_len - after_len;
            let start = hunk.after.start as usize;
            let emit_after = start.saturating_sub(1);
            *padding.entry(emit_after).or_insert(0) += deficit;
        }

        *self.padding_rows_by_doc_line.borrow_mut() = padding.into_iter().collect();
        usize::MAX
    }

    fn insert_virtual_lines(
        &mut self,
        _line_end_char_idx: usize,
        _line_end_visual_pos: Position,
        doc_line: usize,
    ) -> Position {
        if self.last_emitted_doc_line.get() == Some(doc_line) {
            return Position::default();
        }

        let extra_rows = self
            .padding_rows_by_doc_line
            .borrow()
            .binary_search_by_key(&doc_line, |(line, _)| *line)
            .ok()
            .map(|idx| self.padding_rows_by_doc_line.borrow()[idx].1)
            .unwrap_or(0);

        if extra_rows > 0 {
            self.last_emitted_doc_line.set(Some(doc_line));
            Position::new(extra_rows, 0)
        } else {
            Position::default()
        }
    }
}
