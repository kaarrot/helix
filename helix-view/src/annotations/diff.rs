use helix_core::text_annotations::LineAnnotation;
use helix_core::{
    doc_formatter::{DocumentFormatter, TextFormat},
    text_annotations::TextAnnotations,
    Position, Rope,
};
use helix_vcs::DiffHandle;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
};

pub type AlignmentPadding = Rc<RefCell<Vec<(usize, usize)>>>;

/// LineAnnotation that inserts virtual empty rows on the "short" side of a
/// side-by-side diff so the two panes stay visually aligned hunk-by-hunk.
pub struct DiffAlignment {
    diff_handle: DiffHandle,
    last_emitted_doc_line: Cell<Option<usize>>,
    padding_rows_by_doc_line: AlignmentPadding,
    wrapping: Option<(Rope, TextFormat, TextFormat, usize)>,
}

impl DiffAlignment {
    pub fn new(diff_handle: DiffHandle) -> Self {
        Self {
            diff_handle,
            last_emitted_doc_line: Cell::new(None),
            padding_rows_by_doc_line: Rc::default(),
            wrapping: None,
        }
    }

    pub fn padding_rows(&self) -> AlignmentPadding {
        self.padding_rows_by_doc_line.clone()
    }

    pub fn with_wrapping(
        mut self,
        text: Rope,
        format: TextFormat,
        peer_format: TextFormat,
        height: usize,
    ) -> Self {
        if format.soft_wrap || peer_format.soft_wrap {
            self.wrapping = Some((text, format, peer_format, height));
        }
        self
    }
}

impl LineAnnotation for DiffAlignment {
    fn reset_pos(&mut self, char_idx: usize) -> usize {
        self.last_emitted_doc_line.set(None);
        let diff = self.diff_handle.load();
        let mut padding = BTreeMap::<usize, usize>::new();

        if let Some((text, format, peer_format, height)) = &self.wrapping {
            // Only measure lines that can appear in this viewport. Counting
            // the whole file on every render makes wrapped large diffs slow.
            let first = text
                .char_to_line(char_idx.min(text.len_chars()))
                .saturating_sub(1);
            let end = first
                .saturating_add(*height)
                .saturating_add(3)
                .min(text.len_lines());
            let peer = diff.diff_base();
            for line in first..end {
                let mut peer_line = Some(line);
                for index in 0..diff.len() {
                    let hunk = diff.nth_hunk(index);
                    if line < hunk.after.start as usize {
                        break;
                    }
                    if line < hunk.after.end as usize {
                        let offset = line - hunk.after.start as usize;
                        peer_line = (offset < hunk.before.len() as usize)
                            .then_some(hunk.before.start as usize + offset);
                        break;
                    }
                    peer_line = Some(
                        line.saturating_add(hunk.before.end as usize)
                            .saturating_sub(hunk.after.end as usize),
                    );
                }
                if let Some(peer_line) = peer_line.filter(|&line| line < peer.len_lines()) {
                    let deficit = line_height(peer, peer_line, peer_format)
                        .saturating_sub(line_height(text, line, format));
                    if deficit > 0 {
                        *padding.entry(line).or_default() += deficit;
                    }
                }
            }
            for index in 0..diff.len() {
                let hunk = diff.nth_hunk(index);
                let missing = hunk.before.start as usize + hunk.after.len() as usize;
                let emit_after = (hunk.after.end as usize).saturating_sub(1);
                if missing < hunk.before.end as usize && (first..end).contains(&emit_after) {
                    *padding.entry(emit_after).or_default() += (missing..hunk.before.end as usize)
                        .map(|line| line_height(peer, line, peer_format))
                        .sum::<usize>();
                }
            }
            *self.padding_rows_by_doc_line.borrow_mut() = padding.into_iter().collect();
            return usize::MAX;
        }

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

fn line_height(text: &Rope, line: usize, format: &TextFormat) -> usize {
    if !format.soft_wrap {
        return 1;
    }
    let annotations = TextAnnotations::default();
    DocumentFormatter::new_at_prev_checkpoint(
        text.slice(..),
        format,
        &annotations,
        text.line_to_char(line),
    )
    .take_while(|grapheme| grapheme.line_idx == line)
    .map(|grapheme| grapheme.visual_pos.row + 1)
    .max()
    .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn wrapped_replacements_and_unequal_widths_align_following_text() {
        let before = Rope::from("a long unchanged line that wraps differently\nshort\ntail\n");
        let after = Rope::from("a long unchanged line that wraps differently\na long changed line that wraps over several rows\ntail\n");
        let handle = DiffHandle::new(before.clone(), after.clone());
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while handle.load().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let format = TextFormat {
            soft_wrap: true,
            viewport_width: 18,
            ..TextFormat::default()
        };
        let peer_format = TextFormat {
            viewport_width: 19,
            ..format.clone()
        };
        let mut reverse = handle.clone();
        reverse.invert();
        let mut before_annotations = TextAnnotations::default();
        before_annotations.add_line_annotation(Box::new(
            DiffAlignment::new(reverse).with_wrapping(
                before.clone(),
                peer_format.clone(),
                format.clone(),
                100,
            ),
        ));
        let mut after_annotations = TextAnnotations::default();
        after_annotations.add_line_annotation(Box::new(DiffAlignment::new(handle).with_wrapping(
            after.clone(),
            format.clone(),
            peer_format.clone(),
            100,
        )));
        let before_row = DocumentFormatter::new_at_prev_checkpoint(
            before.slice(..),
            &peer_format,
            &before_annotations,
            0,
        )
        .find(|grapheme| grapheme.line_idx == 2)
        .unwrap()
        .visual_pos
        .row;
        let after_row = DocumentFormatter::new_at_prev_checkpoint(
            after.slice(..),
            &format,
            &after_annotations,
            0,
        )
        .find(|grapheme| grapheme.line_idx == 2)
        .unwrap()
        .visual_pos
        .row;
        assert_eq!(before_row, after_row);
    }
}
