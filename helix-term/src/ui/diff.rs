use crate::ui::{
    document::{LinePos, TextRenderer},
    text_decorations::Decoration,
};
use helix_core::{syntax::OverlayHighlights, unicode::width::UnicodeWidthStr, Position};
use helix_view::{
    annotations::rows::{Attention, CommentRowKind, RowMark, VirtualRow, VirtualRowPlan},
    graphics::{Color, Modifier, Rect, Style},
    review::MARKER,
    Document, Theme,
};
use similar::{Algorithm, ChangeTag, DiffOp, TextDiff};
use std::{ops::Range, rc::Rc, time::Duration};

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

/// The theme background as rgb, if it has one.
fn background_rgb(theme: &Theme) -> Option<(u8, u8, u8)> {
    match theme.get("ui.background").bg {
        Some(Color::Rgb(r, g, b)) => Some((r, g, b)),
        _ => None,
    }
}

fn luminance((r, g, b): (u8, u8, u8)) -> f64 {
    0.299 * r as f64 + 0.587 * g as f64 + 0.114 * b as f64
}

/// A background derived from `ui.background` by nudging its luminance, used when
/// a theme does not define a diff surface colour of its own.
pub(crate) fn blended_bg(theme: &Theme, delta: u8) -> Style {
    let Some((br, bg, bb)) = background_rgb(theme) else {
        return Style::default().bg(Color::Gray);
    };
    let (r, g, b) = if luminance((br, bg, bb)) < 128.0 {
        (
            (br as u16 + delta as u16).min(255) as u8,
            (bg as u16 + delta as u16).min(255) as u8,
            (bb as u16 + delta as u16).min(255) as u8,
        )
    } else {
        (
            br.saturating_sub(delta),
            bg.saturating_sub(delta),
            bb.saturating_sub(delta),
        )
    };
    Style::default().bg(Color::Rgb(r, g, b))
}

/// A surface with **both** a background and a legible foreground.
///
/// A background alone is not enough: text drawn with no foreground inherits
/// whatever was there, which on a light theme means near-white on near-white.
/// The foreground comes from the theme's own text colour where it has one, and
/// otherwise from the background's luminance, so it is always readable.
pub(crate) fn blended_surface(theme: &Theme, delta: u8, dim: bool) -> Style {
    let style = blended_bg(theme, delta);

    let fg = theme
        .try_get("ui.text")
        .and_then(|text| text.fg)
        .or_else(|| {
            background_rgb(theme).map(|rgb| {
                if luminance(rgb) < 128.0 {
                    Color::White
                } else {
                    Color::Black
                }
            })
        })
        .unwrap_or(Color::White);

    // Headers and stale notes read as secondary, but must still be readable:
    // pull them toward the background rather than washing them out.
    let fg = match (dim, fg, background_rgb(theme)) {
        (true, Color::Rgb(fr, fg_, fb), Some((br, bg, bb))) => Color::Rgb(
            ((fr as u16 + br as u16) / 2) as u8,
            ((fg_ as u16 + bg as u16) / 2) as u8,
            ((fb as u16 + bb as u16) / 2) as u8,
        ),
        (_, fg, _) => fg,
    };

    style.fg(fg)
}

/// Paints the virtual rows reserved by `VirtualRowLines`.
///
/// Walks the same `VirtualRowPlan` the annotation reserved from, so the painted
/// row count always matches the reserved one.
pub(super) struct VirtualRowDecoration {
    plan: Rc<VirtualRowPlan>,
    /// Where each comment row lands, written back as it is painted so the mouse
    /// can find out what it is over.
    hits: Rc<std::cell::RefCell<Vec<helix_view::review::BoxHit>>>,
    view: helix_view::ViewId,
    spacer: Style,
    /// Backgrounds for the two attention levels.
    focused_bg: Option<helix_view::graphics::Color>,
    under_cursor_bg: Option<helix_view::graphics::Color>,
    /// Backgrounds for the cursor and selection drawn inside a focused box.
    box_cursor_bg: Option<helix_view::graphics::Color>,
    box_selection_bg: Option<helix_view::graphics::Color>,
    /// The one cell of the cursor row the in-box cursor points at.
    box_cell: Style,
    user: Style,
    agent: Style,
    pending: Style,
    summary: Style,
    orphaned: Style,
}

