use helix_core::doc_formatter::FormattedGrapheme;
use helix_core::Rope;
use helix_view::theme::{Style, Theme};
use helix_view::Document;
use helix_vcs::{compute_inline_diff, ChangeType, InlineDiffConfig, InlineDiffHighlight};

use crate::ui::document::{LinePos, TextRenderer};
use crate::ui::text_decorations::Decoration;

pub struct InlineDiffDecoration {
    diff_highlights: Vec<(u32, Vec<InlineDiffHighlight>)>, // (line_number, highlights)
    current_line: u32,
    current_highlights: Vec<InlineDiffHighlight>,
    add_style: Style,
    delete_style: Style,
    change_style: Style,
}

impl InlineDiffDecoration {
    pub fn new(
        doc: &Document,
        other_text: &Rope,
        theme: &Theme,
    ) -> Self {
        let diff_highlights = compute_line_diffs(doc.text(), other_text);

        // Get theme colors for diff highlighting
        let add_style = theme.get("diff.plus").bg(theme.get("diff.plus").bg.unwrap_or(helix_view::graphics::Color::Green));
        let delete_style = theme.get("diff.minus").bg(theme.get("diff.minus").bg.unwrap_or(helix_view::graphics::Color::Red));
        let change_style = theme.get("diff.delta").bg(theme.get("diff.delta").bg.unwrap_or(helix_view::graphics::Color::Yellow));

        Self {
            diff_highlights,
            current_line: 0,
            current_highlights: Vec::new(),
            add_style,
            delete_style,
            change_style,
        }
    }
}

impl Decoration for InlineDiffDecoration {
    fn decorate_line(&mut self, _renderer: &mut TextRenderer, pos: LinePos) {
        self.current_line = pos.doc_line as u32;

        // Find highlights for this line
        self.current_highlights = self.diff_highlights
            .iter()
            .find(|(line, _)| *line == self.current_line)
            .map(|(_, highlights)| highlights.clone())
            .unwrap_or_default();
    }

    fn decorate_grapheme(
        &mut self,
        renderer: &mut TextRenderer,
        grapheme: &FormattedGrapheme,
    ) -> usize {
        let char_idx = grapheme.char_idx;

        // Check if this character should be highlighted
        for highlight in &self.current_highlights {
            if char_idx >= highlight.start_col && char_idx < highlight.end_col {
                let style = match highlight.change_type {
                    ChangeType::Insert => self.add_style,
                    ChangeType::Delete => self.delete_style,
                    ChangeType::Replace => self.change_style,
                };

                renderer.set_style(
                    helix_view::graphics::Rect::new(
                        grapheme.visual_pos.col as u16,
                        grapheme.visual_pos.row as u16,
                        grapheme.width() as u16,
                        1,
                    ),
                    style,
                );
                break;
            }
        }

        usize::MAX
    }
}

/// Compute character-level diffs for changed lines between two texts
fn compute_line_diffs(text1: &Rope, text2: &Rope) -> Vec<(u32, Vec<InlineDiffHighlight>)> {
    let config = InlineDiffConfig::default();
    let mut result = Vec::new();

    let line_count = text1.len_lines().min(text2.len_lines());

    for line_idx in 0..line_count {
        let line1 = text1.line(line_idx).to_string();
        let line2 = text2.line(line_idx).to_string();

        // Only compute diff if lines are different
        if line1 != line2 {
            let highlights = compute_inline_diff(&line1, &line2, &config);
            if !highlights.is_empty() {
                result.push((line_idx as u32, highlights));
            }
        }
    }

    result
}
