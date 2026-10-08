use std::path::{Path, PathBuf};

use crate::compositor::{Callback, Component, Compositor, Context, Event, EventResult};
use crate::ui::{file_picker, overlay::overlaid, Markdown, MarkdownLink};
use crate::{ctrl, key};

use helix_core::line_ending::line_end_char_index;
use helix_core::unicode::segmentation::UnicodeSegmentation;
use helix_core::unicode::width::UnicodeWidthStr;
use helix_core::{Position, Selection};
use helix_stdx::path;
use helix_view::{
    align_view,
    document::Mode,
    editor::Action,
    graphics::{Color, CursorKind, Rect, Style},
    input::{KeyEvent, MouseButton, MouseEvent, MouseEventKind},
    theme::Modifier,
    Align, DocumentId, Editor, Theme, ViewId,
};
use tui::text::{Span, Spans, Text};
use url::Url;

use tui::{
    buffer::Buffer as Surface,
    widgets::{Block, Paragraph, Widget},
};

/// Output rows scrolled per mouse-wheel notch.
const WHEEL_LINES: usize = 1;

/// Keep this many rows of context above the source cursor's mapped line.
const TOP_MARGIN: usize = 3;

/// Wide enough to show the source on the left and the preview on the right.
const MIN_WIDTH_FOR_SIDE_BY_SIDE: u16 = 100;

/// A cell in the rendered output: a rendered line and a display column on it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Pos {
    line: usize,
    col: u16,
}

/// A left-button press inside the panel that may still turn into a drag. Only
/// a press that never moved counts as a click, so dragging across a link
/// selects its text instead of following it.
struct Press {
    at: Pos,
    dragged: bool,
}

struct PreviewCache {
    version: i32,
    theme: String,
    width: u16,
    soft_wrap: bool,
    text: Text<'static>,
    line_map: Vec<Option<usize>>,
    links: Vec<MarkdownLink>,
}

/// Read-only markdown overlay. The source view stays focused; q/Esc close.
pub struct MarkdownPreview {
    source_view: ViewId,
    source_doc: DocumentId,
    base_dir: PathBuf,
    scroll: usize,
    cursor_line: usize,
    last_source_line: Option<usize>,
    line_map: Vec<Option<usize>>,
    row_starts: Vec<usize>,
    links: Vec<MarkdownLink>,
    total_lines: usize,
    area: Rect,
    /// `:markdown-preview split` asked for a side-by-side overlay.
    want_split: bool,
    /// True while the preview is actually painted on the right half.
    side_by_side: bool,
    /// Left button held down inside the panel.
    press: Option<Press>,
    /// Text selection as (anchor, head); either may come first on screen. The
    /// cell under `head` is part of the selection.
    selection: Option<(Pos, Pos)>,
    cache: Option<PreviewCache>,
}

impl MarkdownPreview {
    pub const ID: &'static str = "markdown-preview";

    pub fn new(
        source_view: ViewId,
        source_doc: DocumentId,
        base_dir: PathBuf,
        want_split: bool,
    ) -> Self {
        Self {
            source_view,
            source_doc,
            base_dir,
            scroll: 0,
            cursor_line: 0,
            last_source_line: None,
            line_map: Vec::new(),
            row_starts: Vec::new(),
            links: Vec::new(),
            total_lines: 0,
            area: Rect::default(),
            want_split,
            side_by_side: false,
            press: None,
            selection: None,
            cache: None,
        }
    }

    fn source_cursor_line(&self, editor: &Editor) -> Option<usize> {
        if !editor.tree.contains(self.source_view) {
            return None;
        }
        let doc = editor.document(self.source_doc)?;
        let text = doc.text().slice(..);
        let cursor = doc.selection(self.source_view).primary().cursor(text);
        Some(text.char_to_line(cursor))
    }

    fn rendered_line_for_source(&self, source_line: usize) -> Option<usize> {
        let mut best = None;
        let mut best_diff = usize::MAX;
        for (i, mapped) in self.line_map.iter().enumerate() {
            if let Some(src) = *mapped {
                let diff = src.abs_diff(source_line);
                if diff < best_diff {
                    best_diff = diff;
                    best = Some(i);
                }
                if diff == 0 {
                    break;
                }
            }
        }
        best
    }

    fn source_line_for_rendered(&self, rendered_line: usize) -> Option<usize> {
        for delta in 0..self.line_map.len().max(1) {
            if let Some(Some(src)) = self.line_map.get(rendered_line + delta) {
                return Some(*src);
            }
            if delta > 0 {
                if let Some(idx) = rendered_line.checked_sub(delta) {
                    if let Some(Some(src)) = self.line_map.get(idx) {
                        return Some(*src);
                    }
                }
            }
        }
        None
    }