impl VirtualRowDecoration {
    pub(super) fn new(
        plan: Rc<VirtualRowPlan>,
        theme: &Theme,
        view: helix_view::ViewId,
        hits: Rc<std::cell::RefCell<Vec<helix_view::review::BoxHit>>>,
    ) -> Self {
        // This view's rows are about to be painted again, so whatever was
        // recorded for it last frame is stale.
        hits.borrow_mut().retain(|hit| hit.view != view);
        // Fallbacks carry a foreground as well as a background. A theme that
        // defines none of these keys must still be readable.
        let body = blended_surface(theme, 12, false);
        let secondary = blended_surface(theme, 12, true);
        let role = |key: &str, fallback: Style| theme.try_get(key).unwrap_or(fallback);
        Self {
            plan,
            hits,
            view,
            focused_bg: theme
                .try_get("ui.review.comment.focused")
                .and_then(|style| style.bg)
                .or_else(|| blended_surface(theme, 40, false).bg),
            under_cursor_bg: theme
                .try_get("ui.review.comment.cursor")
                .and_then(|style| style.bg)
                .or_else(|| blended_surface(theme, 22, false).bg),
            // The box draws its own cursor and selection, so it borrows the
            // editor's own keys for them: a selection inside a box should look
            // like a selection, not like a third kind of highlight.
            box_cursor_bg: theme
                .try_get("ui.review.comment.line")
                .and_then(|style| style.bg)
                .or_else(|| {
                    theme
                        .try_get("ui.cursor.primary")
                        .and_then(|style| style.bg)
                })
                .or_else(|| blended_surface(theme, 70, false).bg),
            box_selection_bg: theme
                .try_get("ui.review.comment.selection")
                .and_then(|style| style.bg)
                .or_else(|| theme.try_get("ui.selection").and_then(|style| style.bg))
                .or_else(|| blended_surface(theme, 55, false).bg),
            box_cell: theme
                .try_get("ui.cursor.primary")
                .or_else(|| theme.try_get("ui.cursor"))
                .unwrap_or_else(|| Style::default().add_modifier(Modifier::REVERSED)),
            spacer: theme
                .try_get("ui.diff.spacer")
                .unwrap_or_else(|| blended_bg(theme, 15)),
            user: role("ui.review.comment.user", body),
            agent: role("ui.review.comment.agent", body),
            pending: role("ui.review.comment.pending", body),
            summary: role("ui.review.comment.collapsed", secondary),
            orphaned: role("ui.review.comment.orphaned", secondary),
        }
    }

    fn comment_style(&self, kind: CommentRowKind) -> Style {
        match kind {
            CommentRowKind::Summary => self.summary,
            CommentRowKind::User => self.user,
            CommentRowKind::Agent => self.agent,
            CommentRowKind::Pending => self.pending,
            CommentRowKind::Orphaned => self.orphaned,
        }
    }
}

