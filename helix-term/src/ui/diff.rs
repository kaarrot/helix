use crate::ui::{
    document::{LinePos, TextRenderer},
    text_decorations::Decoration,
};
use helix_core::{syntax::OverlayHighlights, Position};
use helix_view::{
    graphics::{Color, Rect, Style},
    Document, Theme,
};
use similar::{Algorithm, ChangeTag, DiffOp, TextDiff};
use std::{ops::Range, time::Duration};

const MAX_INTRALINE_HUNK_LINES: usize = 40;
const MAX_INTRALINE_HUNK_CHARS: usize = 8 * 1024;
const MAX_INTRALINE_LINE_CHARS: usize = 512;
const MAX_CHAR_DIFF_REPLACE_LINES: usize = 8;
const MAX_LENGTH_RATIO: f32 = 3.0;
const MIN_CHAR_DIFF_RATIO: f32 = 0.55;
const MIN_WORD_DIFF_RATIO: f32 = 0.45;
const MIN_LINE_PAIR_RATIO: f32 = 0.50;
const MAX_REPLACEMENT_HIGHLIGHT_DENSITY: f32 = 0.60;
const INLINE_DIFF_TIMEOUT: Duration = Duration::from_millis(4);

#[derive(Debug, PartialEq, Eq)]
struct IntralineRange {
    line: usize,
    chars: Range<usize>,
}

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
    viewport_anchor: usize,
    viewport_height: usize,
    overlay_highlights: &mut Vec<OverlayHighlights>,
) {
    if !doc.char_diff_enabled {
        return;
    }

    let Some(diff_handle) = doc.diff_handle() else {
        return;
    };

    let diff = diff_handle.load();
    let doc_text = doc.text();
    let diff_base = diff.diff_base();

    let highlight_name = if doc.char_diff_minus_side {
        "diff.minus"
    } else {
        "diff.plus"
    };
    let Some(diff_highlight) = theme.find_highlight_exact(highlight_name) else {
        return;
    };

    let mut add_ranges = Vec::new();
    let first_visible_line = doc_text.char_to_line(viewport_anchor.min(doc_text.len_chars()));
    let last_visible_line = first_visible_line
        .saturating_add(viewport_height)
        .saturating_add(1)
        .min(doc_text.len_lines());

    let hunk_count = diff.len();
    for hunk_idx in 0..hunk_count {
        let hunk = diff.nth_hunk(hunk_idx);
        if hunk == helix_vcs::Hunk::NONE {
            continue;
        }

        let start_line = hunk.after.start as usize;
        let end_line = hunk.after.end as usize;
        if end_line <= first_visible_line || start_line > last_visible_line {
            continue;
        }

        if hunk.before.len() as usize + hunk.after.len() as usize > MAX_INTRALINE_HUNK_LINES {
            continue;
        }

        let mut budget = MAX_INTRALINE_HUNK_CHARS;
        let Some(base_hunk) = collect_hunk_text(diff_base, hunk.before.clone(), &mut budget) else {
            continue;
        };
        let Some(doc_hunk) = collect_hunk_text(doc_text, hunk.after.clone(), &mut budget) else {
            continue;
        };

        for range in adaptive_intraline_ranges(&base_hunk, &doc_hunk) {
            let line_idx = start_line + range.line;
            if line_idx < first_visible_line
                || line_idx >= last_visible_line
                || line_idx >= doc_text.len_lines()
            {
                continue;
            }

            let line_len = doc_text.line(line_idx).len_chars();
            if range.chars.start >= line_len {
                continue;
            }
            let line_start_char = doc_text.line_to_char(line_idx);
            add_ranges.push(
                line_start_char + range.chars.start
                    ..line_start_char + range.chars.end.min(line_len),
            );
        }
    }

    if !add_ranges.is_empty() {
        overlay_highlights.push(OverlayHighlights::Homogeneous {
            highlight: diff_highlight,
            ranges: add_ranges,
        });
    }
}

fn collect_hunk_text(
    text: &helix_core::Rope,
    lines: Range<u32>,
    budget: &mut usize,
) -> Option<String> {
    let mut hunk = String::new();
    for line_idx in lines {
        let line_idx = line_idx as usize;
        if line_idx >= text.len_lines() {
            break;
        }

        let line = text.line(line_idx);
        let line_len = line.len_chars();
        *budget = budget.checked_sub(line_len)?;
        for chunk in line.chunks() {
            hunk.push_str(chunk);
        }
    }
    Some(hunk)
}