    fn total_rows(&self) -> usize {
        self.row_starts.last().copied().unwrap_or(0)
    }

    fn scroll_lines(&mut self, delta: isize) {
        let height = self.area.height.max(1) as usize;
        let max = self.total_rows().saturating_sub(height) as isize;
        self.scroll = (self.scroll as isize + delta).clamp(0, max.max(0)) as usize;
    }

    fn move_cursor(&mut self, delta: isize) -> EventResult {
        if self.total_lines == 0 {
            return EventResult::Consumed(None);
        }
        let height = self.area.height.max(1) as usize;
        let max_line = self.total_lines.saturating_sub(1);

        let cursor_row = self.row_starts.get(self.cursor_line).copied().unwrap_or(0);
        let current_line = if cursor_row < self.scroll {
            self.rendered_line_at_row(self.scroll)
        } else if cursor_row >= self.scroll + height {
            self.rendered_line_at_row((self.scroll + height).saturating_sub(1))
        } else {
            self.cursor_line
        };

        let new_line = (current_line as isize + delta).clamp(0, max_line as isize) as usize;
        self.cursor_line = new_line;
        self.selection = None;

        let margin = TOP_MARGIN.min(height.saturating_sub(1) / 2);
        let line_start = self.row_starts.get(new_line).copied().unwrap_or(0);
        let line_end = self
            .row_starts
            .get(new_line + 1)
            .copied()
            .unwrap_or(line_start + 1);

        if line_start < self.scroll + margin {
            self.scroll = line_start.saturating_sub(margin);
        } else if line_end + margin > self.scroll + height {
            self.scroll = (line_end + margin).saturating_sub(height);
        }
        let max_scroll = self.total_rows().saturating_sub(height);
        self.scroll = self.scroll.min(max_scroll);

        if let Some(src_line) = self.source_line_for_rendered(new_line) {
            self.last_source_line = Some(src_line);
            let view = self.source_view;
            let doc = self.source_doc;
            EventResult::Consumed(Some(Box::new(move |_compositor, cx: &mut Context| {
                goto_source_line(cx.editor, view, doc, src_line, false);
            })))
        } else {
            EventResult::Consumed(None)
        }
    }

    fn rendered_line_at_row(&self, output_row: usize) -> usize {
        if self.total_lines == 0 {
            return 0;
        }
        let idx = match self.row_starts.binary_search(&output_row) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        idx.min(self.total_lines - 1)
    }

    fn goto_source(&mut self, rendered_line: usize) -> EventResult {
        match self.source_line_for_rendered(rendered_line) {
            Some(src_line) => {
                self.last_source_line = Some(src_line);
                let view = self.source_view;
                let doc = self.source_doc;
                EventResult::Consumed(Some(Box::new(move |_compositor, cx: &mut Context| {
                    goto_source_line(cx.editor, view, doc, src_line, false);
                })))
            }
            None => EventResult::Consumed(None),
        }
    }

    fn follow_link_at(&self, rendered_line: usize, col: Option<u16>) -> Option<EventResult> {
        let link = self.links.iter().find(|link| {
            if link.line != rendered_line {
                return false;
            }
            match col {
                Some(c) => c >= link.start_col && c < link.end_col,
                None => true,
            }
        })?;
        let dest = link.dest.clone();
        let base_dir = self.base_dir.clone();
        Some(EventResult::Consumed(Some(Box::new(
            move |compositor: &mut Compositor, cx: &mut Context| {
                if open_link(compositor, cx, &dest, &base_dir) {
                    compositor.remove(Self::ID);
                }
            },
        ))))
    }

