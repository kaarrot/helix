use crate::compositor::{Component, Context};
use arc_swap::ArcSwap;
use tui::{
    buffer::Buffer as Surface,
    text::{Span, Spans, Text},
};

use std::sync::Arc;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use helix_core::{
    syntax::{self, HighlightEvent, OverlayHighlights},
    unicode::{segmentation::UnicodeSegmentation, width::UnicodeWidthStr},
    RopeSlice, Syntax,
};
use helix_view::{
    graphics::{Margin, Rect, Style, UnderlineStyle},
    theme::Modifier,
    Theme,
};

/// A markdown link/image destination anchored to a rendered line. Produced by
/// [`Markdown::parse_with_map`] so the markdown preview can offer goto-file on
/// the links written in the document.
#[derive(Debug, Clone)]
pub struct MarkdownLink {
    /// Index into the rendered lines that this link's text appears on.
    pub line: usize,
    /// The link destination as written in the markdown (URL or path).
    pub dest: String,
    /// Display column range of the link text on the rendered line `[start, end)`.
    pub start_col: u16,
    pub end_col: u16,
}

const TABLE_SEP: &str = " │ ";
const TABLE_SEP_WIDTH: usize = 3;
const TABLE_DIV: &str = "─┼─";
const MIN_CELL_WIDTH: usize = 3;

/// Buffered contents of a GFM table while it is being parsed. The whole table
/// is collected before emitting so per-column widths can be computed and each
/// cell padded into an aligned column.
struct TableCtx<'a> {
    alignments: Vec<Alignment>,
    rows: Vec<TableRowCtx<'a>>,
    cur_row: Option<TableRowCtx<'a>>,
    /// Cell currently being filled; inline events append here instead of the
    /// main span buffer (so `push_line` is not used for table rows).
    cur_cell: Option<TableCell<'a>>,
}

struct TableRowCtx<'a> {
    cells: Vec<TableCell<'a>>,
    is_header: bool,
    src: Option<usize>,
}

struct TableCell<'a> {
    spans: Vec<Span<'a>>,
    /// Destinations with display-column range relative to the start of the cell.
    links: Vec<(String, usize, usize)>,
}

struct OpenLink {
    dest: String,
    start_col: usize,
}

struct LineLink {
    dest: String,
    start_col: usize,
    end_col: usize,
}

struct GraphemePiece {
    text: String,
    style: Style,
    width: usize,
    orig_col: usize,
    is_whitespace: bool,
}

struct WrappedRow<'a> {
    spans: Vec<Span<'a>>,
    orig_to_vis: Vec<(usize, usize, usize)>,
}

/// Total display width of a cell's spans.
fn cells_width(spans: &[Span]) -> usize {
    spans.iter().map(Span::width).sum()
}

fn cell_text(spans: &[Span]) -> String {
    spans.iter().map(|s| s.content.as_ref()).collect()
}

/// Pad `cell` to `width` columns per `align`, styling the inserted spaces with
/// `pad_style`.
fn pad_cell(
    cell: Vec<Span<'_>>,
    width: usize,
    align: Alignment,
    pad_style: Style,
) -> Vec<Span<'_>> {
    let pad = width.saturating_sub(cells_width(&cell));
    let space = |n: usize| Span::styled(" ".repeat(n), pad_style);
    let mut out = Vec::new();
    match align {
        Alignment::Right => {
            out.push(space(pad));
            out.extend(cell);
        }
        Alignment::Center => {
            let left = pad / 2;
            out.push(space(left));
            out.extend(cell);
            out.push(space(pad - left));
        }
        Alignment::Left | Alignment::None => {
            out.extend(cell);
            out.push(space(pad));
        }
    }
    out
}

fn flatten_graphemes(spans: &[Span]) -> Vec<GraphemePiece> {
    let mut col = 0;
    let mut out = Vec::new();
    for span in spans {
        for g in span.content.graphemes(true) {
            let w = UnicodeWidthStr::width(g);
            let is_whitespace =
                g.chars().all(char::is_whitespace) && g != "\u{00a0}" && g != "\u{202f}";
            out.push(GraphemePiece {
                text: g.to_string(),
                style: span.style,
                width: w,
                orig_col: col,
                is_whitespace,
            });
            col += w;
        }
    }
    out
}

fn pieces_to_spans<'a>(pieces: &[GraphemePiece]) -> Vec<Span<'a>> {
    let mut spans: Vec<Span<'a>> = Vec::new();
    for p in pieces {
        if let Some(last) = spans.last_mut() {
            if last.style == p.style {
                last.content.to_mut().push_str(&p.text);
                continue;
            }
        }
        spans.push(Span::styled(p.text.clone(), p.style));
    }
    spans
}

fn wrap_graphemes<'a>(pieces: &[GraphemePiece], width: usize) -> Vec<WrappedRow<'a>> {
    if pieces.is_empty() {
        return vec![WrappedRow {
            spans: Vec::new(),
            orig_to_vis: Vec::new(),
        }];
    }
    if width == 0 {
        let mut vis = 0;
        let orig_to_vis = pieces
            .iter()
            .map(|p| {
                let entry = (p.orig_col, vis, p.width);
                vis += p.width;
                entry
            })
            .collect();
        return vec![WrappedRow {
            spans: pieces_to_spans(pieces),
            orig_to_vis,
        }];
    }

    let mut rows = Vec::new();
    let mut i = 0;
    while i < pieces.len() {
        let mut end = i;
        let mut line_width = 0;
        let mut last_ws = None;
        while end < pieces.len() {
            let p = &pieces[end];
            if line_width + p.width > width && line_width > 0 {
                break;
            }
            line_width += p.width;
            if p.is_whitespace {
                last_ws = Some(end + 1);
            }
            end += 1;
        }
        let mut row_end = if end < pieces.len() {
            last_ws.filter(|&w| w > i).unwrap_or(end)
        } else {
            end
        };
        if row_end == i {
            row_end = (i + 1).min(pieces.len());
        }

        let row_pieces = &pieces[i..row_end];
        let mut vis = 0;
        let mut orig_to_vis = Vec::new();
        for p in row_pieces {
            orig_to_vis.push((p.orig_col, vis, p.width));
            vis += p.width;
        }
        rows.push(WrappedRow {
            spans: pieces_to_spans(row_pieces),
            orig_to_vis,
        });

        i = row_end;
        if i < pieces.len() {
            while i < pieces.len() && pieces[i].is_whitespace {
                i += 1;
            }
        }
    }

    if rows.is_empty() {
        rows.push(WrappedRow {
            spans: Vec::new(),
            orig_to_vis: Vec::new(),
        });
    }
    rows
}

