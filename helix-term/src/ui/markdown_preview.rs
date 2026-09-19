use std::path::{Path, PathBuf};

use crate::compositor::{Callback, Component, Compositor, Context, Event, EventResult};
use crate::ui::{file_picker, overlay::overlaid, Markdown, MarkdownLink};
use crate::{ctrl, key};

use helix_core::line_ending::line_end_char_index;
use helix_core::{Position, Selection};
use helix_stdx::path;
use helix_view::{
    align_view,
    document::Mode,
    editor::Action,
    graphics::{CursorKind, Rect},
    input::{KeyEvent, MouseButton, MouseEvent, MouseEventKind},
    Align, DocumentId, Editor, ViewId,
};
use tui::text::{Span, Spans, Text};
use url::Url;

use tui::{
    buffer::Buffer as Surface,
    widgets::{Block, Paragraph, Widget},
};

/// Output rows scrolled per mouse-wheel notch.
const WHEEL_LINES: usize = 3;

/// Keep this many rows of context above the source cursor's mapped line.
const TOP_MARGIN: usize = 3;

/// Wide enough to show the source on the left and the preview on the right.
const MIN_WIDTH_FOR_SIDE_BY_SIDE: u16 = 100;

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
        let max = self.total_rows().saturating_sub(1) as isize;
        self.scroll = (self.scroll as isize + delta).clamp(0, max) as usize;
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

    fn goto_source(&self, rendered_line: usize) -> EventResult {
        match self.source_line_for_rendered(rendered_line) {
            Some(src_line) => {
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

    fn handle_key(&mut self, event: KeyEvent, mode: Mode) -> EventResult {
        if mode == Mode::Normal {
            match event {
                key!(Esc) | key!('q') | ctrl!('c') => return self.close(),
                key!(Enter) => {
                    return self
                        .follow_link_at(self.cursor_line, None)
                        .unwrap_or(EventResult::Ignored(None));
                }
                _ => {}
            }
        }
        EventResult::Ignored(None)
    }

    fn handle_mouse(&mut self, event: &MouseEvent) -> EventResult {
        let within = self.area.width > 0
            && event.column >= self.area.x
            && event.column < self.area.right()
            && event.row >= self.area.y
            && event.row < self.area.bottom();

        if !within {
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
                let output_row = self.scroll + (event.row - self.area.y) as usize;
                if output_row >= self.total_rows() {
                    return EventResult::Consumed(None);
                }
                let rendered_line = self.rendered_line_at_row(output_row);
                self.cursor_line = rendered_line;
                let col = event.column.saturating_sub(self.area.x);
                self.follow_link_at(rendered_line, Some(col))
                    .unwrap_or_else(|| self.goto_source(rendered_line))
            }
            _ => EventResult::Consumed(None),
        }
    }
}

impl Component for MarkdownPreview {
    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        match event {
            Event::Key(key) => self.handle_key(*key, cx.editor.mode),
            Event::Mouse(mouse) => self.handle_mouse(mouse),
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

        let title = if side_by_side {
            "Markdown preview  (q/Esc close · Enter follow link)"
        } else {
            "Markdown preview  (q/Esc close · navigate source · Enter follow link)"
        };
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
                self.last_source_line = Some(source_line);
                if let Some(rendered) = self.rendered_line_for_source(source_line) {
                    self.cursor_line = rendered;
                    self.scroll = self.row_starts[rendered].saturating_sub(TOP_MARGIN);
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

        let line_start = self.row_starts.get(self.cursor_line).copied().unwrap_or(0);
        let line_end = self
            .row_starts
            .get(self.cursor_line + 1)
            .copied()
            .unwrap_or(line_start);
        let visible_start = line_start.max(self.scroll);
        let visible_end = line_end.min(self.scroll + inner.height as usize);
        if visible_start < visible_end {
            let style = cx
                .editor
                .theme
                .try_get("ui.cursorline.primary")
                .unwrap_or_else(|| cx.editor.theme.get("ui.selection"));
            let row = inner.y + (visible_start - self.scroll) as u16;
            let height = (visible_end - visible_start) as u16;
            surface.set_style(Rect::new(inner.x, row, inner.width, height), style);
        }
    }

    fn id(&self) -> Option<&'static str> {
        Some(Self::ID)
    }
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