    fn rendered_line(&self, line: usize) -> Option<&Spans<'static>> {
        self.cache.as_ref()?.text.lines.get(line)
    }

    /// Display columns a rendered line occupies, clipped to the panel: the
    /// paragraph is drawn without horizontal scrolling, so nothing past the
    /// panel width is on screen to select.
    fn line_width(&self, line: usize) -> u16 {
        let width = self.rendered_line(line).map_or(0, Spans::width);
        width.min(self.area.width as usize) as u16
    }

    /// The cell a mouse event landed on, clamped into the panel so a drag that
    /// has left it keeps extending from the nearest edge.
    fn pos_at(&self, row: u16, column: u16) -> Pos {
        let row = row.clamp(self.area.y, self.area.bottom().saturating_sub(1));
        let output_row = self.scroll + (row - self.area.y) as usize;
        let last_row = self.total_rows().saturating_sub(1);
        let line = self.rendered_line_at_row(output_row.min(last_row));
        let col = column.saturating_sub(self.area.x).min(self.area.width);
        Pos { line, col }
    }

    /// The selection as a half-open range in render order. The cell the pointer
    /// stopped on is included, the way a terminal's own selection behaves.
    fn ordered_selection(&self) -> Option<(Pos, Pos)> {
        let (anchor, head) = self.selection?;
        let (mut start, mut end) = if anchor <= head {
            (anchor, head)
        } else {
            (head, anchor)
        };
        end.col = end.col.saturating_add(1);
        start.col = start.col.min(self.line_width(start.line));
        Some((start, end))
    }

    /// Selected display columns `[start, end)` on a rendered line. Lines before
    /// the last run to their end, so a multi-line selection takes whole lines.
    fn selected_cols(&self, line: usize) -> Option<(u16, u16)> {
        let (start, end) = self.ordered_selection()?;
        if line < start.line || line > end.line {
            return None;
        }
        let from = if line == start.line { start.col } else { 0 };
        let width = self.line_width(line);
        let to = if line == end.line {
            end.col.min(width)
        } else {
            width
        };
        (from < to).then_some((from, to))
    }

    /// The selected text, one rendered line per output line.
    fn selection_text(&self) -> Option<String> {
        let (start, end) = self.ordered_selection()?;
        let mut out = String::new();
        for line in start.line..=end.line.min(self.total_lines.saturating_sub(1)) {
            if line > start.line {
                out.push('\n');
            }
            if let (Some(spans), Some((from, to))) =
                (self.rendered_line(line), self.selected_cols(line))
            {
                out.push_str(slice_cols(spans, from, to).trim_end());
            }
        }
        (!out.trim().is_empty()).then_some(out)
    }

    /// Put the selection, or the cursor line when there is none, on the system
    /// clipboard.
    fn copy(&self, editor: &mut Editor) -> bool {
        let text = match self.selection {
            Some(_) => self.selection_text(),
            None => self
                .rendered_line(self.cursor_line)
                .map(|spans| slice_cols(spans, 0, self.line_width(self.cursor_line))),
        };
        let Some(text) = text.filter(|text| !text.trim().is_empty()) else {
            editor.set_error("Nothing to copy");
            return false;
        };
        let lines = text.lines().count().max(1);
        match editor.registers.write('+', vec![text]) {
            Ok(()) => {
                editor.set_status(format!("Copied {lines} lines to the clipboard"));
                true
            }
            Err(err) => {
                editor.set_error(err.to_string());
                false
            }
        }
    }

    /// Highlighted line if it is on screen, otherwise the line at the viewport center.
    fn close_target_source_line(&self) -> Option<usize> {
        if self.total_lines == 0 {
            return None;
        }
        let visible_end = self.scroll + self.area.height.max(1) as usize;
        let cursor_row = self.row_starts.get(self.cursor_line).copied().unwrap_or(0);
        let rendered = if cursor_row >= self.scroll && cursor_row < visible_end {
            self.cursor_line
        } else {
            let center = self.scroll + (self.area.height as usize) / 2;
            self.rendered_line_at_row(center.min(self.total_rows().saturating_sub(1)))
        };
        self.source_line_for_rendered(rendered)
    }

    fn close(&self) -> EventResult {
        let view = self.source_view;
        let doc = self.source_doc;
        let line = self.close_target_source_line();
        let close: Callback = Box::new(move |compositor, cx| {
            compositor.remove(Self::ID);
            if let Some(line) = line {
                goto_source_line(cx.editor, view, doc, line, true);
            }
        });
        EventResult::Consumed(Some(close))
    }

    fn handle_key(&mut self, event: KeyEvent, cx: &mut Context) -> EventResult {
        if cx.editor.mode == Mode::Normal {
            match event {
                // A standing selection is what Esc clears first; the reader has
                // to ask twice to lose the preview itself.
                key!(Esc) if self.selection.is_some() => {
                    self.selection = None;
                    return EventResult::Consumed(None);
                }
                key!(Esc) | key!('q') | ctrl!('c') => return self.close(),
                key!(Enter) => {
                    return self
                        .follow_link_at(self.cursor_line, None)
                        .unwrap_or(EventResult::Ignored(None));
                }
                key!('y') => {
                    if self.copy(cx.editor) {
                        // The selection has served its purpose; leaving it
                        // standing would make the next copy take something the
                        // reader has stopped pointing at.
                        self.selection = None;
                    }
                    return EventResult::Consumed(None);
                }
                key!('j') | key!(Down) => return self.move_cursor(1),
                key!('k') | key!(Up) => return self.move_cursor(-1),
                ctrl!('e') => {
                    self.scroll_lines(1);
                    return EventResult::Consumed(None);
                }
                ctrl!('y') => {
                    self.scroll_lines(-1);
                    return EventResult::Consumed(None);
                }
                ctrl!('d') | key!(PageDown) | ctrl!('f') => {
                    let half = (self.area.height as isize / 2).max(1);
                    return self.move_cursor(half);
                }
                ctrl!('u') | key!(PageUp) | ctrl!('b') => {
                    let half = (self.area.height as isize / 2).max(1);
                    return self.move_cursor(-half);
                }
                key!(Home) => {
                    let total = self.total_lines as isize;
                    return self.move_cursor(-total);
                }
                key!(End) => {
                    let total = self.total_lines as isize;
                    return self.move_cursor(total);
                }
                _ => {}
            }
        }
        EventResult::Ignored(None)
    }

    fn handle_mouse(&mut self, event: &MouseEvent, cx: &mut Context) -> EventResult {
        let within = self.area.width > 0
            && event.column >= self.area.x
            && event.column < self.area.right()
            && event.row >= self.area.y
            && event.row < self.area.bottom();

        // A drag that has wandered off the panel still belongs to the press
        // that started on it -- otherwise it would select the source document
        // underneath instead of extending the selection here.
        let owns_drag = self.press.is_some()
            && matches!(
                event.kind,
                MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
            );

        if !within && !owns_drag {
            return if self.side_by_side {
                EventResult::Ignored(None)
            } else {
                EventResult::Consumed(None)
            };
        }

        match event.kind {
            MouseEventKind::ScrollDown => {
                self.scroll_lines(WHEEL_LINES as isize);
                EventResult::Consumed(None)
            }
            MouseEventKind::ScrollUp => {
                self.scroll_lines(-(WHEEL_LINES as isize));
                EventResult::Consumed(None)
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.selection = None;
                self.press = Some(Press {
                    at: self.pos_at(event.row, event.column),
                    dragged: false,
                });
                // What a press means is only known once the button comes back
                // up: a click follows a link, a drag selects text.
                EventResult::Consumed(None)
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                // Dragging past an edge scrolls, so a selection can run past
                // what is on screen.
                if event.row < self.area.y {
                    self.scroll_lines(-1);
                } else if event.row >= self.area.bottom() {
                    self.scroll_lines(1);
                }
                let head = self.pos_at(event.row, event.column);
                if let Some(press) = self.press.as_mut() {
                    // A wobble that never leaves the pressed cell is still a
                    // click, so a link does not need a perfectly still hand.
                    press.dragged |= head != press.at;
                    if press.dragged {
                        let anchor = press.at;
                        self.selection = Some((anchor, head));
                    }
                }
                EventResult::Consumed(None)
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let Some(press) = self.press.take() else {
                    return EventResult::Consumed(None);
                };
                if press.dragged {
                    // Copy-on-release, as the terminal's own selection would
                    // do if the preview were not holding the mouse. The
                    // highlight stays up to show what was taken.
                    self.copy(cx.editor);
                    return EventResult::Consumed(None);
                }
                self.click(press.at)
            }
            _ => EventResult::Consumed(None),
        }
    }

    /// A press that never moved: point the cursor there and act on whatever it
    /// landed on.
    fn click(&mut self, at: Pos) -> EventResult {
        if at.line >= self.total_lines {
            return EventResult::Consumed(None);
        }
        self.cursor_line = at.line;
        self.follow_link_at(at.line, Some(at.col))
            .unwrap_or_else(|| self.goto_source(at.line))
    }
}