fn map_link_to_rows(rows: &[WrappedRow<'_>], start: usize, end: usize) -> Vec<(usize, u16, u16)> {
    let mut out = Vec::new();
    for (ri, row) in rows.iter().enumerate() {
        let mut vis_start = None;
        let mut vis_end = None;
        for &(orig, vis, w) in &row.orig_to_vis {
            if orig < end && orig + w > start {
                vis_start = Some(vis_start.map_or(vis, |s: usize| s.min(vis)));
                vis_end = Some(vis + w);
            }
        }
        if let (Some(s), Some(e)) = (vis_start, vis_end) {
            out.push((ri, s as u16, e.max(s + 1) as u16));
        }
    }
    out
}

fn budget_column_widths(natural: &[usize], wrap_width: u16) -> Option<Vec<usize>> {
    let n_cols = natural.len();
    if n_cols == 0 {
        return Some(Vec::new());
    }
    if wrap_width == 0 {
        return Some(natural.to_vec());
    }
    let available = (wrap_width as usize)
        .saturating_sub(TABLE_SEP_WIDTH.saturating_mul(n_cols.saturating_sub(1)));
    if available < n_cols * MIN_CELL_WIDTH {
        return None;
    }
    let natural_sum: usize = natural.iter().sum();
    if natural_sum <= available {
        return Some(natural.to_vec());
    }

    let mut widths = vec![MIN_CELL_WIDTH; n_cols];
    let extra = available.saturating_sub(MIN_CELL_WIDTH * n_cols);
    let deficits: Vec<(usize, usize)> = (0..n_cols)
        .map(|i| (i, natural[i].saturating_sub(MIN_CELL_WIDTH)))
        .filter(|(_, d)| *d > 0)
        .collect();
    let total_deficit: usize = deficits.iter().map(|(_, d)| *d).sum();
    if total_deficit > 0 && extra > 0 {
        for &(i, d) in &deficits {
            widths[i] += extra * d / total_deficit;
        }
        let mut rem = available.saturating_sub(widths.iter().sum());
        for &(i, _) in &deficits {
            if rem == 0 {
                break;
            }
            let can = natural[i].saturating_sub(widths[i]);
            let add = can.min(rem);
            widths[i] += add;
            rem -= add;
        }
    }
    Some(widths)
}

fn header_labels(rows: &[TableRowCtx]) -> Vec<String> {
    match rows.iter().find(|r| r.is_header) {
        Some(header) => header
            .cells
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                let text = cell_text(&cell.spans);
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    format!("Col {}", i + 1)
                } else {
                    trimmed.to_string()
                }
            })
            .collect(),
        None => Vec::new(),
    }
}

fn label_for(labels: &[String], i: usize) -> String {
    labels
        .get(i)
        .cloned()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("Col {}", i + 1))
}

fn push_table_line<'a>(
    spans: Vec<Span<'a>>,
    src: Option<usize>,
    lines: &mut Vec<Spans<'a>>,
    line_map: &mut Vec<Option<usize>>,
) -> usize {
    let idx = lines.len();
    lines.push(Spans::from(spans));
    line_map.push(src);
    idx
}

fn emit_cell_links(
    links: &[(String, usize, usize)],
    rows: &[WrappedRow<'_>],
    line_base: usize,
    col_offset: usize,
    col_width: usize,
    out: &mut Vec<MarkdownLink>,
) {
    if links.is_empty() {
        return;
    }
    for (dest, start, end) in links {
        let mapped = map_link_to_rows(rows, *start, *end);
        if mapped.is_empty() {
            for ri in 0..rows.len().max(1) {
                out.push(MarkdownLink {
                    line: line_base + ri,
                    dest: dest.clone(),
                    start_col: col_offset as u16,
                    end_col: (col_offset + col_width) as u16,
                });
            }
        } else {
            for (ri, vs, ve) in mapped {
                out.push(MarkdownLink {
                    line: line_base + ri,
                    dest: dest.clone(),
                    start_col: (col_offset as u16).saturating_add(vs),
                    end_col: (col_offset as u16).saturating_add(ve),
                });
            }
        }
    }
}

fn emit_stacked_row<'a>(
    row: TableRowCtx<'a>,
    labels: &[String],
    wrap_width: u16,
    border_style: Style,
    lines: &mut Vec<Spans<'a>>,
    line_map: &mut Vec<Option<usize>>,
    links: &mut Vec<MarkdownLink>,
) {
    let width = wrap_width as usize;
    for (i, cell) in row.cells.into_iter().enumerate() {
        let label = label_for(labels, i);
        let prefix = format!("{label}: ");
        let prefix_width = UnicodeWidthStr::width(prefix.as_str());
        let mut spans = vec![Span::styled(prefix, border_style)];
        spans.extend(cell.spans);
        let wrapped = wrap_graphemes(&flatten_graphemes(&spans), width);
        let line_base = lines.len();
        for wrow in &wrapped {
            push_table_line(wrow.spans.clone(), row.src, lines, line_map);
        }
        let offset_links: Vec<(String, usize, usize)> = cell
            .links
            .iter()
            .map(|(dest, start, end)| (dest.clone(), start + prefix_width, end + prefix_width))
            .collect();
        emit_cell_links(&offset_links, &wrapped, line_base, 0, 0, links);
    }
}