impl Decoration for VirtualRowDecoration {
    fn render_virt_lines(
        &mut self,
        renderer: &mut TextRenderer,
        pos: LinePos,
        virt_off: Position,
    ) -> Position {
        if !pos.first_visual_line {
            return Position::default();
        }
        let rows = self.plan.rows_at(pos.doc_line);
        if rows.is_empty() {
            return Position::default();
        }

        let viewport = renderer.viewport;
        for (i, row) in rows.iter().enumerate() {
            let y = pos.visual_line + virt_off.row as u16 + i as u16;
            if y >= renderer.offset.row as u16 + viewport.height {
                break;
            }
            match row {
                VirtualRow::Spacer => {
                    renderer.set_style(Rect::new(viewport.x, y, viewport.width, 1), self.spacer);
                }
                // The open input draws over these; painting them keeps the code
                // beneath from showing through while it is being typed into.
                VirtualRow::Composing => {
                    renderer.set_style(Rect::new(viewport.x, y, viewport.width, 1), self.spacer);
                }
                VirtualRow::Comment {
                    thread,
                    kind,
                    text,
                    spans,
                    attention,
                    mark,
                    body,
                } => {
                    self.hits.borrow_mut().push(helix_view::review::BoxHit {
                        view: self.view,
                        row: y,
                        x: viewport.x,
                        width: viewport.width,
                        thread: *thread,
                        body: *body,
                    });
                    let mut style = self.comment_style(*kind);
                    // Two steps of emphasis: passing the line tints it, stopping
                    // on the box tints it further, so "this one takes my keys"
                    // is visible rather than inferred.
                    match attention {
                        Attention::Focused => {
                            if let Some(bg) = self.focused_bg {
                                style = style.bg(bg);
                            }
                        }
                        Attention::UnderCursor => {
                            if let Some(bg) = self.under_cursor_bg {
                                style = style.bg(bg);
                            }
                        }
                        Attention::Idle => {}
                    }
                    // The in-box cursor and selection sit on top of that: they
                    // say where `y` would copy from, which is a stronger claim
                    // than which box has the keys.
                    match mark {
                        RowMark::Cursor { .. } => {
                            if let Some(bg) = self.box_cursor_bg {
                                style = style.bg(bg);
                            }
                        }
                        RowMark::Selected => {
                            if let Some(bg) = self.box_selection_bg {
                                style = style.bg(bg);
                            }
                        }
                        RowMark::Span { .. } | RowMark::None => {}
                    }
                    renderer.set_style(Rect::new(viewport.x, y, viewport.width, 1), style);
                    renderer.set_stringn(viewport.x, y, MARKER, 1, style);
                    let text_x = viewport.x + 1;
                    let text_width = viewport.width.saturating_sub(1);
                    if spans.is_empty() {
                        renderer.set_stringn(text_x, y, text, text_width as usize, style);
                    } else {
                        // Span backgrounds would cover the focus and cursor
                        // tint painted above. Keep the markdown foreground and
                        // emphasis, and let the row keep its own background.
                        paint_comment_spans(renderer, text_x, y, text_width, style, spans);
                    }
                    // Part of a row picked out goes on last, over the text.
                    let (start, end, cell) = match *mark {
                        RowMark::Cursor { col } => (col, col.saturating_add(1), self.box_cell),
                        RowMark::Span { start, end } => (
                            start,
                            end,
                            self.box_selection_bg
                                .map_or(Style::default(), |bg| Style::default().bg(bg)),
                        ),
                        RowMark::Selected | RowMark::None => (0, 0, Style::default()),
                    };
                    let end = end.min(text_width);
                    if start < end {
                        renderer.set_style(Rect::new(text_x + start, y, end - start, 1), cell);
                    }
                }
            }
        }

        // Always the full reserved count, even when the loop broke early at the
        // viewport edge: the annotation reserved this many, and the formatter
        // laid text out around that count.
        Position::new(rows.len(), 0)
    }
}

/// Paint one markdown row. The row background is already set; span backgrounds
/// are dropped so a focused or selected row keeps that tint.
fn paint_comment_spans(
    renderer: &mut TextRenderer,
    mut x: u16,
    y: u16,
    width: u16,
    row_style: Style,
    spans: &[helix_view::annotations::rows::CommentSpan],
) {
    let right = x.saturating_add(width);
    for span in spans {
        if x >= right || span.text.is_empty() {
            continue;
        }
        let room = (right - x) as usize;
        let mut span_style = span.style;
        span_style.bg = None;
        let span_style = row_style.patch(span_style);
        renderer.set_stringn(x, y, &span.text, room, span_style);
        let advance = UnicodeWidthStr::width(span.text.as_str()).min(room) as u16;
        x = x.saturating_add(advance);
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
