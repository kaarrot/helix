use crate::compositor::{Component, Context};
use arc_swap::ArcSwap;
use tui::{
    buffer::Buffer as Surface,
    text::{Span, Spans, Text},
};

use std::sync::Arc;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use helix_core::{
    syntax::{self, HighlightEvent, OverlayHighlights},
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
        self.parse_with_map(theme).0
    }

    /// Like [`Markdown::parse`], but also returns a per-rendered-line mapping
    /// back to the source line it came from (`None` for inserted blank/separator
    /// lines), plus the markdown links anchored to each rendered line. Used by
    /// the markdown preview for click-to-source and goto-file.
    pub fn parse_with_map(
        &self,
        theme: Option<&Theme>,
    ) -> (tui::text::Text<'_>, Vec<Option<usize>>, Vec<MarkdownLink>) {
        // Flush the accumulated spans as one finished line, recording its source
        // line and any links anchored to it. ALL visible-line output must go
        // through this (and blank-line output through `blank`) so `lines`,
        // `line_map` and `links` stay in lockstep.
        fn push_line<'a>(
            spans: &mut Vec<Span<'a>>,
            lines: &mut Vec<Spans<'a>>,
            line_map: &mut Vec<Option<usize>>,
            links: &mut Vec<MarkdownLink>,
            cur_links: &mut Vec<String>,
            src: Option<usize>,
        ) {
            let spans = std::mem::take(spans);
            if !spans.is_empty() {
                let idx = lines.len();
                lines.push(Spans::from(spans));
                line_map.push(src);
                for dest in cur_links.drain(..) {
                    links.push(MarkdownLink { line: idx, dest });
                }
            } else {
                // No visible content to anchor links to; drop them.
                cur_links.clear();
            }
        }

        // Push an inserted blank/separator line with no corresponding source line.
        fn blank<'a>(lines: &mut Vec<Spans<'a>>, line_map: &mut Vec<Option<usize>>) {
            lines.push(Spans::default());
            line_map.push(None);
        }

        // Render raw HTML (common in READMEs, e.g. `<div align="center">`,
        // `<img ...>`, `<br>`) by stripping the tag markup rather than printing it
        // verbatim. `<br>` becomes a line break; other tags are dropped but any
        // text content between them is kept. HTML newlines are treated as
        // whitespace so block tags don't introduce spurious blank lines.
        #[allow(clippy::too_many_arguments)]
        fn push_html<'a>(
            html: &str,
            style: Style,
            spans: &mut Vec<Span<'a>>,
            lines: &mut Vec<Spans<'a>>,
            line_map: &mut Vec<Option<usize>>,
            links: &mut Vec<MarkdownLink>,
            cur_links: &mut Vec<String>,
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
                            push_line(spans, lines, line_map, links, cur_links, src);
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
        let byte_to_line =
            |b: usize| line_starts.partition_point(|&start| start <= b).saturating_sub(1);

        let mut options = Options::empty();
        options.insert(Options::ENABLE_STRIKETHROUGH);
        // `into_offset_iter` yields the source byte range alongside each event so
        // we can map rendered lines back to source lines.
        let parser = Parser::new_ext(&self.contents, options).into_offset_iter();

        // TODO: if possible, render links as terminal hyperlinks: https://gist.github.com/egmontkob/eb114294efbcd5adb1944c9f3cb5feda
        let mut tags = Vec::new();
        let mut spans = Vec::new();
        let mut lines = Vec::new();
        let mut line_map: Vec<Option<usize>> = Vec::new();
        let mut links: Vec<MarkdownLink> = Vec::new();
        // Link destinations seen on the line currently being accumulated; flushed
        // into `links` by `push_line` once the line is finalized.
        let mut cur_links: Vec<String> = Vec::new();
        // Source line of the content currently being accumulated into `spans`.
        let mut src: Option<usize> = None;
        let mut list_stack = Vec::new();

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
                            &mut cur_links,
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
                Event::Start(tag) => {
                    // Capture link/image destinations so the preview can offer
                    // goto-file on them. The link text itself flows through the
                    // normal `Event::Text` path below.
                    if let Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } = &tag {
                        cur_links.push(dest_url.to_string());
                    }
                    tags.push(tag);
                    if spans.is_empty() && !list_stack.is_empty() {
                        // TODO: could push indent + 2 or 3 spaces to align with
                        // the rest of the list.
                        spans.push(Span::from(get_indent(list_stack.len())));
                    }
                }
                Event::End(tag) => {
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
                                &mut cur_links,
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
                        // Make link/image text visibly clickable: apply the theme
                        // link color (if any) and always underline, even when the
                        // link text also carries emphasis/strong.
                        if tags
                            .iter()
                            .any(|tag| matches!(tag, Tag::Link { .. } | Tag::Image { .. }))
                        {
                            style = style.patch(link_style);
                            style.underline_style = Some(UnderlineStyle::Line);
                        }
                        spans.push(Span::styled(text, style));
                    }
                }
                Event::Code(text) => {
                    spans.push(Span::styled(text, code_style));
                }
                Event::Html(text) | Event::InlineHtml(text) => {
                    push_html(
                        &text,
                        text_style,
                        &mut spans,
                        &mut lines,
                        &mut line_map,
                        &mut links,
                        &mut cur_links,
                        src,
                    );
                }
                Event::SoftBreak | Event::HardBreak => {
                    push_line(
                        &mut spans,
                        &mut lines,
                        &mut line_map,
                        &mut links,
                        &mut cur_links,
                        src,
                    );
                    if !list_stack.is_empty() {
                        // TODO: could push indent + 2 or 3 spaces to align with
                        // the rest of the list.
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
            &mut cur_links,
            src,
        );

        // if last line is empty, remove it
        if let Some(line) = lines.last() {
            if line.0.is_empty() {
                lines.pop();
                line_map.pop();
            }
        }

        (Text::from(lines), line_map, links)
    }
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