/// Emit a buffered table as aligned rendered lines. When `wrap_width` is non-zero,
/// each cell is word-wrapped to a budgeted column width; if the columns cannot
/// fit, the row is stacked as `Col: value` lines.
fn emit_table<'a>(
    mut table: TableCtx<'a>,
    text_style: Style,
    border_style: Style,
    wrap_width: u16,
    lines: &mut Vec<Spans<'a>>,
    line_map: &mut Vec<Option<usize>>,
    links: &mut Vec<MarkdownLink>,
) {
    let n_cols = table
        .rows
        .iter()
        .map(|row| row.cells.len())
        .max()
        .unwrap_or(0)
        .max(table.alignments.len());
    if n_cols == 0 {
        return;
    }

    for row in &mut table.rows {
        while row.cells.len() < n_cols {
            row.cells.push(TableCell {
                spans: Vec::new(),
                links: Vec::new(),
            });
        }
    }

    let mut natural = vec![1usize; n_cols];
    for row in &table.rows {
        for (i, cell) in row.cells.iter().enumerate() {
            natural[i] = natural[i].max(cells_width(&cell.spans).max(1));
        }
    }

    let labels = header_labels(&table.rows);
    let widths = budget_column_widths(&natural, wrap_width);
    let stacked = widths.is_none();
    let skip_header = stacked && table.rows.iter().any(|r| !r.is_header);

    if stacked {
        for row in table.rows {
            if row.is_header && skip_header {
                continue;
            }
            emit_stacked_row(
                row,
                &labels,
                wrap_width,
                border_style,
                lines,
                line_map,
                links,
            );
        }
        return;
    }

    let widths = widths.unwrap();
    let align = |i: usize| table.alignments.get(i).copied().unwrap_or(Alignment::None);
    let mut divider_emitted = false;

    for row in table.rows {
        let is_header = row.is_header;
        let src = row.src;
        let mut wrapped_cells: Vec<(Vec<WrappedRow>, Vec<(String, usize, usize)>)> =
            Vec::with_capacity(n_cols);
        let mut height = 1usize;
        for (i, cell) in row.cells.into_iter().enumerate() {
            let col_w = widths[i];
            let rows = wrap_graphemes(
                &flatten_graphemes(&cell.spans),
                if wrap_width > 0 { col_w } else { 0 },
            );
            height = height.max(rows.len().max(1));
            wrapped_cells.push((rows, cell.links));
        }

        let line_base = lines.len();
        for vis_row in 0..height {
            let mut spans: Vec<Span> = Vec::new();
            for (i, (rows, _)) in wrapped_cells.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::styled(TABLE_SEP, border_style));
                }
                let cell_spans = rows
                    .get(vis_row)
                    .map(|r| r.spans.clone())
                    .unwrap_or_default();
                spans.extend(pad_cell(cell_spans, widths[i], align(i), text_style));
            }
            push_table_line(spans, src, lines, line_map);
        }

        let mut col_offset = 0usize;
        for (i, (rows, cell_links)) in wrapped_cells.iter().enumerate() {
            emit_cell_links(cell_links, rows, line_base, col_offset, widths[i], links);
            col_offset += widths[i] + TABLE_SEP_WIDTH;
        }

        if is_header && !divider_emitted {
            divider_emitted = true;
            let mut sep: Vec<Span> = Vec::new();
            for (i, width) in widths.iter().enumerate() {
                if i > 0 {
                    sep.push(Span::styled(TABLE_DIV, border_style));
                }
                sep.push(Span::styled("─".repeat(*width), border_style));
            }
            push_table_line(sep, None, lines, line_map);
        }
    }
}

fn styled_multiline_text<'a>(text: &str, style: Style) -> Text<'a> {
    let spans: Vec<_> = text
        .lines()
        .map(|line| Span::styled(line.to_string(), style))
        .map(Spans::from)
        .collect();
    Text::from(spans)
}

