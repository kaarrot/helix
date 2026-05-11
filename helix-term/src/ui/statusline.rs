use helix_core::indent::IndentStyle;
use helix_core::{coords_at_pos, encoding, unicode::width::UnicodeWidthStr, Position};
use helix_lsp::lsp::DiagnosticSeverity;
use helix_view::document::DEFAULT_LANGUAGE_NAME;
use helix_view::{
    document::{Mode, SCRATCH_BUFFER_NAME},
    graphics::Rect,
    theme::Style,
    Document, Editor, View,
};

use crate::ui::{Completion, ProgressSpinners};

use helix_view::editor::StatusLineElement as StatusLineElementID;
use tui::buffer::Buffer as Surface;
use tui::text::{Span, Spans};

const COMPLETION_MAX_ITEMS: usize = 5;
const COMPLETION_MAX_LABEL_WIDTH: usize = 16;

pub struct RenderContext<'a> {
    pub editor: &'a Editor,
    pub doc: &'a Document,
    pub view: &'a View,
    pub focused: bool,
    pub spinners: &'a ProgressSpinners,
    pub completion: Option<&'a Completion>,
    pub parts: RenderBuffer<'a>,
}

impl<'a> RenderContext<'a> {
    pub fn new(
        editor: &'a Editor,
        doc: &'a Document,
        view: &'a View,
        focused: bool,
        spinners: &'a ProgressSpinners,
        completion: Option<&'a Completion>,
    ) -> Self {
        RenderContext {
            editor,
            doc,
            view,
            focused,
            spinners,
            completion,
            parts: RenderBuffer::default(),
        }
    }
}

#[derive(Default)]
pub struct RenderBuffer<'a> {
    pub left: Spans<'a>,
    pub center: Spans<'a>,
    pub right: Spans<'a>,
}

pub fn render(context: &mut RenderContext, viewport: Rect, surface: &mut Surface) {
    let base_style = if context.focused {
        context.editor.theme.get("ui.statusline")
    } else {
        context.editor.theme.get("ui.statusline.inactive")
    };

    surface.set_style(viewport.with_height(1), base_style);

    // Left side of the status line.

    let config = context.editor.config();

    for element_id in &config.statusline.left {
        let render = get_render_function(*element_id);
        (render)(context, |context, span| {
            append(&mut context.parts.left, span, base_style)
        });
    }

    surface.set_spans(
        viewport.x,
        viewport.y,
        &context.parts.left,
        context.parts.left.width() as u16,
    );

    // Right side of the status line.

    for element_id in &config.statusline.right {
        let render = get_render_function(*element_id);
        (render)(context, |context, span| {
            append(&mut context.parts.right, span, base_style)
        })
    }

    surface.set_spans(
        viewport.x
            + viewport
                .width
                .saturating_sub(context.parts.right.width() as u16),
        viewport.y,
        &context.parts.right,
        context.parts.right.width() as u16,
    );

    // Center of the status line.

    for element_id in &config.statusline.center {
        let render = get_render_function(*element_id);
        (render)(context, |context, span| {
            append(&mut context.parts.center, span, base_style)
        })
    }

    // Width of the empty space between the left and center area and between the center and right area.
    let spacing = 1u16;

    let edge_width = context.parts.left.width().max(context.parts.right.width()) as u16;
    let center_max_width = viewport.width.saturating_sub(2 * edge_width + 2 * spacing);
    let center_width = center_max_width.min(context.parts.center.width() as u16);

    surface.set_spans(
        viewport.x + viewport.width / 2 - center_width / 2,
        viewport.y,
        &context.parts.center,
        center_width,
    );
}

