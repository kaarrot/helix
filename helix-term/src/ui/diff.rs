use crate::ui::{
    document::{LinePos, TextRenderer},
    text_decorations::Decoration,
};
use helix_core::{syntax::OverlayHighlights, Position};
use helix_view::{
    graphics::{Color, Rect, Style},
    Document, Theme,
};
use similar::{
    algorithms::{diff_deadline, Capture, IdentifyDistinct, Replace},
    get_diff_ratio, Algorithm, DiffOp, DiffableStr,
};
use std::{
    hash::Hash,
    ops::Range,
    time::{Duration, Instant},
};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntralineSide {
    Before,
    After,
}

#[derive(Debug, PartialEq, Eq)]
struct IntralineRange {
    line: usize,
    chars: Range<usize>,
}

/// Paints a tinted background on virtual spacer lines produced by `DiffAlignment`
/// so the "no content here" regions are visually distinct from real lines.
pub(super) struct DiffSpacerDecoration {
    spacer_rows: helix_view::annotations::diff::AlignmentPadding,
    style: Style,
}

impl DiffSpacerDecoration {
    pub(super) fn new(
        padding: helix_view::annotations::diff::AlignmentPadding,
        theme: &Theme,
    ) -> Self {
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
            spacer_rows: padding,
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
        let padding = self.spacer_rows.borrow();
        let rows = padding
            .binary_search_by_key(&pos.doc_line, |(line, _)| *line)
            .ok()
            .map(|idx| padding[idx].1)
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
    view_id: helix_view::ViewId,
    theme: &Theme,
    viewport_anchor: usize,
    viewport_height: usize,
    overlay_highlights: &mut Vec<OverlayHighlights>,
) {
    if !doc.char_diff_enabled_for_view(view_id) {
        return;
    }

    let Some(diff_handle) = doc.diff_handle_for_view(view_id) else {
        return;
    };

    let diff = diff_handle.load();
    let doc_text = doc.text();
    let diff_base = diff.diff_base();

    let highlight_name = if diff.is_inverted() {
        "diff.minus.intraline"
    } else {
        "diff.plus.intraline"
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

        let (before_hunk, after_hunk, side) = if diff.is_inverted() {
            (&doc_hunk, &base_hunk, IntralineSide::Before)
        } else {
            (&base_hunk, &doc_hunk, IntralineSide::After)
        };

        for range in adaptive_intraline_ranges(before_hunk, after_hunk, side) {
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

/// Diff two slices without `similar::algorithms::Compact`, which shifts
/// `Insert` ops through following `Equal` runs and can split adjacent
/// `Delete`/`Insert` replacements (e.g. highlighting the last `b` in
/// `aaabbb` vs `aaacbb`).
fn diff_slices_without_compact<T>(algorithm: Algorithm, old: &[T], new: &[T]) -> Vec<DiffOp>
where
    T: Eq + Hash + Ord,
{
    let deadline = Instant::now().checked_add(INLINE_DIFF_TIMEOUT);
    let mut hook = Replace::new(Capture::new());
    if old.len() > 100 || new.len() > 100 {
        let ih = IdentifyDistinct::<u32>::new(old, 0..old.len(), new, 0..new.len());
        let _ = diff_deadline(
            algorithm,
            &mut hook,
            ih.old_lookup(),
            ih.old_range(),
            ih.new_lookup(),
            ih.new_range(),
            deadline,
        );
    } else {
        let _ = diff_deadline(
            algorithm,
            &mut hook,
            old,
            0..old.len(),
            new,
            0..new.len(),
            deadline,
        );
    }
    hook.into_inner().into_ops()
}

fn adaptive_intraline_ranges(
    before: &str,
    after: &str,
    side: IntralineSide,
) -> Vec<IntralineRange> {
    let before_chars = before.chars().count();
    let after_chars = after.chars().count();
    if before_chars + after_chars > MAX_INTRALINE_HUNK_CHARS {
        return Vec::new();
    }
    if before_chars != 0 && after_chars != 0 && !length_ratio_ok(before_chars, after_chars) {
        return Vec::new();
    }

    let old_lines = before.tokenize_lines();
    let new_lines = after.tokenize_lines();

    if old_lines.len() + new_lines.len() > MAX_INTRALINE_HUNK_LINES {
        return Vec::new();
    }

    let line_ops = diff_slices_without_compact(Algorithm::Patience, &old_lines, &new_lines);

    let mut ranges = Vec::new();
    for op in line_ops {
        match op {
            DiffOp::Equal { .. } => {}
            DiffOp::Delete {
                old_index, old_len, ..
            } => {
                if side == IntralineSide::Before {
                    push_whole_line_ranges(&old_lines, old_index, old_len, &mut ranges);
                }
            }
            DiffOp::Insert {
                new_index, new_len, ..
            } => {
                if side == IntralineSide::After {
                    push_whole_line_ranges(&new_lines, new_index, new_len, &mut ranges);
                }
            }
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => push_replacement_ranges(
                &old_lines,
                &new_lines,
                old_index,
                old_len,
                new_index,
                new_len,
                side,
                &mut ranges,
            ),
        }
    }

    ranges
}

fn push_whole_line_ranges(
    lines: &[&str],
    start: usize,
    len: usize,
    ranges: &mut Vec<IntralineRange>,
) {
    for line in start..start + len {
        push_whole_line_range(lines[line], line, ranges);
    }
}

fn push_whole_line_range(text: &str, line: usize, ranges: &mut Vec<IntralineRange>) {
    let line_chars = text.chars().count();
    if line_chars == 0 || line_chars > MAX_INTRALINE_LINE_CHARS || text.trim().is_empty() {
        return;
    }
    ranges.push(IntralineRange {
        line,
        chars: 0..line_chars,
    });
}

#[allow(clippy::too_many_arguments)]
fn push_replacement_ranges(
    old_lines: &[&str],
    new_lines: &[&str],
    old_index: usize,
    old_len: usize,
    new_index: usize,
    new_len: usize,
    side: IntralineSide,
    ranges: &mut Vec<IntralineRange>,
) {
    if old_len == new_len && old_len <= MAX_CHAR_DIFF_REPLACE_LINES {
        let ranges_start = ranges.len();
        for offset in 0..new_len {
            let line = match side {
                IntralineSide::Before => old_index + offset,
                IntralineSide::After => new_index + offset,
            };
            push_paired_line_ranges(
                old_lines[old_index + offset],
                new_lines[new_index + offset],
                line,
                side,
                ranges,
            );
        }

        if ranges.len() != ranges_start {
            return;
        }
    }

    push_matched_replacement_ranges(
        old_lines,
        new_lines,
        old_index,
        old_len,
        new_index,
        new_len,
        side,
        ranges,
    );
}

#[allow(clippy::too_many_arguments)]
fn push_matched_replacement_ranges(
    old_lines: &[&str],
    new_lines: &[&str],
    old_index: usize,
    old_len: usize,
    new_index: usize,
    new_len: usize,
    side: IntralineSide,
    ranges: &mut Vec<IntralineRange>,
) {
    let mut used_old_lines = vec![false; old_len];
    let mut line_matches = vec![None; new_len];

    for new_offset in 0..new_len {
        let after = new_lines[new_index + new_offset];
        let Some(old_offset) =
            best_line_match(old_lines, old_index, old_len, after, &used_old_lines)
        else {
            continue;
        };

        used_old_lines[old_offset] = true;
        line_matches[new_offset] = Some(old_offset);
    }

    if line_matches.iter().all(Option::is_none) {
        return;
    }

    match side {
        IntralineSide::After => {
            for (new_offset, old_offset) in line_matches.into_iter().enumerate() {
                let after = new_lines[new_index + new_offset];
                if let Some(old_offset) = old_offset {
                    push_paired_line_ranges(
                        old_lines[old_index + old_offset],
                        after,
                        new_index + new_offset,
                        side,
                        ranges,
                    );
                } else {
                    push_whole_line_range(after, new_index + new_offset, ranges);
                }
            }
        }
        IntralineSide::Before => {
            let mut old_to_new = vec![None; old_len];
            for (new_offset, old_offset) in line_matches.into_iter().enumerate() {
                if let Some(old_offset) = old_offset {
                    old_to_new[old_offset] = Some(new_offset);
                }
            }
            for (old_offset, new_offset) in old_to_new.into_iter().enumerate() {
                let before = old_lines[old_index + old_offset];
                if let Some(new_offset) = new_offset {
                    push_paired_line_ranges(
                        before,
                        new_lines[new_index + new_offset],
                        old_index + old_offset,
                        side,
                        ranges,
                    );
                } else {
                    push_whole_line_range(before, old_index + old_offset, ranges);
                }
            }
        }
    }
}

fn best_line_match(
    old_lines: &[&str],
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

        let ratio = line_similarity(old_lines[old_index + old_offset], after);
        if ratio >= MIN_LINE_PAIR_RATIO && best.is_none_or(|(_, best_ratio)| ratio > best_ratio) {
            best = Some((old_offset, ratio));
        }
    }

    best.map(|(old_offset, _)| old_offset)
}

fn line_similarity(before: &str, after: &str) -> f32 {
    let before_len = before.chars().count();
    let after_len = after.chars().count();
    if before_len == 0 || after_len == 0 {
        return 0.0;
    }
    if before_len > MAX_INTRALINE_LINE_CHARS
        || after_len > MAX_INTRALINE_LINE_CHARS
        || !length_ratio_ok(before_len, after_len)
    {
        return 0.0;
    }

    let before_chars: Vec<char> = before.chars().collect();
    let after_chars: Vec<char> = after.chars().collect();
    let ops = diff_slices_without_compact(Algorithm::Myers, &before_chars, &after_chars);
    get_diff_ratio(&ops, before_chars.len(), after_chars.len())
}

fn push_paired_line_ranges(
    before: &str,
    after: &str,
    line: usize,
    side: IntralineSide,
    ranges: &mut Vec<IntralineRange>,
) {
    let ranges_start = ranges.len();
    push_char_diff_line_ranges(before, after, line, side, ranges);
    if ranges.len() == ranges_start {
        push_word_diff_line_ranges(before, after, line, side, ranges);
    }
}

fn push_char_diff_line_ranges(
    before: &str,
    after: &str,
    line: usize,
    side: IntralineSide,
    ranges: &mut Vec<IntralineRange>,
) {
    let before_len = before.chars().count();
    let after_len = after.chars().count();
    let target_len = match side {
        IntralineSide::Before => before_len,
        IntralineSide::After => after_len,
    };
    if target_len == 0
        || before_len > MAX_INTRALINE_LINE_CHARS
        || after_len > MAX_INTRALINE_LINE_CHARS
        || (before_len != 0 && after_len != 0 && !length_ratio_ok(before_len, after_len))
    {
        return;
    }

    let before_chars: Vec<char> = before.chars().collect();
    let after_chars: Vec<char> = after.chars().collect();
    let ops = diff_slices_without_compact(Algorithm::Myers, &before_chars, &after_chars);
    if get_diff_ratio(&ops, before_chars.len(), after_chars.len()) < MIN_CHAR_DIFF_RATIO {
        return;
    }

    let ranges_start = ranges.len();
    let mut highlighted_chars = 0;
    for op in ops {
        let changed = match (side, op) {
            (
                IntralineSide::Before,
                DiffOp::Delete {
                    old_index, old_len, ..
                }
                | DiffOp::Replace {
                    old_index, old_len, ..
                },
            ) => Some(old_index..old_index + old_len),
            (
                IntralineSide::After,
                DiffOp::Insert {
                    new_index, new_len, ..
                }
                | DiffOp::Replace {
                    new_index, new_len, ..
                },
            ) => Some(new_index..new_index + new_len),
            _ => None,
        };
        if let Some(chars) = changed {
            if !chars.is_empty() {
                highlighted_chars += chars.len();
                ranges.push(IntralineRange { line, chars });
            }
        }
    }

    if highlighted_chars == 0 || too_dense(highlighted_chars, target_len) {
        ranges.truncate(ranges_start);
    }
}

fn push_word_diff_line_ranges(
    before: &str,
    after: &str,
    line: usize,
    side: IntralineSide,
    ranges: &mut Vec<IntralineRange>,
) {
    let before_len = before.chars().count();
    let after_len = after.chars().count();
    let target_len = match side {
        IntralineSide::Before => before_len,
        IntralineSide::After => after_len,
    };
    if target_len == 0
        || before_len > MAX_INTRALINE_LINE_CHARS
        || after_len > MAX_INTRALINE_LINE_CHARS
        || (before_len != 0 && after_len != 0 && !length_ratio_ok(before_len, after_len))
    {
        return;
    }

    let before_words = before.tokenize_words();
    let after_words = after.tokenize_words();
    let ops = diff_slices_without_compact(Algorithm::Patience, &before_words, &after_words);
    if get_diff_ratio(&ops, before_words.len(), after_words.len()) < MIN_WORD_DIFF_RATIO {
        return;
    }

    let ranges_start = ranges.len();
    let mut char_idx = 0;
    let mut highlighted_chars = 0;
    for op in ops {
        match (side, op) {
            (IntralineSide::Before, DiffOp::Equal { old_index, len, .. }) => {
                char_idx += before_words[old_index..old_index + len]
                    .iter()
                    .map(|word| word.chars().count())
                    .sum::<usize>();
            }
            (
                IntralineSide::Before,
                DiffOp::Delete {
                    old_index, old_len, ..
                }
                | DiffOp::Replace {
                    old_index, old_len, ..
                },
            ) => {
                let char_count: usize = before_words[old_index..old_index + old_len]
                    .iter()
                    .map(|word| word.chars().count())
                    .sum();
                if char_count != 0 {
                    ranges.push(IntralineRange {
                        line,
                        chars: char_idx..char_idx + char_count,
                    });
                    highlighted_chars += char_count;
                    char_idx += char_count;
                }
            }
            (IntralineSide::After, DiffOp::Equal { new_index, len, .. }) => {
                char_idx += after_words[new_index..new_index + len]
                    .iter()
                    .map(|word| word.chars().count())
                    .sum::<usize>();
            }
            (
                IntralineSide::After,
                DiffOp::Insert {
                    new_index, new_len, ..
                }
                | DiffOp::Replace {
                    new_index, new_len, ..
                },
            ) => {
                let char_count: usize = after_words[new_index..new_index + new_len]
                    .iter()
                    .map(|word| word.chars().count())
                    .sum();
                if char_count != 0 {
                    ranges.push(IntralineRange {
                        line,
                        chars: char_idx..char_idx + char_count,
                    });
                    highlighted_chars += char_count;
                    char_idx += char_count;
                }
            }
            _ => {}
        }
    }

    if highlighted_chars == 0 || too_dense(highlighted_chars, target_len) {
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
    use super::{adaptive_intraline_ranges, IntralineRange, IntralineSide};

    fn range(line: usize, chars: std::ops::Range<usize>) -> IntralineRange {
        IntralineRange { line, chars }
    }

    #[test]
    fn small_one_line_edit_uses_character_ranges() {
        assert_eq!(
            adaptive_intraline_ranges("let value = 1;\n", "let value = 2;\n", IntralineSide::After),
            vec![range(0, 12..13)]
        );
        assert_eq!(
            adaptive_intraline_ranges(
                "let value = 1;\n",
                "let value = 2;\n",
                IntralineSide::Before
            ),
            vec![range(0, 12..13)]
        );
    }

    #[test]
    fn repeated_trailing_char_replacement_stays_aligned_on_both_sides() {
        // Replacing the first `b` in `aaabbb` with `c` (`aaacbb`) must highlight
        // index 3 on both panes, not slide `b` to the end of the run (index 5).
        assert_eq!(
            adaptive_intraline_ranges("aaabbb\n", "aaacbb\n", IntralineSide::Before),
            vec![range(0, 3..4)]
        );
        assert_eq!(
            adaptive_intraline_ranges("aaabbb\n", "aaacbb\n", IntralineSide::After),
            vec![range(0, 3..4)]
        );

        // And symmetrically in the reverse edit direction (`aaacbb` -> `aaabbb`).
        assert_eq!(
            adaptive_intraline_ranges("aaacbb\n", "aaabbb\n", IntralineSide::Before),
            vec![range(0, 3..4)]
        );
        assert_eq!(
            adaptive_intraline_ranges("aaacbb\n", "aaabbb\n", IntralineSide::After),
            vec![range(0, 3..4)]
        );
    }

    #[test]
    fn word_diff_fallback_aligns_both_sides() {
        // Char ratio is < 0.55, so this falls back to word diff and aligns
        // both replaced words (`zzzzzzzz`/`yyyyyyyy` and `p`/`q`) on both sides.
        assert_eq!(
            adaptive_intraline_ranges(
                "a b c zzzzzzzz p q q\n",
                "a b c yyyyyyyy q q q\n",
                IntralineSide::After
            ),
            vec![range(0, 6..14), range(0, 15..16)]
        );
        assert_eq!(
            adaptive_intraline_ranges(
                "a b c zzzzzzzz p q q\n",
                "a b c yyyyyyyy q q q\n",
                IntralineSide::Before
            ),
            vec![range(0, 6..14), range(0, 15..16)]
        );
    }

    #[test]
    fn repeated_trailing_line_replacement_does_not_shift_to_last_line() {
        assert_eq!(
            adaptive_intraline_ranges(
                "same\nbbbc\nbbbb\n",
                "same\nbbbb\nbbbb\n",
                IntralineSide::Before
            ),
            vec![range(1, 3..4)]
        );
        assert_eq!(
            adaptive_intraline_ranges(
                "same\nbbbc\nbbbb\n",
                "same\nbbbb\nbbbb\n",
                IntralineSide::After
            ),
            vec![range(1, 3..4)]
        );
    }

    #[test]
    fn inserted_lines_do_not_shift_later_character_pairs() {
        assert_eq!(
            adaptive_intraline_ranges(
                "same\nlet value = 1;\nnext\n",
                "same\ninserted\nlet value = 2;\nnext\n",
                IntralineSide::After
            ),
            vec![range(1, 0..9), range(2, 12..13)]
        );
        assert_eq!(
            adaptive_intraline_ranges(
                "same\nlet value = 1;\nnext\n",
                "same\ninserted\nlet value = 2;\nnext\n",
                IntralineSide::Before
            ),
            vec![range(1, 12..13)]
        );
    }

    #[test]
    fn large_hunks_skip_intraline_ranges() {
        let before = (0..24).map(|i| format!("before {i}\n")).collect::<String>();
        let after = (0..24).map(|i| format!("after {i}\n")).collect::<String>();

        assert!(adaptive_intraline_ranges(&before, &after, IntralineSide::After).is_empty());
        assert!(adaptive_intraline_ranges(&before, &after, IntralineSide::Before).is_empty());
    }

    #[test]
    fn dense_rewrites_skip_intraline_ranges() {
        assert!(adaptive_intraline_ranges(
            "aaaaaaaaaaaaaaaaaaaaaaaa\n",
            "bbbbbbbbbbbbbbbbbbbbbbbb\n",
            IntralineSide::After
        )
        .is_empty());
        assert!(adaptive_intraline_ranges(
            "aaaaaaaaaaaaaaaaaaaaaaaa\n",
            "bbbbbbbbbbbbbbbbbbbbbbbb\n",
            IntralineSide::Before
        )
        .is_empty());
    }
}