fn adaptive_intraline_ranges(before: &str, after: &str) -> Vec<IntralineRange> {
    let before_chars = before.chars().count();
    let after_chars = after.chars().count();
    if before_chars + after_chars > MAX_INTRALINE_HUNK_CHARS {
        return Vec::new();
    }
    if before_chars != 0 && after_chars != 0 && !length_ratio_ok(before_chars, after_chars) {
        return Vec::new();
    }

    let mut config = TextDiff::configure();
    config
        .algorithm(Algorithm::Patience)
        .timeout(INLINE_DIFF_TIMEOUT);
    let line_diff = config.diff_lines(before, after);

    if line_diff.old_slices().len() + line_diff.new_slices().len() > MAX_INTRALINE_HUNK_LINES {
        return Vec::new();
    }

    let mut ranges = Vec::new();
    for op in line_diff.ops() {
        match *op {
            DiffOp::Equal { .. } | DiffOp::Delete { .. } => {}
            DiffOp::Insert {
                new_index, new_len, ..
            } => push_inserted_line_ranges(&line_diff, new_index, new_len, &mut ranges),
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => push_replacement_ranges(
                &line_diff,
                old_index,
                old_len,
                new_index,
                new_len,
                &mut ranges,
            ),
        }
    }

    ranges
}

fn push_inserted_line_ranges(
    line_diff: &TextDiff<'_, '_, '_, str>,
    new_index: usize,
    new_len: usize,
    ranges: &mut Vec<IntralineRange>,
) {
    for line in new_index..new_index + new_len {
        push_inserted_line_range(line_diff.new_slices()[line], line, ranges);
    }
}

fn push_inserted_line_range(after: &str, line: usize, ranges: &mut Vec<IntralineRange>) {
    let line_chars = after.chars().count();
    if line_chars == 0 || line_chars > MAX_INTRALINE_LINE_CHARS || after.trim().is_empty() {
        return;
    }
    ranges.push(IntralineRange {
        line,
        chars: 0..line_chars,
    });
}

fn push_replacement_ranges(
    line_diff: &TextDiff<'_, '_, '_, str>,
    old_index: usize,
    old_len: usize,
    new_index: usize,
    new_len: usize,
    ranges: &mut Vec<IntralineRange>,
) {
    if old_len == new_len && old_len <= MAX_CHAR_DIFF_REPLACE_LINES {
        let ranges_start = ranges.len();
        for offset in 0..new_len {
            push_paired_line_ranges(
                line_diff.old_slices()[old_index + offset],
                line_diff.new_slices()[new_index + offset],
                new_index + offset,
                ranges,
            );
        }

        if ranges.len() != ranges_start {
            return;
        }
    }

    push_matched_replacement_ranges(line_diff, old_index, old_len, new_index, new_len, ranges);
}

fn push_matched_replacement_ranges(
    line_diff: &TextDiff<'_, '_, '_, str>,
    old_index: usize,
    old_len: usize,
    new_index: usize,
    new_len: usize,
    ranges: &mut Vec<IntralineRange>,
) {
    let mut used_old_lines = vec![false; old_len];
    let mut line_matches = vec![None; new_len];

    for new_offset in 0..new_len {
        let after = line_diff.new_slices()[new_index + new_offset];
        let Some(old_offset) =
            best_line_match(line_diff, old_index, old_len, after, &used_old_lines)
        else {
            continue;
        };

        used_old_lines[old_offset] = true;
        line_matches[new_offset] = Some(old_offset);
    }

    if line_matches.iter().all(Option::is_none) {
        return;
    }

    for (new_offset, old_offset) in line_matches.into_iter().enumerate() {
        let after = line_diff.new_slices()[new_index + new_offset];
        if let Some(old_offset) = old_offset {
            push_paired_line_ranges(
                line_diff.old_slices()[old_index + old_offset],
                after,
                new_index + new_offset,
                ranges,
            );
        } else {
            push_inserted_line_range(after, new_index + new_offset, ranges);
        }
    }
}

fn best_line_match(
    line_diff: &TextDiff<'_, '_, '_, str>,
    old_index: usize,
    old_len: usize,
    after: &str,
    used_old_lines: &[bool],
) -> Option<usize> {
    let mut best = None;
    for old_offset in 0..old_len {
        if used_old_lines[old_offset] {
            continue;
        }

        let ratio = line_similarity(line_diff.old_slices()[old_index + old_offset], after);
        if ratio >= MIN_LINE_PAIR_RATIO && best.is_none_or(|(_, best_ratio)| ratio > best_ratio) {
            best = Some((old_offset, ratio));
        }
    }

    best.map(|(old_offset, _)| old_offset)
}

fn line_similarity(before: &str, after: &str) -> f32 {
    let before_chars = before.chars().count();
    let after_chars = after.chars().count();
    if before_chars == 0 || after_chars == 0 {
        return 0.0;
    }
    if before_chars > MAX_INTRALINE_LINE_CHARS
        || after_chars > MAX_INTRALINE_LINE_CHARS
        || !length_ratio_ok(before_chars, after_chars)
    {
        return 0.0;
    }

    let mut config = TextDiff::configure();
    config.timeout(INLINE_DIFF_TIMEOUT);
    config.diff_chars(before, after).ratio()
}

fn push_paired_line_ranges(
    before: &str,
    after: &str,
    line: usize,
    ranges: &mut Vec<IntralineRange>,
) {
    let ranges_start = ranges.len();
    push_char_diff_line_ranges(before, after, line, ranges);
    if ranges.len() == ranges_start {
        push_word_diff_line_ranges(before, after, line, ranges);
    }
}