pub fn completion_suggestion_index_at(
    context: &mut RenderContext,
    viewport: Rect,
    column: u16,
    row: u16,
) -> Option<usize> {
    if row != viewport.y || column < viewport.left() || column >= viewport.right() {
        return None;
    }

    let (left_elements, center_elements, right_elements) = {
        let config = context.editor.config();
        let statusline = &config.statusline;
        (
            statusline.left.clone(),
            statusline.center.clone(),
            statusline.right.clone(),
        )
    };

    let left = section_layout(context, &left_elements);
    let right = section_layout(context, &right_elements);
    let center = section_layout(context, &center_elements);

    let spacing = 1u16;
    let left_width = left.width.min(u16::MAX as usize) as u16;
    let right_width = right.width.min(u16::MAX as usize) as u16;
    let edge_width = left_width.max(right_width);
    let center_max_width = viewport.width.saturating_sub(2 * edge_width + 2 * spacing);
    let center_width = center_max_width.min(center.width.min(u16::MAX as usize) as u16);

    let center_start = viewport.x + viewport.width / 2 - center_width / 2;
    let right_start = viewport.x + viewport.width.saturating_sub(right_width);

    hit_test_completion_suggestion(&center, center_start, center_width, viewport, column)
        .or_else(|| {
            hit_test_completion_suggestion(&right, right_start, right_width, viewport, column)
        })
        .or_else(|| hit_test_completion_suggestion(&left, viewport.x, left_width, viewport, column))
}

#[derive(Default)]
struct SectionLayout {
    width: usize,
    suggestions: Vec<CompletionSuggestionRange>,
}

struct CompletionSuggestionRange {
    start: usize,
    end: usize,
    index: usize,
}

fn section_layout(context: &mut RenderContext, elements: &[StatusLineElementID]) -> SectionLayout {
    let mut layout = SectionLayout::default();

    for element_id in elements {
        if matches!(element_id, StatusLineElementID::CompletionSuggestions) {
            for part in completion_suggestion_spans(context) {
                let span_width = part.span.content.width();
                if let Some(index) = part.index {
                    layout.suggestions.push(CompletionSuggestionRange {
                        start: layout.width,
                        end: layout.width + span_width,
                        index,
                    });
                }
                layout.width += span_width;
            }
        } else {
            let render = get_render_function(*element_id);
            let span_count_before = context.parts.left.0.len();
            let width_before = context.parts.left.width();
            (render)(context, |context, span| {
                context.parts.left.0.push(span);
            });
            layout.width += context.parts.left.width() - width_before;
            context.parts.left.0.truncate(span_count_before);
        }
    }

    layout
}

fn hit_test_completion_suggestion(
    layout: &SectionLayout,
    section_start: u16,
    section_width: u16,
    viewport: Rect,
    column: u16,
) -> Option<usize> {
    let section_start = section_start as usize;
    let section_end = section_start + section_width as usize;
    let viewport_start = viewport.left() as usize;
    let viewport_end = viewport.right() as usize;
    let column = column as usize;

    if column < section_start.max(viewport_start) || column >= section_end.min(viewport_end) {
        return None;
    }

    let relative_column = column.saturating_sub(section_start);
    layout
        .suggestions
        .iter()
        .find(|range| relative_column >= range.start && relative_column < range.end)
        .map(|range| range.index)
}

fn append<'a>(buffer: &mut Spans<'a>, mut span: Span<'a>, base_style: Style) {
    span.style = base_style.patch(span.style);
    buffer.0.push(span);
}