impl Component for MarkdownPreview {
    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        match event {
            Event::Key(key) => self.handle_key(*key, cx),
            Event::Mouse(mouse) => self.handle_mouse(mouse, cx),
            _ => EventResult::Ignored(None),
        }
    }

    fn cursor(&self, _area: Rect, _ctx: &Editor) -> (Option<Position>, CursorKind) {
        // Must return Some so Compositor::cursor does not fall through to EditorView.
        let row_off = self
            .row_starts
            .get(self.cursor_line)
            .copied()
            .unwrap_or(0)
            .saturating_sub(self.scroll);
        let max_row = self.area.height.saturating_sub(1) as usize;
        let row = (self.area.y as usize).saturating_add(row_off.min(max_row));
        (
            Some(Position {
                row,
                col: self.area.x as usize,
            }),
            CursorKind::Hidden,
        )
    }

    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        let area = area.clip_bottom(1);
        let side_by_side = self.want_split && area.width >= MIN_WIDTH_FOR_SIDE_BY_SIDE;
        let panel = if side_by_side {
            area.clip_left(area.width / 2)
        } else {
            area
        };
        self.side_by_side = side_by_side;

        let title =
            "Markdown preview  (q/Esc close · j/k move · Enter follow link · drag or y to copy)";
        let block = Block::bordered().title(title);
        let inner = block.inner(panel);
        surface.clear_with(panel, cx.editor.theme.get("ui.popup"));
        block.render(panel, surface);
        self.area = inner;

        let theme_name = cx.editor.theme.name().to_string();
        let syn_loader = cx.editor.syn_loader.clone();
        let (contents, version, wrap_width, soft_wrap) = match cx.editor.document(self.source_doc) {
            Some(doc) => {
                let fmt = doc.text_format(inner.width, Some(&cx.editor.theme));
                (
                    doc.text().to_string(),
                    doc.version(),
                    fmt.viewport_width,
                    fmt.soft_wrap,
                )
            }
            None => (String::new(), 0, inner.width, false),
        };

        let cache_hit = self.cache.as_ref().is_some_and(|cache| {
            cache.version == version
                && cache.theme == theme_name
                && cache.width == wrap_width
                && cache.soft_wrap == soft_wrap
        });
        if !cache_hit {
            let markdown = Markdown::new(contents, syn_loader);
            let (text, line_map, links) =
                markdown.parse_with_map(Some(&cx.editor.theme), wrap_width, soft_wrap);
            self.cache = Some(PreviewCache {
                version,
                theme: theme_name,
                width: wrap_width,
                soft_wrap,
                text: own_text(text),
                line_map,
                links,
            });
        }

        {
            let cache = self.cache.as_ref().unwrap();
            self.total_lines = cache.text.lines.len();
            self.line_map.clone_from(&cache.line_map);
            self.links.clone_from(&cache.links);
        }
        self.row_starts = (0..=self.total_lines).collect();

        if let Some(source_line) = self.source_cursor_line(cx.editor) {
            if self.last_source_line != Some(source_line) {
                let first_open = self.last_source_line.is_none();
                self.last_source_line = Some(source_line);
                if let Some(rendered) = self.rendered_line_for_source(source_line) {
                    self.cursor_line = rendered;
                    let target_start = self.row_starts.get(rendered).copied().unwrap_or(0);
                    let target_end = self
                        .row_starts
                        .get(rendered + 1)
                        .copied()
                        .unwrap_or(target_start + 1);
                    if first_open {
                        self.scroll = target_start.saturating_sub(TOP_MARGIN);
                    } else {
                        let height = inner.height as usize;
                        let margin = TOP_MARGIN.min(height.saturating_sub(1) / 2);
                        if target_start < self.scroll + margin {
                            self.scroll = target_start.saturating_sub(margin);
                        } else if target_end + margin > self.scroll + height {
                            self.scroll = (target_end + margin).saturating_sub(height);
                        }
                    }
                }
            }
        }

        let max_scroll = self.total_rows().saturating_sub(inner.height as usize);
        self.scroll = self.scroll.min(max_scroll);
        self.cursor_line = self.cursor_line.min(self.total_lines.saturating_sub(1));

        {
            let text = &self.cache.as_ref().unwrap().text;
            let paragraph = Paragraph::new(text).scroll((self.scroll as u16, 0));
            paragraph.render(inner, surface);
        }

        let popup_bg = cx.editor.theme.get("ui.popup").bg;
        let cursor_style = distinct_style(
            &cx.editor.theme,
            &["ui.cursorline.primary", "ui.selection", "ui.menu.selected"],
            &[popup_bg],
        );

        let line_start = self.row_starts.get(self.cursor_line).copied().unwrap_or(0);
        let line_end = self
            .row_starts
            .get(self.cursor_line + 1)
            .copied()
            .unwrap_or(line_start);
        let visible_start = line_start.max(self.scroll);
        let visible_end = line_end.min(self.scroll + inner.height as usize);
        if visible_start < visible_end {
            let row = inner.y + (visible_start - self.scroll) as u16;
            let height = (visible_end - visible_start) as u16;
            surface.set_style(Rect::new(inner.x, row, inner.width, height), cursor_style);
        }

        // Drawn over the cursor line, so a selection that covers it still reads
        // as the selection.
        if self.selection.is_some() {
            let selection_style = distinct_style(
                &cx.editor.theme,
                &["ui.selection", "ui.menu.selected", "ui.cursorline.primary"],
                &[popup_bg, cursor_style.bg],
            );
            let visible_end = self.scroll + inner.height as usize;
            for row in self.scroll..visible_end.min(self.total_rows()) {
                let line = self.rendered_line_at_row(row);
                let Some((start, end)) = self.selected_cols(line) else {
                    continue;
                };
                let width = end.saturating_sub(start).min(inner.width.saturating_sub(start));
                let area = Rect::new(inner.x + start, inner.y + (row - self.scroll) as u16, width, 1);
                surface.set_style(area, selection_style);
            }
        }
    }

    fn id(&self) -> Option<&'static str> {
        Some(Self::ID)
    }
}

