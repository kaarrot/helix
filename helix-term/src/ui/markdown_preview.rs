use std::path::{Path, PathBuf};

use crate::compositor::{Callback, Component, Compositor, Context, Event, EventResult};
use crate::ui::{file_picker, overlay::overlaid, Markdown, MarkdownLink};
use crate::{ctrl, key};

use helix_core::Selection;
use helix_stdx::path;
use helix_view::{
    align_view,
    document::Mode,
    editor::Action,
    graphics::Rect,
    input::{KeyEvent, MouseButton, MouseEvent, MouseEventKind},
    Align, DocumentId, Editor, ViewId,
};
use url::Url;

use tui::{
    buffer::Buffer as Surface,
    widgets::{Block, Paragraph, Widget, Wrap},
};

/// Rendered lines scrolled per mouse-wheel notch.
const WHEEL_LINES: usize = 3;

/// A read-only full-screen overlay that renders a markdown document as styled
/// text on top of the editor, and follows the (hidden) source buffer: keep
/// navigating the source normally and the preview scrolls to keep the source
/// cursor's region in view. Fills the whole screen so it's usable on small
/// terminals where a side-by-side split would be too cramped.
///
/// - Navigate the source (it stays focused) -> the preview follows.
/// - Click a rendered line -> jump the source cursor to the matching source
///   line (which then re-centers the preview there).
/// - Click a line carrying a markdown link -> follow it like goto-file.
/// - Mouse wheel over the panel scrolls it locally until the source moves again.
/// - `q`/`Esc` close it; all other keys pass through to the source editor.
pub struct MarkdownPreview {
    /// The view showing the source markdown, whose cursor drives the preview.
    source_view: ViewId,
    /// The source markdown document; re-read each render so the preview is live.
    source_doc: DocumentId,
    /// Directory the source file lives in, used to resolve relative link paths.
    base_dir: PathBuf,
    /// Vertical scroll offset into the rendered lines.
    scroll: usize,
    /// Highlighted rendered line (tracks the source cursor's mapped line).
    cursor_line: usize,
    /// Last source line we synced to; the preview re-centers only when the
    /// source cursor moves to a different line, so wheel-scrolling sticks.
    last_source_line: Option<usize>,

    // Recomputed every render, read back by event handling:
    /// Rendered-line -> source-line map (`None` for inserted blank lines).
    line_map: Vec<Option<usize>>,
    /// Links anchored to rendered lines.
    links: Vec<MarkdownLink>,
    /// Number of rendered lines.
    total_lines: usize,
    /// Inner content rectangle from the last render, for mouse hit-testing.
    area: Rect,
}

impl MarkdownPreview {
    pub const ID: &'static str = "markdown-preview";

    pub fn new(source_view: ViewId, source_doc: DocumentId, base_dir: PathBuf) -> Self {
        Self {
            source_view,
            source_doc,
            base_dir,
            scroll: 0,
            cursor_line: 0,
            last_source_line: None,
            line_map: Vec::new(),
            links: Vec::new(),
            total_lines: 0,
            area: Rect::default(),
        }
    }

    /// The source line the source view's cursor is currently on, if available.
    fn source_cursor_line(&self, editor: &Editor) -> Option<usize> {
        if !editor.tree.contains(self.source_view) {
            return None;
        }
        let doc = editor.document(self.source_doc)?;
        let text = doc.text().slice(..);
        let cursor = doc.selection(self.source_view).primary().cursor(text);
        Some(text.char_to_line(cursor))
    }

    /// The rendered line closest to the given source line, or `None` if nothing
    /// mapped (e.g. an empty document).
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

    /// The source line closest to the given rendered line, searching outward so
    /// clicking a blank/separator line still targets a nearby source line.
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

    fn scroll_lines(&mut self, delta: isize) {
        let max = self.total_lines.saturating_sub(1) as isize;
        self.scroll = (self.scroll as isize + delta).clamp(0, max) as usize;
    }

