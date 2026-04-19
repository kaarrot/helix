use crate::ui::{
    document::{LinePos, TextRenderer},
    text_decorations::Decoration,
};
use helix_core::{syntax::OverlayHighlights, Position};
use helix_view::{
    graphics::{Color, Rect, Style},
    Document, Theme,
};

/// Paints a tinted background on virtual spacer lines produced by `DiffAlignment`
/// so the "no content here" regions are visually distinct from real lines.
pub(super) struct DiffSpacerDecoration {
    spacer_rows: Vec<(usize, usize)>,
    style: Style,
}

impl DiffSpacerDecoration {
    pub(super) fn new(diff_handle: &helix_vcs::DiffHandle, theme: &Theme) -> Self {
        let diff = diff_handle.load();
        let mut padding = std::collections::BTreeMap::<usize, usize>::new();
        for i in 0..diff.len() {
            let hunk = diff.nth_hunk(i);
            let before_len = hunk.before.len() as usize;
            let after_len = hunk.after.len() as usize;
            if before_len > after_len {
                let deficit = before_len - after_len;
                let doc_line = hunk.after.start as usize;
                let emit_after = doc_line.saturating_sub(1);
                *padding.entry(emit_after).or_insert(0) += deficit;
            }
        }
        let style = theme.try_get("ui.diff.spacer").unwrap_or_else(|| {
            let bg_style = theme.get("ui.background");
            match bg_style.bg {
                Some(Color::Rgb(br, bg, bb)) => {
                    let lum = 0.299 * br as f64 + 0.587 * bg as f64 + 0.114 * bb as f64;
                    let (r, g, b) = if lum < 128.0 {
                        (
                            (br as u16 + 15).min(255) as u8,
                            (bg as u16 + 15).min(255) as u8,
                            (bb as u16 + 15).min(255) as u8,
                        )
                    } else {
                        (
                            br.saturating_sub(15),
                            bg.saturating_sub(15),
                            bb.saturating_sub(15),
                        )
                    };
                    Style::default().bg(Color::Rgb(r, g, b))
                }
                _ => Style::default().bg(Color::Gray),
            }
        });
        Self {
            spacer_rows: padding.into_iter().collect(),
            style,
        }
    }
}

impl Decoration for DiffSpacerDecoration {
    fn render_virt_lines(
        &mut self,
        renderer: &mut TextRenderer,
        pos: LinePos,
        virt_off: Position,
    ) -> Position {
        let rows = self
            .spacer_rows
            .binary_search_by_key(&pos.doc_line, |(line, _)| *line)
            .ok()
            .map(|idx| self.spacer_rows[idx].1)
            .unwrap_or(0);

        if rows > 0 {
            for i in 0..rows {
                let y = pos.visual_line + virt_off.row as u16 + i as u16;
                if y >= renderer.offset.row as u16 + renderer.viewport.height {
                    break;
                }
                renderer.set_style(
                    Rect::new(renderer.viewport.x, y, renderer.viewport.width, 1),
                    self.style,
                );
            }
            Position::new(rows, 0)
        } else {
            Position::default()
        }
    }
}

/// Append character-level diff highlight spans to `overlay_highlights`.
pub(super) fn char_diff_highlights_into(
    doc: &Document,
    theme: &Theme,
    overlay_highlights: &mut Vec<OverlayHighlights>,
) {
    use similar::TextDiff;

    if !doc.char_diff_enabled {
        return;
    }

    let Some(diff_handle) = doc.diff_handle() else {
        return;
    };

    let diff = diff_handle.load();
    let doc_text = doc.text();
    let diff_base = diff.diff_base();

    let Some(add_highlight) = theme.find_highlight_exact("diff.plus") else {
        return;
    };

    let mut add_ranges = Vec::new();

    let hunk_count = diff.len();
    for hunk_idx in 0..hunk_count {
        let hunk = diff.nth_hunk(hunk_idx);
        if hunk == helix_vcs::Hunk::NONE {
            continue;
        }

        let start_line = hunk.after.start as usize;
        let end_line = hunk.after.end as usize;

        for line_idx in start_line..end_line {
            if line_idx >= doc_text.len_lines() {
                break;
            }

            let doc_line = doc_text.line(line_idx);
            let doc_line_str = doc_line.to_string();
            let line_start_char = doc_text.line_to_char(line_idx);

            let line_offset_in_hunk = line_idx - hunk.after.start as usize;
            let base_line_idx = hunk.before.start as usize + line_offset_in_hunk;

            if hunk.is_pure_insertion() {
                if !doc_line_str.trim().is_empty() {
                    let char_count = doc_line_str.chars().count();
                    add_ranges.push(line_start_char..line_start_char + char_count);
                }
                continue;
            }

            if base_line_idx >= diff_base.len_lines() {
                continue;
            }

            let base_line = diff_base.line(base_line_idx);
            let base_line_str = base_line.to_string();

            if doc_line_str == base_line_str {
                continue;
            }

            let char_diff = TextDiff::from_chars(&base_line_str, &doc_line_str);

            let mut doc_idx = 0;
            for change in char_diff.iter_all_changes() {
                let value = change.value();
                let char_count = value.chars().count();

                match change.tag() {
                    similar::ChangeTag::Equal => {
                        doc_idx += char_count;
                    }
                    similar::ChangeTag::Insert => {
                        let start = line_start_char + doc_idx;
                        let end = start + char_count;
                        add_ranges.push(start..end);
                        doc_idx += char_count;
                    }
                    similar::ChangeTag::Delete => {}
                }
            }
        }
    }

    if !add_ranges.is_empty() {
        overlay_highlights.push(OverlayHighlights::Homogeneous {
            highlight: add_highlight,
            ranges: add_ranges,
        });
    }
}