/// Plain text of a rendered line's display columns `[from, to)`. A wide
/// grapheme belongs to the column it starts in.
fn slice_cols(spans: &Spans, from: u16, to: u16) -> String {
    let (from, to) = (from as usize, to as usize);
    let mut out = String::new();
    let mut col = 0usize;
    for span in &spans.0 {
        for grapheme in span.content.as_ref().graphemes(true) {
            if col >= to {
                return out;
            }
            if col >= from {
                out.push_str(grapheme);
            }
            col += grapheme.width().max(1);
        }
    }
    out
}

/// The first of `keys` the theme paints with a background of its own that no
/// colour in `taken` already uses, so a highlight cannot come out invisible --
/// `github_dark`, for one, paints `ui.cursorline.primary` and `ui.popup` the
/// same shade, which would hide the cursor line inside the preview. Falls back
/// to inverting the line, which is visible under any theme.
fn distinct_style(theme: &Theme, keys: &[&str], taken: &[Option<Color>]) -> Style {
    keys.iter()
        .filter_map(|key| theme.try_get(key))
        .find(|style| style.bg.is_some() && !taken.contains(&style.bg))
        .unwrap_or_else(|| Style::default().add_modifier(Modifier::REVERSED))
}

fn own_text(text: Text<'_>) -> Text<'static> {
    Text {
        lines: text
            .lines
            .into_iter()
            .map(|spans| {
                Spans(
                    spans
                        .0
                        .into_iter()
                        .map(|span| Span::styled(span.content.into_owned(), span.style))
                        .collect(),
                )
            })
            .collect(),
    }
}

fn goto_source_line(
    editor: &mut Editor,
    view_id: ViewId,
    doc_id: DocumentId,
    line: usize,
    preserve_col: bool,
) {
    if editor.document(doc_id).is_none() || !editor.tree.contains(view_id) {
        return;
    }
    {
        let view = editor.tree.get_mut(view_id);
        if view.doc != doc_id {
            view.doc = doc_id;
        }
    }
    let Some(doc) = editor.documents.get_mut(&doc_id) else {
        return;
    };
    doc.ensure_view_init(view_id);

    let pos = {
        let text = doc.text();
        if text.len_chars() == 0 {
            0
        } else {
            let line = line.min(text.len_lines().saturating_sub(1));
            let line_start = text.line_to_char(line);
            if preserve_col {
                let cursor = doc.selection(view_id).primary().cursor(text.slice(..));
                let cursor = cursor.min(text.len_chars().saturating_sub(1));
                let old_line = text.char_to_line(cursor);
                let old_start = text.line_to_char(old_line);
                let col = cursor.saturating_sub(old_start);
                let line_end = line_end_char_index(&text.slice(..), line);
                line_start + col.min(line_end.saturating_sub(line_start))
            } else {
                line_start
            }
        }
    };
    doc.set_selection(view_id, Selection::point(pos));
    align_view(doc, editor.tree.get(view_id), Align::Center);
}

/// Open a markdown link destination. Returns `true` when the editor navigated.
fn open_link(compositor: &mut Compositor, cx: &mut Context, dest: &str, base_dir: &Path) -> bool {
    let dest = dest.trim();
    if dest.is_empty() {
        return false;
    }

    if let Ok(url) = Url::parse(dest) {
        if url.scheme() == "file" {
            return open_path_in_editor(compositor, cx, &PathBuf::from(url.path()));
        }
        cx.jobs.callback(crate::open_external_url_callback(url));
        return false;
    }

    let path = dest.split('#').next().unwrap_or(dest);
    if path.is_empty() {
        return false;
    }
    let expanded = path::expand(path);
    open_path_in_editor(compositor, cx, &base_dir.join(expanded))
}

fn open_path_in_editor(compositor: &mut Compositor, cx: &mut Context, path: &Path) -> bool {
    if path.is_dir() {
        let picker = file_picker(cx.editor, path.to_path_buf());
        compositor.push(Box::new(overlaid(picker)));
        true
    } else if let Err(err) = cx.editor.open(path, Action::Replace) {
        cx.editor.set_error(format!("Open file failed: {err:?}"));
        false
    } else {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helix_view::keyboard::KeyModifiers;

    fn test_preview(total_lines: usize, height: u16) -> MarkdownPreview {
        let mut preview = MarkdownPreview::new(
            ViewId::default(),
            DocumentId::default(),
            PathBuf::new(),
            false,
        );
        preview.total_lines = total_lines;
        preview.row_starts = (0..=total_lines).collect();
        preview.area = Rect::new(0, 0, 80, height);
        preview
    }

    /// A preview whose rendered output is `lines`, as if it had just painted.
    fn preview_of(lines: &[&str]) -> MarkdownPreview {
        let mut preview = test_preview(lines.len(), lines.len() as u16);
        preview.cache = Some(PreviewCache {
            version: 0,
            theme: String::new(),
            width: preview.area.width,
            soft_wrap: false,
            text: own_text(Text::from(
                lines
                    .iter()
                    .map(|line| Spans::from(line.to_string()))
                    .collect::<Vec<_>>(),
            )),
            line_map: (0..lines.len()).map(Some).collect(),
            links: Vec::new(),
        });
        preview.line_map = (0..lines.len()).map(Some).collect();
        preview
    }

    fn theme_from(toml: &str) -> helix_view::Theme {
        toml::from_str::<toml::Value>(toml)
            .expect("valid toml")
            .into()
    }

    #[test]
    fn slice_cols_cuts_on_display_columns() {
        let spans = Spans::from("hello world".to_string());
        assert_eq!(slice_cols(&spans, 0, 5), "hello");
        assert_eq!(slice_cols(&spans, 6, 11), "world");
        assert_eq!(slice_cols(&spans, 6, 99), "world");
        assert_eq!(slice_cols(&spans, 4, 4), "");

        // A double-width grapheme belongs to the column it starts in.
        let wide = Spans::from("あb".to_string());
        assert_eq!(slice_cols(&wide, 0, 2), "あ");
        assert_eq!(slice_cols(&wide, 2, 3), "b");
    }

    #[test]
    fn drag_selects_the_cells_it_covers() {
        let mut preview = preview_of(&["first line", "second line", "third line"]);

        // Columns 6..=9 of the first line, dragged left to right.
        preview.selection = Some((Pos { line: 0, col: 6 }, Pos { line: 0, col: 9 }));
        assert_eq!(preview.selected_cols(0), Some((6, 10)));
        assert_eq!(preview.selection_text().as_deref(), Some("line"));

        // The same range dragged right to left selects the same text.
        preview.selection = Some((Pos { line: 0, col: 9 }, Pos { line: 0, col: 6 }));
        assert_eq!(preview.selection_text().as_deref(), Some("line"));
    }

    #[test]
    fn multi_line_selection_takes_whole_lines_between_the_ends() {
        let mut preview = preview_of(&["first line", "second line", "third line"]);
        preview.selection = Some((Pos { line: 0, col: 6 }, Pos { line: 2, col: 4 }));

        assert_eq!(preview.selected_cols(0), Some((6, 10)));
        assert_eq!(preview.selected_cols(1), Some((0, 11)));
        assert_eq!(preview.selected_cols(2), Some((0, 5)));
        assert_eq!(
            preview.selection_text().as_deref(),
            Some("line\nsecond line\nthird")
        );
    }

    #[test]
    fn a_press_that_never_moved_is_not_a_selection() {
        let mut preview = preview_of(&["first line", "second line"]);
        let down = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 3,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        let drag = MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 6,
            row: 1,
            ..down
        };

        preview.selection = None;
        preview.press = Some(Press {
            at: preview.pos_at(down.row, down.column),
            dragged: false,
        });
        assert!(preview.selection.is_none(), "a press alone selects nothing");

        let head = preview.pos_at(drag.row, drag.column);
        let press = preview.press.as_mut().unwrap();
        press.dragged = true;
        let anchor = press.at;
        preview.selection = Some((anchor, head));
        assert_eq!(
            preview.selection_text().as_deref(),
            Some("st line\nsecond")
        );
    }

    #[test]
    fn cursorline_falls_back_when_the_theme_hides_it() {
        // github_dark paints ui.popup and ui.cursorline.primary the same shade,
        // which would leave the cursor line invisible inside the preview.
        let theme = theme_from(
            r##"
            "ui.popup" = { bg = "#161b22" }
            "ui.cursorline.primary" = { bg = "#161b22" }
            "ui.selection" = { bg = "#0c2d6b" }
            "##,
        );
        let popup_bg = theme.get("ui.popup").bg;
        let style = distinct_style(
            &theme,
            &["ui.cursorline.primary", "ui.selection", "ui.menu.selected"],
            &[popup_bg],
        );
        assert_eq!(style.bg, theme.get("ui.selection").bg);
    }

    #[test]
    fn cursorline_keeps_a_theme_that_stands_out() {
        let theme = theme_from(
            r##"
            "ui.popup" = { bg = "#161b22" }
            "ui.cursorline.primary" = { bg = "#2d333b" }
            "ui.selection" = { bg = "#0c2d6b" }
            "##,
        );
        let popup_bg = theme.get("ui.popup").bg;
        let style = distinct_style(
            &theme,
            &["ui.cursorline.primary", "ui.selection", "ui.menu.selected"],
            &[popup_bg],
        );
        assert_eq!(style.bg, theme.get("ui.cursorline.primary").bg);
    }

    #[test]
    fn highlights_invert_when_the_theme_offers_no_background() {
        let theme = theme_from(r##""ui.text" = { fg = "#ffffff" }"##);
        let style = distinct_style(&theme, &["ui.cursorline.primary", "ui.selection"], &[None]);
        assert!(style.add_modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn selection_highlight_differs_from_the_cursor_line() {
        let theme = theme_from(
            r##"
            "ui.popup" = { bg = "#161b22" }
            "ui.cursorline.primary" = { bg = "#161b22" }
            "ui.selection" = { bg = "#0c2d6b" }
            "ui.menu.selected" = { bg = "#3fb950" }
            "##,
        );
        let popup_bg = theme.get("ui.popup").bg;
        let cursor = distinct_style(
            &theme,
            &["ui.cursorline.primary", "ui.selection", "ui.menu.selected"],
            &[popup_bg],
        );
        let selection = distinct_style(
            &theme,
            &["ui.selection", "ui.menu.selected", "ui.cursorline.primary"],
            &[popup_bg, cursor.bg],
        );
        assert_ne!(cursor.bg, selection.bg);
        assert_eq!(selection.bg, theme.get("ui.menu.selected").bg);
    }

    #[test]
    fn mouse_wheel_scrolls_single_line() {
        assert_eq!(WHEEL_LINES, 1);
        let mut preview = test_preview(50, 10);
        assert_eq!(preview.scroll, 0);

        preview.scroll_lines(WHEEL_LINES as isize);
        assert_eq!(preview.scroll, 1);

        preview.scroll_lines(WHEEL_LINES as isize);
        assert_eq!(preview.scroll, 2);

        preview.scroll_lines(-(WHEEL_LINES as isize));
        assert_eq!(preview.scroll, 1);
    }

    #[test]
    fn move_cursor_scrolls_smoothly_line_by_line() {
        let mut preview = test_preview(50, 10);
        // Start cursor at row 0, scroll at 0. Margin is TOP_MARGIN (3).
        assert_eq!(preview.cursor_line, 0);
        assert_eq!(preview.scroll, 0);

        // Moving down within the viewport: cursor moves, scroll does not jump.
        preview.move_cursor(1);
        assert_eq!(preview.cursor_line, 1);
        assert_eq!(preview.scroll, 0);

        preview.move_cursor(1);
        assert_eq!(preview.cursor_line, 2);
        assert_eq!(preview.scroll, 0);

        // Move to row 6 (line_end = 7, 7 + 3 = 10 == scroll + height). Still no scroll.
        preview.move_cursor(4);
        assert_eq!(preview.cursor_line, 6);
        assert_eq!(preview.scroll, 0);

        // Moving to row 7: 8 + 3 = 11 > 10. Scrolls down by 1 row smoothly.
        preview.move_cursor(1);
        assert_eq!(preview.cursor_line, 7);
        assert_eq!(preview.scroll, 1);

        // Move down another row: scrolls down another row smoothly.
        preview.move_cursor(1);
        assert_eq!(preview.cursor_line, 8);
        assert_eq!(preview.scroll, 2);

        // Move up: cursor moves up, scroll stays at 2 until hitting top margin.
        preview.move_cursor(-1);
        assert_eq!(preview.cursor_line, 7);
        assert_eq!(preview.scroll, 2);
    }
}