fn get_render_function<'a, F>(element_id: StatusLineElementID) -> impl Fn(&mut RenderContext<'a>, F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    match element_id {
        helix_view::editor::StatusLineElement::Mode => render_mode,
        helix_view::editor::StatusLineElement::Spinner => render_lsp_spinner,
        helix_view::editor::StatusLineElement::FileBaseName => render_file_base_name,
        helix_view::editor::StatusLineElement::FileName => render_file_name,
        helix_view::editor::StatusLineElement::FileAbsolutePath => render_file_absolute_path,
        helix_view::editor::StatusLineElement::FileModificationIndicator => {
            render_file_modification_indicator
        }
        helix_view::editor::StatusLineElement::ReadOnlyIndicator => render_read_only_indicator,
        helix_view::editor::StatusLineElement::FileEncoding => render_file_encoding,
        helix_view::editor::StatusLineElement::FileLineEnding => render_file_line_ending,
        helix_view::editor::StatusLineElement::FileIndentStyle => render_file_indent_style,
        helix_view::editor::StatusLineElement::FileType => render_file_type,
        helix_view::editor::StatusLineElement::Diagnostics => render_diagnostics,
        helix_view::editor::StatusLineElement::WorkspaceDiagnostics => render_workspace_diagnostics,
        helix_view::editor::StatusLineElement::Selections => render_selections,
        helix_view::editor::StatusLineElement::PrimarySelectionLength => {
            render_primary_selection_length
        }
        helix_view::editor::StatusLineElement::Position => render_position,
        helix_view::editor::StatusLineElement::PositionPercentage => render_position_percentage,
        helix_view::editor::StatusLineElement::TotalLineNumbers => render_total_line_numbers,
        helix_view::editor::StatusLineElement::Separator => render_separator,
        helix_view::editor::StatusLineElement::Spacer => render_spacer,
        helix_view::editor::StatusLineElement::VersionControl => render_version_control,
        helix_view::editor::StatusLineElement::Register => render_register,
        helix_view::editor::StatusLineElement::CurrentWorkingDirectory => render_cwd,
        helix_view::editor::StatusLineElement::CompletionSuggestions => {
            render_completion_suggestions
        }
    }
}

fn render_mode<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    let visible = context.focused;
    let config = context.editor.config();
    let modenames = &config.statusline.mode;
    let mode_str = match context.editor.mode() {
        Mode::Insert => &modenames.insert,
        Mode::Select => &modenames.select,
        Mode::Normal => &modenames.normal,
    };
    let content = if visible {
        format!(" {mode_str} ")
    } else {
        // If not focused, explicitly leave an empty space instead of returning None.
        " ".repeat(mode_str.width() + 2)
    };
    let style = if visible && config.color_modes {
        match context.editor.mode() {
            Mode::Insert => context.editor.theme.get("ui.statusline.insert"),
            Mode::Select => context.editor.theme.get("ui.statusline.select"),
            Mode::Normal => context.editor.theme.get("ui.statusline.normal"),
        }
    } else {
        Style::default()
    };
    write(context, Span::styled(content, style));
}

// TODO think about handling multiple language servers
fn render_lsp_spinner<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    let language_server = context.doc.language_servers().next();
    write(
        context,
        language_server
            .and_then(|srv| {
                context
                    .spinners
                    .get(srv.id())
                    .and_then(|spinner| spinner.frame())
            })
            // Even if there's no spinner; reserve its space to avoid elements frequently shifting.
            .unwrap_or(" ")
            .into(),
    );
}

fn render_diagnostics<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    use helix_core::diagnostic::Severity;
    let (hints, info, warnings, errors) =
        context
            .doc
            .diagnostics()
            .iter()
            .fold((0, 0, 0, 0), |mut counts, diag| {
                match diag.severity {
                    Some(Severity::Hint) | None => counts.0 += 1,
                    Some(Severity::Info) => counts.1 += 1,
                    Some(Severity::Warning) => counts.2 += 1,
                    Some(Severity::Error) => counts.3 += 1,
                }
                counts
            });

    for sev in &context.editor.config().statusline.diagnostics {
        match sev {
            Severity::Hint if hints > 0 => {
                write(context, Span::styled("●", context.editor.theme.get("hint")));
                write(context, format!(" {} ", hints).into());
            }
            Severity::Info if info > 0 => {
                write(context, Span::styled("●", context.editor.theme.get("info")));
                write(context, format!(" {} ", info).into());
            }
            Severity::Warning if warnings > 0 => {
                write(
                    context,
                    Span::styled("●", context.editor.theme.get("warning")),
                );
                write(context, format!(" {} ", warnings).into());
            }
            Severity::Error if errors > 0 => {
                write(
                    context,
                    Span::styled("●", context.editor.theme.get("error")),
                );
                write(context, format!(" {} ", errors).into());
            }
            _ => {}
        }
    }
}