pub fn highlighted_code_block<'a>(
    text: &str,
    language: &str,
    theme: Option<&Theme>,
    loader: &syntax::Loader,
    // Optional overlay highlights to mix in with the syntax highlights.
    //
    // Note that `OverlayHighlights` is typically used with char indexing but the only caller
    // which passes this parameter currently passes **byte indices** instead.
    additional_highlight_spans: Option<OverlayHighlights>,
) -> Text<'a> {
    let mut spans = Vec::new();
    let mut lines = Vec::new();

    let get_theme = |key: &str| -> Style { theme.map(|t| t.get(key)).unwrap_or_default() };
    let text_style = get_theme(Markdown::TEXT_STYLE);
    let code_style = get_theme(Markdown::BLOCK_STYLE);

    let theme = match theme {
        Some(t) => t,
        None => return styled_multiline_text(text, code_style),
    };

    let ropeslice = RopeSlice::from(text);
    let Some(syntax) = loader
        .language_for_match(RopeSlice::from(language))
        .and_then(|lang| Syntax::new(ropeslice, lang, loader).ok())
    else {
        return styled_multiline_text(text, code_style);
    };

    let mut syntax_highlighter = syntax.highlighter(ropeslice, loader, ..);
    let mut syntax_highlight_stack = Vec::new();
    let mut overlay_highlight_stack = Vec::new();
    let mut overlay_highlighter = syntax::OverlayHighlighter::new(additional_highlight_spans);
    let mut pos = 0;

    while pos < ropeslice.len_bytes() as u32 {
        if pos == syntax_highlighter.next_event_offset() {
            let (event, new_highlights) = syntax_highlighter.advance();
            if event == HighlightEvent::Refresh {
                syntax_highlight_stack.clear();
            }
            syntax_highlight_stack.extend(new_highlights);
        } else if pos == overlay_highlighter.next_event_offset() as u32 {
            let (event, new_highlights) = overlay_highlighter.advance();
            if event == HighlightEvent::Refresh {
                overlay_highlight_stack.clear();
            }
            overlay_highlight_stack.extend(new_highlights)
        }

        let start = pos;
        pos = syntax_highlighter
            .next_event_offset()
            .min(overlay_highlighter.next_event_offset() as u32);
        if pos == u32::MAX {
            pos = ropeslice.len_bytes() as u32;
        }
        if pos == start {
            continue;
        }
        // The highlighter should always move forward.
        // If the highlighter malfunctions, bail on syntax highlighting and log an error.
        debug_assert!(pos > start);
        if pos < start {
            log::error!("Failed to highlight '{language}': {text:?}");
            return styled_multiline_text(text, code_style);
        }

        let style = syntax_highlight_stack
            .iter()
            .chain(overlay_highlight_stack.iter())
            .fold(text_style, |acc, highlight| {
                acc.patch(theme.highlight(*highlight))
            });

        let mut slice = &text[start as usize..pos as usize];
        // TODO: do we need to handle all unicode line endings
        // here, or is just '\n' okay?
        while let Some(end) = slice.find('\n') {
            // emit span up to newline
            let text = &slice[..end];
            let text = text.replace('\t', "    "); // replace tabs
            let span = Span::styled(text, style);
            spans.push(span);

            // truncate slice to after newline
            slice = &slice[end + 1..];

            // make a new line
            let spans = std::mem::take(&mut spans);
            lines.push(Spans::from(spans));
        }

        if !slice.is_empty() {
            let span = Span::styled(slice.replace('\t', "    "), style);
            spans.push(span);
        }
    }

    if !spans.is_empty() {
        let spans = std::mem::take(&mut spans);
        lines.push(Spans::from(spans));
    }

    Text::from(lines)
}

pub struct Markdown {
    contents: String,

    config_loader: Arc<ArcSwap<syntax::Loader>>,
}

// TODO: pre-render and self reference via Pin
// better yet, just use Tendril + subtendril for references

impl Markdown {
    const TEXT_STYLE: &'static str = "ui.text";
    const BLOCK_STYLE: &'static str = "markup.raw.inline";
    const RULE_STYLE: &'static str = "punctuation.special";
    const LINK_STYLE: &'static str = "markup.link.text";
    const UNNUMBERED_LIST_STYLE: &'static str = "markup.list.unnumbered";
    const NUMBERED_LIST_STYLE: &'static str = "markup.list.numbered";
    const HEADING_STYLES: [&'static str; 6] = [
        "markup.heading.1",
        "markup.heading.2",
        "markup.heading.3",
        "markup.heading.4",
        "markup.heading.5",
        "markup.heading.6",
    ];
    const INDENT: &'static str = "  ";

    pub fn new(contents: String, config_loader: Arc<ArcSwap<syntax::Loader>>) -> Self {
        Self {
            contents,
            config_loader,
        }
    }