fn push_char_diff_line_ranges(
    before: &str,
    after: &str,
    line: usize,
    ranges: &mut Vec<IntralineRange>,
) {
    let before_chars = before.chars().count();
    let after_chars = after.chars().count();
    if after_chars == 0
        || before_chars > MAX_INTRALINE_LINE_CHARS
        || after_chars > MAX_INTRALINE_LINE_CHARS
        || (before_chars != 0 && !length_ratio_ok(before_chars, after_chars))
    {
        return;
    }

    let mut config = TextDiff::configure();
    config.timeout(INLINE_DIFF_TIMEOUT);
    let char_diff = config.diff_chars(before, after);
    if char_diff.ratio() < MIN_CHAR_DIFF_RATIO {
        return;
    }

    let ranges_start = ranges.len();
    let mut after_idx = 0;
    let mut highlighted_chars = 0;
    for change in char_diff.iter_all_changes() {
        let char_count = change.value().chars().count();
        match change.tag() {
            ChangeTag::Equal => after_idx += char_count,
            ChangeTag::Insert => {
                if char_count != 0 {
                    ranges.push(IntralineRange {
                        line,
                        chars: after_idx..after_idx + char_count,
                    });
                    highlighted_chars += char_count;
                    after_idx += char_count;
                }
            }
            ChangeTag::Delete => {}
        }
    }

    if highlighted_chars == 0 || too_dense(highlighted_chars, after_chars) {
        ranges.truncate(ranges_start);
    }
}

fn push_word_diff_line_ranges(
    before: &str,
    after: &str,
    line: usize,
    ranges: &mut Vec<IntralineRange>,
) {
    let before_chars = before.chars().count();
    let after_chars = after.chars().count();
    if after_chars == 0
        || before_chars > MAX_INTRALINE_LINE_CHARS
        || after_chars > MAX_INTRALINE_LINE_CHARS
        || (before_chars != 0 && !length_ratio_ok(before_chars, after_chars))
    {
        return;
    }

    let mut config = TextDiff::configure();
    config
        .algorithm(Algorithm::Patience)
        .timeout(INLINE_DIFF_TIMEOUT);
    let word_diff = config.diff_words(before, after);
    if word_diff.ratio() < MIN_WORD_DIFF_RATIO {
        return;
    }

    let ranges_start = ranges.len();
    let mut after_idx = 0;
    let mut highlighted_chars = 0;
    for change in word_diff.iter_all_changes() {
        let char_count = change.value().chars().count();
        match change.tag() {
            ChangeTag::Equal => after_idx += char_count,
            ChangeTag::Insert => {
                if char_count != 0 {
                    ranges.push(IntralineRange {
                        line,
                        chars: after_idx..after_idx + char_count,
                    });
                    highlighted_chars += char_count;
                    after_idx += char_count;
                }
            }
            ChangeTag::Delete => {}
        }
    }

    if highlighted_chars == 0 || too_dense(highlighted_chars, after_chars) {
        ranges.truncate(ranges_start);
    }
}

fn length_ratio_ok(a: usize, b: usize) -> bool {
    let shorter = a.min(b);
    let longer = a.max(b);
    shorter == 0 || longer as f32 / shorter as f32 <= MAX_LENGTH_RATIO
}

fn too_dense(highlighted_chars: usize, line_chars: usize) -> bool {
    line_chars != 0
        && highlighted_chars as f32 / line_chars as f32 > MAX_REPLACEMENT_HIGHLIGHT_DENSITY
}

#[cfg(test)]
mod tests {
    use super::{adaptive_intraline_ranges, IntralineRange};

    fn range(line: usize, chars: std::ops::Range<usize>) -> IntralineRange {
        IntralineRange { line, chars }
    }

    #[test]
    fn small_one_line_edit_uses_character_ranges() {
        assert_eq!(
            adaptive_intraline_ranges("let value = 1;\n", "let value = 2;\n"),
            vec![range(0, 12..13)]
        );
    }

    #[test]
    fn inserted_lines_do_not_shift_later_character_pairs() {
        assert_eq!(
            adaptive_intraline_ranges(
                "same\nlet value = 1;\nnext\n",
                "same\ninserted\nlet value = 2;\nnext\n"
            ),
            vec![range(1, 0..9), range(2, 12..13)]
        );
    }

    #[test]
    fn large_hunks_skip_intraline_ranges() {
        let before = (0..24).map(|i| format!("before {i}\n")).collect::<String>();
        let after = (0..24).map(|i| format!("after {i}\n")).collect::<String>();

        assert!(adaptive_intraline_ranges(&before, &after).is_empty());
    }

    #[test]
    fn dense_rewrites_skip_intraline_ranges() {
        assert!(adaptive_intraline_ranges(
            "aaaaaaaaaaaaaaaaaaaaaaaa\n",
            "bbbbbbbbbbbbbbbbbbbbbbbb\n"
        )
        .is_empty());
    }
}