fn render_workspace_diagnostics<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    use helix_core::diagnostic::Severity;
    let (hints, info, warnings, errors) = context.editor.diagnostics.values().flatten().fold(
        (0u32, 0u32, 0u32, 0u32),
        |mut counts, (diag, _)| {
            match diag.severity {
                // PERF: For large workspace diagnostics, this loop can be very tight.
                //
                // Most often the diagnostics will be for warnings and errors.
                // Errors should tend to be fixed fast, leaving warnings as the most common.
                Some(DiagnosticSeverity::WARNING) => counts.2 += 1,
                Some(DiagnosticSeverity::ERROR) => counts.3 += 1,
                Some(DiagnosticSeverity::HINT) => counts.0 += 1,
                Some(DiagnosticSeverity::INFORMATION) => counts.1 += 1,
                // Fallback to `hint`.
                _ => counts.0 += 1,
            }
            counts
        },
    );

    let sevs_to_show = &context.editor.config().statusline.workspace_diagnostics;

    // Avoid showing the " W " if no diagnostic counts will be shown.
    if !sevs_to_show.iter().any(|sev| match sev {
        Severity::Hint => hints != 0,
        Severity::Info => info != 0,
        Severity::Warning => warnings != 0,
        Severity::Error => errors != 0,
    }) {
        return;
    }

    write(context, " W ".into());

    for sev in sevs_to_show {
        match sev {
            Severity::Hint if hints > 0 => {
                write(context, Span::styled("●", context.editor.theme.get("hint")));
                write(context, format!(" {} ", hints).into());
            }
            Severity::Info if info > 0 => {
                write(context, Span::styled("●", context.editor.theme.get("info")));
                write(context, format!(" {} ", info).into());
            }
            Severity::Warning if warnings > 0 => {
                write(
                    context,
                    Span::styled("●", context.editor.theme.get("warning")),
                );
                write(context, format!(" {} ", warnings).into());
            }
            Severity::Error if errors > 0 => {
                write(
                    context,
                    Span::styled("●", context.editor.theme.get("error")),
                );
                write(context, format!(" {} ", errors).into());
            }
            _ => {}
        }
    }
}

fn render_selections<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    let selection = context.doc.selection(context.view.id);
    let count = selection.len();
    write(
        context,
        if count == 1 {
            " 1 sel ".into()
        } else {
            format!(" {}/{count} sels ", selection.primary_index() + 1).into()
        },
    );
}

fn render_primary_selection_length<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    let tot_sel = context.doc.selection(context.view.id).primary().len();
    write(
        context,
        format!(" {} char{} ", tot_sel, if tot_sel == 1 { "" } else { "s" }).into(),
    );
}

fn get_position(context: &RenderContext) -> Position {
    coords_at_pos(
        context.doc.text().slice(..),
        context
            .doc
            .selection(context.view.id)
            .primary()
            .cursor(context.doc.text().slice(..)),
    )
}

fn render_position<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    if suggestions_taking_over(context) {
        return;
    }
    let position = get_position(context);
    write(
        context,
        format!(" {}:{} ", position.row + 1, position.col + 1).into(),
    );
}

fn render_total_line_numbers<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    if suggestions_taking_over(context) {
        return;
    }
    let total_line_numbers = context.doc.text().len_lines();

    write(context, format!(" {} ", total_line_numbers).into());
}

fn render_position_percentage<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    if suggestions_taking_over(context) {
        return;
    }
    let position = get_position(context);
    let maxrows = context.doc.text().len_lines();
    write(
        context,
        format!("{}%", (position.row + 1) * 100 / maxrows).into(),
    );
}

fn render_file_encoding<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    let enc = context.doc.encoding();

    if enc != encoding::UTF_8 {
        write(context, format!(" {} ", enc.name()).into());
    }
}