    pub fn parse(&self, theme: Option<&Theme>) -> tui::text::Text<'_> {
        // Hover / completion / signature-help: original renderer (no GFM table
        // layout, HTML stripping, or link collection).
        self.parse_impl(theme, false, 0, false).0
    }

    /// Preview parser: aligned/wrapping GFM tables, HTML tag stripping, and
    /// per-line source mapping plus link column ranges for click-to-source
    /// and goto-file.
    ///
    /// `wrap_width` is the inner panel width used to budget table columns and
    /// (when `wrap_prose` is set) to wrap long paragraphs. `0` means no width
    /// constraint.
    pub fn parse_with_map(
        &self,
        theme: Option<&Theme>,
        wrap_width: u16,
        wrap_prose: bool,
    ) -> (tui::text::Text<'_>, Vec<Option<usize>>, Vec<MarkdownLink>) {
        self.parse_impl(theme, true, wrap_width, wrap_prose)
    }

    fn parse_impl(
        &self,
        theme: Option<&Theme>,
        preview: bool,
        wrap_width: u16,
        wrap_prose: bool,
    ) -> (tui::text::Text<'_>, Vec<Option<usize>>, Vec<MarkdownLink>) {
        fn push_line<'a>(
            spans: &mut Vec<Span<'a>>,
            lines: &mut Vec<Spans<'a>>,
            line_map: &mut Vec<Option<usize>>,
            links: &mut Vec<MarkdownLink>,
            line_links: &mut Vec<LineLink>,
            src: Option<usize>,
        ) {
            let spans = std::mem::take(spans);
            if !spans.is_empty() {
                let idx = lines.len();
                let line_width = cells_width(&spans);
                lines.push(Spans::from(spans));
                line_map.push(src);
                for link in line_links.drain(..) {
                    let end = link
                        .end_col
                        .max(link.start_col + 1)
                        .min(line_width.max(link.start_col + 1));
                    links.push(MarkdownLink {
                        line: idx,
                        dest: link.dest,
                        start_col: link.start_col as u16,
                        end_col: end as u16,
                    });
                }
            } else {
                line_links.clear();
            }
        }

        // Push an inserted blank/separator line with no corresponding source line.
        fn blank<'a>(lines: &mut Vec<Spans<'a>>, line_map: &mut Vec<Option<usize>>) {
            lines.push(Spans::default());
            line_map.push(None);
        }

        // Preview-only: strip tags, treat `<br>` as a line break, keep inner text.
        #[allow(clippy::too_many_arguments)]
        fn push_html<'a>(
            html: &str,
            style: Style,
            spans: &mut Vec<Span<'a>>,
            lines: &mut Vec<Spans<'a>>,
            line_map: &mut Vec<Option<usize>>,
            links: &mut Vec<MarkdownLink>,
            line_links: &mut Vec<LineLink>,
            src: Option<usize>,
        ) {
            let mut cur = String::new();
            let mut chars = html.chars();
            while let Some(c) = chars.next() {
                match c {
                    '<' => {
                        let mut tag = String::new();
                        for tc in chars.by_ref() {
                            if tc == '>' {
                                break;
                            }
                            tag.push(tc);
                        }
                        let name = tag.trim().trim_start_matches('/').to_ascii_lowercase();
                        let is_br =
                            name == "br" || name.starts_with("br ") || name.starts_with("br/");
                        if is_br {
                            if !cur.is_empty() {
                                spans.push(Span::styled(std::mem::take(&mut cur), style));
                            }
                            push_line(spans, lines, line_map, links, line_links, src);
                        }
                    }
                    '\n' | '\r' => {}
                    _ => cur.push(c),
                }
            }
            if !cur.is_empty() {
                spans.push(Span::styled(cur, style));
            }
        }

        // Byte offset -> 0-based source line, via line-start offsets.
        let mut line_starts = vec![0usize];
        for (i, b) in self.contents.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        let byte_to_line = |b: usize| {
            line_starts
                .partition_point(|&start| start <= b)
                .saturating_sub(1)
        };

        let mut options = Options::empty();
        options.insert(Options::ENABLE_STRIKETHROUGH);
        if preview {
            options.insert(Options::ENABLE_TABLES);
        }
        // `into_offset_iter` yields the source byte range alongside each event so
        // we can map rendered lines back to source lines.
        let parser = Parser::new_ext(&self.contents, options).into_offset_iter();

        // TODO: if possible, render links as terminal hyperlinks: https://gist.github.com/egmontkob/eb114294efbcd5adb1944c9f3cb5feda
        let mut tags = Vec::new();
        let mut spans = Vec::new();
        let mut lines = Vec::new();
        let mut line_map: Vec<Option<usize>> = Vec::new();
        let mut links: Vec<MarkdownLink> = Vec::new();
        let mut line_links: Vec<LineLink> = Vec::new();
        let mut open_link: Option<OpenLink> = None;
        let mut src: Option<usize> = None;
        let mut list_stack = Vec::new();
        let mut table: Option<TableCtx> = None;

        let get_indent = |level: usize| {
            if level < 1 {
                String::new()
            } else {
                Self::INDENT.repeat(level - 1)
            }
        };

        let get_theme = |key: &str| -> Style { theme.map(|t| t.get(key)).unwrap_or_default() };
        let text_style = get_theme(Self::TEXT_STYLE);
        let code_style = get_theme(Self::BLOCK_STYLE);
        let numbered_list_style = get_theme(Self::NUMBERED_LIST_STYLE);
        let unnumbered_list_style = get_theme(Self::UNNUMBERED_LIST_STYLE);
        let rule_style = get_theme(Self::RULE_STYLE);
        let link_style = get_theme(Self::LINK_STYLE);
        let heading_styles: Vec<Style> = Self::HEADING_STYLES
            .iter()
            .map(|key| get_theme(key))
            .collect();

        // Transform text in `<code>` blocks into `Event::Code`
        let mut in_code = false;
        let parser = parser.filter_map(|(event, range)| match event {
            Event::Html(tag)
                if tag.starts_with("<code") && matches!(tag.chars().nth(5), Some(' ' | '>')) =>
            {
                in_code = true;
                None
            }
            Event::Html(tag) if *tag == *"</code>" => {
                in_code = false;
                None
            }
            Event::Text(text) if in_code => Some((Event::Code(text), range)),
            _ => Some((event, range)),
        });

        for (event, range) in parser {
            // Track the source line of whatever content we're about to emit.
            match &event {
                Event::Start(_) | Event::Text(_) | Event::Code(_) => {
                    src = Some(byte_to_line(range.start));
                }
                _ => {}
            }
            match event {
                Event::Start(Tag::List(list)) => {
                    // if the list stack is not empty this is a sub list, in that
                    // case we need to push the current line before proceeding
                    if !list_stack.is_empty() {
                        push_line(
                            &mut spans,
                            &mut lines,
                            &mut line_map,
                            &mut links,
                            &mut line_links,
                            src,
                        );
                    }

                    list_stack.push(list);
                }
                Event::End(TagEnd::List(_)) => {
                    list_stack.pop();

                    // whenever top-level list closes, empty line
                    if list_stack.is_empty() {
                        blank(&mut lines, &mut line_map);
                    }
                }
                Event::Start(Tag::Item) => {
                    if list_stack.is_empty() {
                        log::warn!("markdown parsing error, list item without list");
                    }

                    tags.push(Tag::Item);

                    // get the appropriate bullet for the current list
                    let (bullet, bullet_style) = list_stack
                        .last()
                        .unwrap_or(&None) // use the '- ' bullet in case the list stack would be empty
                        .map_or((String::from("• "), unnumbered_list_style), |number| {
                            (format!("{}. ", number), numbered_list_style)
                        });

                    // increment the current list number if there is one
                    if let Some(v) = list_stack.last_mut().unwrap_or(&mut None).as_mut() {
                        *v += 1;
                    }

                    let prefix = get_indent(list_stack.len()) + bullet.as_str();
                    spans.push(Span::styled(prefix, bullet_style));
                }
                Event::Start(Tag::Table(alignments)) => {
                    // Buffer the whole table; it's emitted with aligned columns
                    // once we see its End.
                    table = Some(TableCtx {
                        alignments,
                        rows: Vec::new(),
                        cur_row: None,
                        cur_cell: None,
                    });
                }
                Event::Start(Tag::TableHead) => {
                    if let Some(t) = table.as_mut() {
                        t.cur_row = Some(TableRowCtx {
                            cells: Vec::new(),
                            is_header: true,
                            src,
                        });
                    }
                }
                Event::Start(Tag::TableRow) => {
                    if let Some(t) = table.as_mut() {
                        t.cur_row = Some(TableRowCtx {
                            cells: Vec::new(),
                            is_header: false,
                            src,
                        });
                    }
                }
                Event::Start(Tag::TableCell) => {
                    if let Some(t) = table.as_mut() {
                        t.cur_cell = Some(TableCell {
                            spans: Vec::new(),
                            links: Vec::new(),
                        });
                    }
                }
                Event::Start(tag) => {
                    if preview {
                        if let Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } = &tag {
                            let start_col = match table.as_ref().and_then(|t| t.cur_cell.as_ref()) {
                                Some(cell) => cells_width(&cell.spans),
                                None => cells_width(&spans),
                            };
                            open_link = Some(OpenLink {
                                dest: dest_url.to_string(),
                                start_col,
                            });
                        }
                    }
                    tags.push(tag);
                    if spans.is_empty() && !list_stack.is_empty() {
                        spans.push(Span::from(get_indent(list_stack.len())));
                    }
                }
                Event::End(TagEnd::TableCell) => {
                    if let Some(open) = open_link.take() {
                        if let Some(cell) = table.as_mut().and_then(|t| t.cur_cell.as_mut()) {
                            let end = cells_width(&cell.spans).max(open.start_col + 1);
                            cell.links.push((open.dest, open.start_col, end));
                        }
                    }
                    if let Some(t) = table.as_mut() {
                        if let Some(cell) = t.cur_cell.take() {
                            if let Some(row) = t.cur_row.as_mut() {
                                row.cells.push(cell);
                            }
                        }
                    }
                }
                Event::End(TagEnd::TableHead) | Event::End(TagEnd::TableRow) => {
                    if let Some(t) = table.as_mut() {
                        if let Some(row) = t.cur_row.take() {
                            t.rows.push(row);
                        }
                    }
                }
                Event::End(TagEnd::Table) => {
                    if let Some(t) = table.take() {
                        emit_table(
                            t,
                            text_style,
                            rule_style,
                            wrap_width,
                            &mut lines,
                            &mut line_map,
                            &mut links,
                        );
                        blank(&mut lines, &mut line_map);
                    }
                    open_link = None;
                    line_links.clear();
                }
                Event::End(tag) => {
                    if matches!(tag, TagEnd::Link | TagEnd::Image) {
                        if let Some(open) = open_link.take() {
                            let end = match table.as_ref().and_then(|t| t.cur_cell.as_ref()) {
                                Some(cell) => cells_width(&cell.spans),
                                None => cells_width(&spans),
                            }
                            .max(open.start_col + 1);
                            if let Some(cell) = table.as_mut().and_then(|t| t.cur_cell.as_mut()) {
                                cell.links.push((open.dest, open.start_col, end));
                            } else {
                                line_links.push(LineLink {
                                    dest: open.dest,
                                    start_col: open.start_col,
                                    end_col: end,
                                });
                            }
                        }
                    }
                    tags.pop();
                    match tag {
                        TagEnd::Heading(_)
                        | TagEnd::Paragraph
                        | TagEnd::CodeBlock
                        | TagEnd::Item => {
                            push_line(
                                &mut spans,
                                &mut lines,
                                &mut line_map,
                                &mut links,
                                &mut line_links,
                                src,
                            );
                        }
                        _ => (),
                    }

                    // whenever heading, code block or paragraph closes, empty line
                    match tag {
                        TagEnd::Heading(_) | TagEnd::Paragraph | TagEnd::CodeBlock => {
                            blank(&mut lines, &mut line_map);
                        }
                        _ => (),
                    }
                }
                Event::Text(text) => {
                    if let Some(Tag::CodeBlock(kind)) = tags.last() {
                        let language = match kind {
                            CodeBlockKind::Fenced(language) => language,
                            CodeBlockKind::Indented => "",
                        };
                        let tui_text = highlighted_code_block(
                            &text,
                            language,
                            theme,
                            &self.config_loader.load(),
                            None,
                        );
                        // The fenced block's text spans consecutive source lines,
                        // one per highlighted output line.
                        let start_line = byte_to_line(range.start);
                        for (i, line) in tui_text.lines.into_iter().enumerate() {
                            lines.push(line);
                            line_map.push(Some(start_line + i));
                        }
                    } else {
                        let mut style = match tags.last() {
                            Some(Tag::Heading { level, .. }) => match level {
                                HeadingLevel::H1 => heading_styles[0],
                                HeadingLevel::H2 => heading_styles[1],
                                HeadingLevel::H3 => heading_styles[2],
                                HeadingLevel::H4 => heading_styles[3],
                                HeadingLevel::H5 => heading_styles[4],
                                HeadingLevel::H6 => heading_styles[5],
                            },
                            Some(Tag::Emphasis) => text_style.add_modifier(Modifier::ITALIC),
                            Some(Tag::Strong) => text_style.add_modifier(Modifier::BOLD),
                            Some(Tag::Strikethrough) => {
                                text_style.add_modifier(Modifier::CROSSED_OUT)
                            }
                            _ => text_style,
                        };
                        if preview
                            && tags
                                .iter()
                                .any(|tag| matches!(tag, Tag::Link { .. } | Tag::Image { .. }))
                        {
                            style = style.patch(link_style);
                            style.underline_style = Some(UnderlineStyle::Line);
                        }
                        let span = Span::styled(text, style);
                        match table.as_mut().and_then(|t| t.cur_cell.as_mut()) {
                            Some(cell) => cell.spans.push(span),
                            None => spans.push(span),
                        }
                    }
                }
                Event::Code(text) => {
                    let span = Span::styled(text, code_style);
                    match table.as_mut().and_then(|t| t.cur_cell.as_mut()) {
                        Some(cell) => cell.spans.push(span),
                        None => spans.push(span),
                    }
                }
                Event::Html(text) | Event::InlineHtml(text) => {
                    if !preview {
                        spans.push(Span::styled(text, code_style));
                    } else if let Some(cell) = table.as_mut().and_then(|t| t.cur_cell.as_mut()) {
                        cell.spans.push(Span::styled(text, text_style));
                    } else {
                        push_html(
                            &text,
                            text_style,
                            &mut spans,
                            &mut lines,
                            &mut line_map,
                            &mut links,
                            &mut line_links,
                            src,
                        );
                    }
                }
                Event::SoftBreak | Event::HardBreak => {
                    if let Some(open) = open_link.as_mut() {
                        let end = cells_width(&spans).max(open.start_col + 1);
                        line_links.push(LineLink {
                            dest: open.dest.clone(),
                            start_col: open.start_col,
                            end_col: end,
                        });
                        open.start_col = 0;
                    }
                    push_line(
                        &mut spans,
                        &mut lines,
                        &mut line_map,
                        &mut links,
                        &mut line_links,
                        src,
                    );
                    if !list_stack.is_empty() {
                        spans.push(Span::from(get_indent(list_stack.len())));
                    }
                }
                Event::Rule => {
                    lines.push(Spans::from(Span::styled("───", rule_style)));
                    line_map.push(None);
                    blank(&mut lines, &mut line_map);
                }
                // TaskListMarker(bool) true if checked
                _ => {
                    log::warn!("unhandled markdown event {:?}", event);
                }
            }
            // build up a vec of Paragraph tui widgets
        }

        push_line(
            &mut spans,
            &mut lines,
            &mut line_map,
            &mut links,
            &mut line_links,
            src,
        );

        // if last line is empty, remove it
        if let Some(line) = lines.last() {
            if line.0.is_empty() {
                lines.pop();
                line_map.pop();
            }
        }

        if preview && wrap_prose && wrap_width > 0 {
            (lines, line_map, links) = wrap_wide_lines(lines, line_map, links, wrap_width);
        }

        (Text::from(lines), line_map, links)
    }
}