    /// Jump the source view's cursor to the source line for the given rendered
    /// line; the follow-up render re-centers the preview there.
    fn goto_source(&self, rendered_line: usize) -> EventResult {
        match self.source_line_for_rendered(rendered_line) {
            Some(src_line) => {
                let view = self.source_view;
                EventResult::Consumed(Some(Box::new(move |_compositor, cx: &mut Context| {
                    goto_source_line(cx.editor, view, src_line);
                })))
            }
            None => EventResult::Consumed(None),
        }
    }

    /// Follow the markdown link anchored to the given rendered line, if any.
    fn follow_link(&self, rendered_line: usize) -> Option<EventResult> {
        let link = self.links.iter().find(|link| link.line == rendered_line)?;
        let dest = link.dest.clone();
        let base_dir = self.base_dir.clone();
        Some(EventResult::Consumed(Some(Box::new(
            move |compositor: &mut Compositor, cx: &mut Context| {
                // Close the preview only when we navigate the editor (opening a
                // file); external URLs open elsewhere and leave the preview up.
                if open_link(compositor, cx, &dest, &base_dir) {
                    compositor.remove(Self::ID);
                }
            },
        ))))
    }

    fn handle_key(&mut self, event: KeyEvent, mode: Mode) -> EventResult {
        // Only capture the close keys in normal mode; everything else passes
        // through to the source editor so it stays fully navigable. Gating on
        // normal mode keeps insert-mode editing intact — including its own Esc
        // to leave insert and a literal `q` while typing.
        if mode == Mode::Normal {
            match event {
                key!(Esc) | key!('q') | ctrl!('c') => {
                    let close: Callback = Box::new(|compositor, _| {
                        compositor.remove(Self::ID);
                    });
                    return EventResult::Consumed(Some(close));
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

        match event.kind {
            MouseEventKind::ScrollDown if within => {
                self.scroll_lines(WHEEL_LINES as isize);
                EventResult::Consumed(None)
            }
            MouseEventKind::ScrollUp if within => {
                self.scroll_lines(-(WHEEL_LINES as isize));
                EventResult::Consumed(None)
            }
            MouseEventKind::Down(MouseButton::Left) if within => {
                let rendered_line = self.scroll + (event.row - self.area.y) as usize;
                if rendered_line >= self.total_lines {
                    return EventResult::Consumed(None);
                }
                self.cursor_line = rendered_line;
                // A link on the clicked line takes priority over a plain jump.
                self.follow_link(rendered_line)
                    .unwrap_or_else(|| self.goto_source(rendered_line))
            }
            // Swallow other mouse events (including clicks on the source pane) so
            // the preview stays open until q/Esc rather than being torn down by
            // the editor refocusing on a click.
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

    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        // Fullscreen: cover the whole editor. Small screens don't have room for a
        // useful side-by-side split, so the preview replaces the source view while
        // it's up.
        let panel = area;

        // Re-read the source document each frame so the preview stays live. Also
        // pick up the source doc's soft-wrap setting so the preview honors it —
        // long paragraphs in narrow terminals wrap instead of getting truncated.
        let (contents, soft_wrap) = cx
            .editor
            .document(self.source_doc)
            .map(|doc| {
                (
                    doc.text().to_string(),
                    doc.text_format(panel.width, Some(&cx.editor.theme)).soft_wrap,
                )
            })
            .unwrap_or_default();
        let markdown = Markdown::new(contents, cx.editor.syn_loader.clone());
        let (text, line_map, links) = markdown.parse_with_map(Some(&cx.editor.theme));
        self.total_lines = text.lines.len();
        self.line_map = line_map;
        self.links = links;

        let block =
            Block::bordered().title("Markdown preview  (q/Esc close · navigate source to scroll)");
        let inner = block.inner(panel);
        surface.clear_with(panel, cx.editor.theme.get("ui.popup"));
        block.render(panel, surface);
        self.area = inner;

        // Follow the source cursor: when it moves to a different source line,
        // highlight the matching rendered line and center the panel on it. Only
        // on change, so mouse-wheel scrolling isn't immediately overridden.
        if let Some(source_line) = self.source_cursor_line(cx.editor) {
            if self.last_source_line != Some(source_line) {
                self.last_source_line = Some(source_line);
                if let Some(rendered) = self.rendered_line_for_source(source_line) {
                    self.cursor_line = rendered;
                    self.scroll = rendered.saturating_sub(inner.height as usize / 2);
                }
            }
        }

        // Clamp scroll/cursor now that we know the rendered length.
        let max_scroll = self.total_lines.saturating_sub(inner.height as usize);
        self.scroll = self.scroll.min(max_scroll);
        self.cursor_line = self.cursor_line.min(self.total_lines.saturating_sub(1));

        // When soft-wrap is off, one rendered line == one screen row, so a clicked
        // row maps directly to a rendered line (`screen_row = rendered_line -
        // scroll`) and the cursor-line highlight lands exactly.
        //
        // When soft-wrap is on, Paragraph interprets `scroll.0` as an output-row
        // offset (post-wrap) while we still track scroll/cursor in rendered-line
        // units — click and highlight positions can drift by the number of wrap
        // segments above them. That's an accepted tradeoff for readable output on
        // narrow terminals; wrap-aware mapping can come later if needed.
        let mut paragraph = Paragraph::new(&text).scroll((self.scroll as u16, 0));
        if soft_wrap {
            paragraph = paragraph.wrap(Wrap { trim: false });
        }
        paragraph.render(inner, surface);

        // Highlight the active line.
        if self.cursor_line >= self.scroll && self.cursor_line < self.scroll + inner.height as usize
        {
            let row = inner.y + (self.cursor_line - self.scroll) as u16;
            let style = cx
                .editor
                .theme
                .try_get("ui.cursorline.primary")
                .unwrap_or_else(|| cx.editor.theme.get("ui.selection"));
            surface.set_style(Rect::new(inner.x, row, inner.width, 1), style);
        }
    }

    fn id(&self) -> Option<&'static str> {
        Some(Self::ID)
    }
}

/// Move `view`'s cursor to the given 0-based `line` in its document and center
/// the view on it.
fn goto_source_line(editor: &mut Editor, view: ViewId, line: usize) {
    if !editor.tree.contains(view) {
        return;
    }
    let doc_id = editor.tree.get(view).doc;
    let Some(doc) = editor.documents.get_mut(&doc_id) else {
        return;
    };
    let text = doc.text();
    let line = line.min(text.len_lines().saturating_sub(1));
    let pos = text.line_to_char(line);
    doc.set_selection(view, Selection::point(pos));
    align_view(doc, editor.tree.get(view), Align::Center);
}

/// Open a markdown link destination, resolving relative paths against
/// `base_dir`. Absolute non-`file` URLs (http, mailto, …) are handed to the
/// OS; `file:` URLs and filesystem paths open in the editor, with directories
/// opening a file picker.
///
/// Returns `true` when the editor navigated (so the caller can close the
/// preview) and `false` when the link was handed off externally.
fn open_link(compositor: &mut Compositor, cx: &mut Context, dest: &str, base_dir: &Path) -> bool {
    let dest = dest.trim();
    if dest.is_empty() {
        return false;
    }

    // Absolute URLs: hand non-file schemes to the OS; open file: in the editor.
    if let Ok(url) = Url::parse(dest) {
        if url.scheme() == "file" {
            return open_path_in_editor(compositor, cx, &PathBuf::from(url.path()));
        }
        cx.jobs.callback(crate::open_external_url_callback(url));
        return false;
    }

    // Otherwise a filesystem path; drop any in-page anchor (e.g. `f.md#section`).
    let path = dest.split('#').next().unwrap_or(dest);
    if path.is_empty() {
        return false;
    }
    let expanded = path::expand(path);
    open_path_in_editor(compositor, cx, &base_dir.join(expanded))
}

/// Open a filesystem path in the editor: directories push a file picker, files
/// are opened in place. Returns `true` if the editor navigated.
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