fn render_file_line_ending<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    use helix_core::LineEnding::*;
    let line_ending = match context.doc.line_ending {
        Crlf => "CRLF",
        LF => "LF",
        #[cfg(feature = "unicode-lines")]
        VT => "VT", // U+000B -- VerticalTab
        #[cfg(feature = "unicode-lines")]
        FF => "FF", // U+000C -- FormFeed
        #[cfg(feature = "unicode-lines")]
        CR => "CR", // U+000D -- CarriageReturn
        #[cfg(feature = "unicode-lines")]
        Nel => "NEL", // U+0085 -- NextLine
        #[cfg(feature = "unicode-lines")]
        LS => "LS", // U+2028 -- Line Separator
        #[cfg(feature = "unicode-lines")]
        PS => "PS", // U+2029 -- ParagraphSeparator
    };

    write(context, format!(" {} ", line_ending).into());
}

fn render_file_type<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    let file_type = context.doc.language_name().unwrap_or(DEFAULT_LANGUAGE_NAME);

    write(context, format!(" {} ", file_type).into());
}

fn render_file_name<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    if suggestions_taking_over(context) {
        return;
    }
    let title = {
        let rel_path = context.doc.relative_path();
        let path = rel_path
            .as_ref()
            .map(|p| p.to_string_lossy())
            .unwrap_or_else(|| SCRATCH_BUFFER_NAME.into());
        format!(" {} ", path)
    };

    write(context, title.into());
}

fn render_file_absolute_path<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    if suggestions_taking_over(context) {
        return;
    }
    let title = {
        let path = context.doc.path();
        let path = path
            .as_ref()
            .map(|p| p.to_string_lossy())
            .unwrap_or_else(|| SCRATCH_BUFFER_NAME.into());
        format!(" {} ", path)
    };

    write(context, title.into());
}

fn render_file_modification_indicator<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    let title = if context.doc.is_modified() {
        "[+]"
    } else {
        "   "
    };

    write(context, title.into());
}

fn render_read_only_indicator<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    let title = if context.doc.readonly {
        " [readonly] "
    } else {
        ""
    };
    write(context, title.into());
}

fn render_file_base_name<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    if suggestions_taking_over(context) {
        return;
    }
    let title = {
        let rel_path = context.doc.relative_path();
        let path = rel_path
            .as_ref()
            .and_then(|p| p.file_name().map(|s| s.to_string_lossy()))
            .unwrap_or_else(|| SCRATCH_BUFFER_NAME.into());
        format!(" {} ", path)
    };

    write(context, title.into());
}

fn render_separator<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    let sep = &context.editor.config().statusline.separator;
    let style = context.editor.theme.get("ui.statusline.separator");

    write(context, Span::styled(sep.to_string(), style));
}

fn render_spacer<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    write(context, " ".into());
}

fn render_version_control<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    if suggestions_taking_over(context) {
        return;
    }
    let head = context
        .doc
        .version_control_head()
        .unwrap_or_default()
        .to_string();

    write(context, head.into());
}

fn render_register<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    if let Some(reg) = context.editor.selected_register {
        write(context, format!(" reg={} ", reg).into())
    }
}

fn render_file_indent_style<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    let style = context.doc.indent_style;

    write(
        context,
        match style {
            IndentStyle::Tabs => " tabs ".into(),
            IndentStyle::Spaces(indent) => {
                format!(" {} space{} ", indent, if indent == 1 { "" } else { "s" }).into()
            }
        },
    );
}

fn render_cwd<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    if suggestions_taking_over(context) {
        return;
    }
    let cwd = helix_stdx::env::current_working_dir();
    let cwd = cwd
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    write(context, cwd.into())
}

/// True when completion suggestions are currently being rendered in the statusline
/// and are therefore allowed to hide path elements to free up space.
fn suggestions_taking_over(context: &RenderContext) -> bool {
    use helix_view::editor::CompletionDisplay;
    if context.editor.mode() != Mode::Insert {
        return false;
    }
    let Some(completion) = context.completion else {
        return false;
    };
    if completion.is_empty() {
        return false;
    }
    if matches!(
        context.editor.config().completion_display,
        CompletionDisplay::Popup
    ) {
        return false;
    }
    let cfg = &context.editor.config().statusline;
    cfg.left
        .iter()
        .chain(cfg.center.iter())
        .chain(cfg.right.iter())
        .any(|e| matches!(e, StatusLineElementID::CompletionSuggestions))
}