fn wrap_wide_lines<'a>(
    lines: Vec<Spans<'a>>,
    line_map: Vec<Option<usize>>,
    links: Vec<MarkdownLink>,
    width: u16,
) -> (Vec<Spans<'a>>, Vec<Option<usize>>, Vec<MarkdownLink>) {
    let width = width as usize;
    if width == 0 {
        return (lines, line_map, links);
    }
    let mut by_line: Vec<Vec<MarkdownLink>> = vec![Vec::new(); lines.len()];
    for link in links {
        if link.line < by_line.len() {
            by_line[link.line].push(link);
        }
    }
    let mut out_lines = Vec::new();
    let mut out_map = Vec::new();
    let mut out_links = Vec::new();
    for (i, line) in lines.into_iter().enumerate() {
        let src = line_map[i];
        let line_links = std::mem::take(&mut by_line[i]);
        if line.width() <= width {
            let idx = out_lines.len();
            for mut link in line_links {
                link.line = idx;
                out_links.push(link);
            }
            out_lines.push(line);
            out_map.push(src);
            continue;
        }
        let rows = wrap_graphemes(&flatten_graphemes(&line.0), width);
        let base = out_lines.len();
        for row in &rows {
            out_lines.push(Spans::from(row.spans.clone()));
            out_map.push(src);
        }
        for link in line_links {
            let mapped = map_link_to_rows(&rows, link.start_col as usize, link.end_col as usize);
            if mapped.is_empty() {
                out_links.push(MarkdownLink {
                    line: base,
                    dest: link.dest,
                    start_col: link.start_col,
                    end_col: link.end_col,
                });
            } else {
                for (ri, vs, ve) in mapped {
                    out_links.push(MarkdownLink {
                        line: base + ri,
                        dest: link.dest.clone(),
                        start_col: vs,
                        end_col: ve,
                    });
                }
            }
        }
    }
    (out_lines, out_map, out_links)
}

impl Component for Markdown {
    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        use tui::widgets::{Paragraph, Widget, Wrap};

        let text = self.parse(Some(&cx.editor.theme));

        let par = Paragraph::new(&text)
            .wrap(Wrap { trim: false })
            .scroll((cx.scroll.unwrap_or_default() as u16, 0));

        let margin = Margin::all(1);
        par.render(area.inner(margin), surface);
    }

    fn required_size(&mut self, viewport: (u16, u16)) -> Option<(u16, u16)> {
        let padding = 2;
        let contents = self.parse(None);

        // TODO: account for tab width
        let max_text_width = (viewport.0.saturating_sub(padding)).min(120);
        let (width, height) = crate::ui::text::required_size(&contents, max_text_width);

        Some((width + padding, height + padding))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_swap::ArcSwap;

    fn stringify(text: &Text) -> Vec<String> {
        text.lines
            .iter()
            .map(|spans| {
                spans
                    .0
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    fn parse_preview(
        md: &str,
        wrap_width: u16,
    ) -> (Vec<String>, Vec<Option<usize>>, Vec<MarkdownLink>) {
        let loader = Arc::new(ArcSwap::from_pointee(syntax::Loader::default()));
        let markdown = Markdown::new(md.to_string(), loader);
        let (text, map, links) = markdown.parse_with_map(None, wrap_width, false);
        (stringify(&text), map, links)
    }

    fn render_lines(md: &str) -> Vec<String> {
        parse_preview(md, 0).0
    }

    #[test]
    fn table_columns_are_aligned() {
        let md = "\
| Name | Age | City |
| --- | --- | --- |
| Alice | 30 | New York |
| Bob | 5 | LA |
";
        let lines = render_lines(md);
        let table: Vec<&String> = lines.iter().filter(|l| !l.trim().is_empty()).collect();

        // Header, divider, two body rows.
        assert_eq!(table.len(), 4, "unexpected lines: {lines:?}");
        assert_eq!(table[0], "Name  │ Age │ City    ");
        assert_eq!(table[1], "──────┼─────┼─────────");
        assert_eq!(table[2], "Alice │ 30  │ New York");
        assert_eq!(table[3], "Bob   │ 5   │ LA      ");

        // Every row renders to the same display width -> columns line up.
        let widths: Vec<usize> = table.iter().map(|l| l.chars().count()).collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "rows differ in width: {widths:?}"
        );
    }

    #[test]
    fn table_respects_column_alignment() {
        let md = "\
| Left | Mid | Right |
| :-- | :-: | --: |
| a | b | c |
";
        let lines = render_lines(md);
        let row = lines
            .iter()
            .find(|l| l.contains('a'))
            .expect("body row present");
        // col0 width 4 (left), col1 width 3 (center), col2 width 5 (right):
        // left stays flush-left, center pads both sides, right is flush-right.
        assert_eq!(row, "a    │  b  │     c");
    }

    #[test]
    fn table_pads_ragged_rows() {
        let md = "\
| A | B | C |
| --- | --- | --- |
| only |
| x | y |
";
        let lines = render_lines(md);
        let table: Vec<&String> = lines.iter().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(table.len(), 4, "unexpected lines: {lines:?}");
        let widths: Vec<usize> = table.iter().map(|l| l.chars().count()).collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "ragged rows differ in width: {widths:?} {table:?}"
        );
        assert!(table[2].contains("only"), "{table:?}");
        assert!(
            table[3].contains('x') && table[3].contains('y'),
            "{table:?}"
        );
    }

    #[test]
    fn line_map_tracks_header_divider_and_body() {
        let md = "\
# Title

| Name | Age |
| --- | --- |
| Alice | 30 |
";
        let (lines, map, _) = parse_preview(md, 0);
        assert_eq!(
            lines.len(),
            map.len(),
            "lines and line_map must stay in lockstep"
        );
        // Title is source line 0; table header is source line 2; divider has no source.
        let title = lines
            .iter()
            .position(|l| l.contains("Title"))
            .expect("title");
        assert_eq!(map[title], Some(0));
        let header = lines
            .iter()
            .position(|l| l.starts_with("Name"))
            .expect("header");
        assert_eq!(map[header], Some(2));
        assert_eq!(
            map[header + 1],
            None,
            "divider should not map to a source line"
        );
        let body = lines
            .iter()
            .position(|l| l.contains("Alice"))
            .expect("body");
        assert_eq!(map[body], Some(4));
    }

    #[test]
    fn link_is_anchored_to_rendered_line() {
        let md = "see [docs](README.md) please\n";
        let (lines, _, links) = parse_preview(md, 0);
        let line = lines
            .iter()
            .position(|l| l.contains("docs"))
            .expect("link text");
        let link = links
            .iter()
            .find(|l| l.dest == "README.md")
            .expect("link dest stored");
        assert_eq!(link.line, line);
        assert!(link.end_col > link.start_col, "{link:?}");
        let rendered = &lines[line];
        let text = "docs";
        let start = rendered.find(text).unwrap();
        assert_eq!(link.start_col as usize, start);
        assert_eq!(link.end_col as usize, start + text.len());
    }

    #[test]
    fn table_cell_wraps_to_width() {
        let md = "\
| Left | Right |
| --- | --- |
| a-long-cell-value | short |
";
        let (lines, _, _) = parse_preview(md, 18);
        let table: Vec<&String> = lines.iter().filter(|l| !l.trim().is_empty()).collect();
        assert!(
            table.iter().all(|l| l.chars().count() <= 18),
            "wrapped table exceeded width: {table:?}"
        );
        let joined = table
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined.contains("a-long-cell") && joined.contains("value"),
            "expected the long cell to wrap onto extra rows: {lines:?}"
        );
        assert!(
            table.len() > 3,
            "expected extra wrap rows beyond header/divider/body: {lines:?}"
        );
    }

    #[test]
    fn table_stacks_when_too_narrow() {
        let md = "\
| Name | Age | City | Country |
| --- | --- | --- | --- |
| Alice | 30 | New York | USA |
";
        let (lines, map, _) = parse_preview(md, 12);
        let stacked: Vec<&String> = lines
            .iter()
            .filter(|l| {
                l.contains("Alice") || l.contains("30") || l.contains("York") || l.contains("USA")
            })
            .collect();
        assert!(
            stacked
                .iter()
                .any(|l| l.contains("Name:") && l.contains("Alice")),
            "expected stacked Col: value fallback, got {lines:?}"
        );
        assert!(
            stacked.iter().all(|l| l.chars().count() <= 12),
            "stacked rows exceeded width: {stacked:?}"
        );
        let alice_idx = lines.iter().position(|l| l.contains("Alice")).unwrap();
        assert_eq!(map[alice_idx], Some(2));
    }

    #[test]
    fn table_cell_link_is_anchored() {
        let md = "\
| A | B |
| --- | --- |
| [docs](README.md) | x |
";
        let (lines, _, links) = parse_preview(md, 0);
        let line = lines
            .iter()
            .position(|l| l.contains("docs"))
            .expect("link text in table");
        let link = links
            .iter()
            .find(|l| l.dest == "README.md")
            .expect("table cell dest stored");
        assert_eq!(link.line, line);
        assert!(link.end_col > link.start_col, "{link:?} lines={lines:?}");
    }

    #[test]
    fn popup_parse_does_not_align_tables() {
        let md = "\
| Name | Age |
| --- | --- |
| Alice | 30 |
";
        let loader = Arc::new(ArcSwap::from_pointee(syntax::Loader::default()));
        let markdown = Markdown::new(md.to_string(), loader);
        let lines = stringify(&markdown.parse(None));
        assert!(
            lines.iter().any(|l| l.contains('|')),
            "popup parse should keep table syntax as text, got {lines:?}"
        );
        assert!(
            lines.iter().all(|l| !l.contains('│')),
            "popup parse should not emit aligned table columns, got {lines:?}"
        );
    }
}