fn render_completion_suggestions<'a, F>(context: &mut RenderContext<'a>, write: F)
where
    F: Fn(&mut RenderContext<'a>, Span<'a>) + Copy,
{
    for part in completion_suggestion_spans(context) {
        write(context, part.span);
    }
}

struct CompletionSuggestionSpan<'a> {
    span: Span<'a>,
    index: Option<usize>,
}

fn completion_suggestion_spans<'a>(
    context: &RenderContext<'a>,
) -> Vec<CompletionSuggestionSpan<'a>> {
    use crate::handlers::completion::{CompletionItem, LspCompletionItem};
    use helix_core::CompletionItem as CoreCompletionItem;
    use helix_view::editor::CompletionDisplay;

    if context.editor.mode() != Mode::Insert {
        return Vec::new();
    }
    if matches!(
        context.editor.config().completion_display,
        CompletionDisplay::Popup
    ) {
        return Vec::new();
    }
    let Some(completion) = context.completion else {
        return Vec::new();
    };

    let unselected_style = context.editor.theme.get("ui.menu");
    let selected_style = context.editor.theme.get("ui.menu.selected");

    let window_start = completion
        .cursor()
        .map_or(0, |c| c.saturating_sub(COMPLETION_MAX_ITEMS - 1));

    let mut spans = Vec::new();
    let mut wrote_any = false;
    for (relative_index, (item, selected)) in completion
        .matched_items()
        .skip(window_start)
        .take(COMPLETION_MAX_ITEMS)
        .enumerate()
    {
        let label = match item {
            CompletionItem::Lsp(LspCompletionItem { item, .. }) => item.label.as_str(),
            CompletionItem::Other(CoreCompletionItem { label, .. }) => label.as_ref(),
        };

        let mut truncated = String::with_capacity(COMPLETION_MAX_LABEL_WIDTH + 1);
        for (i, ch) in label.chars().enumerate() {
            if i >= COMPLETION_MAX_LABEL_WIDTH {
                truncated.push('…');
                break;
            }
            truncated.push(ch);
        }

        if wrote_any {
            spans.push(CompletionSuggestionSpan {
                span: Span::styled(" ", unselected_style),
                index: None,
            });
        }
        let style = if selected {
            selected_style
        } else {
            unselected_style
        };
        spans.push(CompletionSuggestionSpan {
            span: Span::styled(format!(" {truncated} "), style),
            index: Some(window_start + relative_index),
        });
        wrote_any = true;
    }

    spans
}

#[cfg(test)]
mod tests {
    use std::{borrow::Cow, path::Path, sync::Arc};

    use arc_swap::{access::Map, ArcSwap};
    use helix_core::{completion::CompletionProvider, syntax, Selection, Transaction};
    use helix_view::{
        document::Mode,
        editor::{Action, CompletionDisplay, StatusLineElement},
        graphics::Rect,
        theme, Editor,
    };

    use crate::{
        config::Config as AppConfig,
        handlers,
        handlers::completion::CompletionItem,
        ui::{Completion, ProgressSpinners},
    };

    use super::{completion_suggestion_index_at, render, RenderContext, Surface};

    struct TestHarness {
        _runtime: tokio::runtime::Runtime,
        app_config: Arc<ArcSwap<AppConfig>>,
        editor: Editor,
    }

    impl TestHarness {
        fn new() -> Self {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            let _guard = runtime.enter();

            let mut app_config = AppConfig::default();
            app_config.editor.statusline.left = vec![
                StatusLineElement::Mode,
                StatusLineElement::FileName,
                StatusLineElement::CompletionSuggestions,
            ];
            app_config.editor.statusline.right = vec![StatusLineElement::Position];

            let app_config = Arc::new(ArcSwap::from_pointee(app_config));
            let handlers = handlers::setup(Arc::clone(&app_config));
            let editor_config =
                Arc::new(Map::new(Arc::clone(&app_config), |config: &AppConfig| {
                    &config.editor
                }));

            let mut editor = Editor::new(
                Rect::new(0, 0, 40, 5),
                Arc::new(theme::Loader::new(&[])),
                Arc::new(ArcSwap::from_pointee(syntax::Loader::default())),
                editor_config,
                handlers,
            );
            editor.new_file(Action::VerticalSplit);
            editor.mode = Mode::Insert;

            let (view, doc) = current!(editor);
            doc.set_selection(view.id, Selection::point(0));
            doc.set_path(Some(Path::new("src/main.rs")));

            Self {
                _runtime: runtime,
                app_config,
                editor,
            }
        }

        fn set_completion_display(&self, completion_display: CompletionDisplay) {
            let mut config = (*self.app_config.load_full()).clone();
            config.editor.completion_display = completion_display;
            self.app_config.store(Arc::new(config));
        }
    }

    fn test_completion(editor: &Editor, labels: &[&str]) -> Completion {
        let (_, doc) = current_ref!(editor);
        let items = labels
            .iter()
            .map(|label| {
                CompletionItem::Other(helix_core::CompletionItem {
                    transaction: Transaction::new(doc.text()),
                    label: Cow::Owned((*label).to_owned()),
                    kind: Cow::Borrowed(""),
                    documentation: None,
                    provider: CompletionProvider::Word,
                })
            })
            .collect();

        Completion::new(editor, items, 0)
    }

    fn render_statusline(editor: &Editor, completion: Option<&Completion>, width: u16) -> String {
        let (view, doc) = current_ref!(editor);
        let viewport = Rect::new(0, 0, width, 1);
        let mut surface = Surface::empty(viewport);
        let spinners = ProgressSpinners::default();
        let mut context = RenderContext::new(editor, doc, view, true, &spinners, completion);

        render(&mut context, viewport, &mut surface);

        let mut line = String::new();
        for x in 0..width {
            line.push_str(&surface.get(x, 0).unwrap().symbol);
        }
        line.trim_end().to_owned()
    }

    #[test]
    fn statusline_completion_respects_display_mode_and_editor_mode() {
        let mut harness = TestHarness::new();
        let completion = test_completion(&harness.editor, &["alpha", "beta", "gamma"]);

        harness.set_completion_display(CompletionDisplay::Statusline);
        harness.editor.mode = Mode::Insert;
        let line = render_statusline(&harness.editor, Some(&completion), 26);
        assert!(line.contains("alpha"));
        assert!(line.contains("beta"));
        assert!(!line.contains("src/main.rs"));
        assert!(!line.contains("1:1"));

        harness.set_completion_display(CompletionDisplay::Popup);
        harness.editor.mode = Mode::Insert;
        let line = render_statusline(&harness.editor, Some(&completion), 40);
        assert!(line.contains("src/main.rs"));
        assert!(line.contains("1:1"));
        assert!(!line.contains("alpha"));

        harness.set_completion_display(CompletionDisplay::Both);
        harness.editor.mode = Mode::Normal;
        let line = render_statusline(&harness.editor, Some(&completion), 40);
        assert!(line.contains("src/main.rs"));
        assert!(line.contains("1:1"));
        assert!(!line.contains("alpha"));
    }

    #[test]
    fn statusline_completion_hit_test_returns_rendered_item_index() {
        let harness = TestHarness::new();
        let completion = test_completion(&harness.editor, &["alpha", "beta", "gamma"]);
        harness.set_completion_display(CompletionDisplay::Statusline);

        let line = render_statusline(&harness.editor, Some(&completion), 40);
        let beta_column = line.find("beta").expect("beta should render") as u16;
        let (view, doc) = current_ref!(harness.editor);
        let viewport = Rect::new(0, 0, 40, 1);
        let spinners = ProgressSpinners::default();
        let mut context = RenderContext::new(
            &harness.editor,
            doc,
            view,
            true,
            &spinners,
            Some(&completion),
        );

        assert_eq!(
            Some(1),
            completion_suggestion_index_at(&mut context, viewport, beta_column, 0)
        );
        assert_eq!(
            None,
            completion_suggestion_index_at(&mut context, viewport, beta_column, 1)
        );
    }
}
