use crate::doc_formatter::{DocumentFormatter, TextFormat};
use crate::text_annotations::{InlineAnnotation, Overlay, TextAnnotations};

impl TextFormat {
    fn new_test(softwrap: bool) -> Self {
        TextFormat {
            soft_wrap: softwrap,
            tab_width: 2,
            max_wrap: 3,
            max_indent_retain: 4,
            wrap_indicator: ".".into(),
            wrap_indicator_highlight: None,
            // use a prime number to allow lining up too often with repeat
            viewport_width: 17,
            soft_wrap_at_text_width: false,
        }
    }
}

impl<'t> DocumentFormatter<'t> {
    fn collect_to_str(&mut self) -> String {
        use std::fmt::Write;
        let mut res = String::new();
        let viewport_width = self.text_fmt.viewport_width;
        let soft_wrap_at_text_width = self.text_fmt.soft_wrap_at_text_width;
        let mut line = 0;

        for grapheme in self {
            if grapheme.visual_pos.row != line {
                line += 1;
                assert_eq!(grapheme.visual_pos.row, line);
                write!(res, "\n{}", ".".repeat(grapheme.visual_pos.col)).unwrap();
            }
            if !soft_wrap_at_text_width {
                assert!(
                    grapheme.visual_pos.col <= viewport_width as usize,
                    "softwrapped failed {}<={viewport_width}",
                    grapheme.visual_pos.col
                );
            }
            write!(res, "{}", grapheme.raw).unwrap();
        }

        res
    }
}

fn softwrap_text(text: &str) -> String {
    DocumentFormatter::new_at_prev_checkpoint(
        text.into(),
        &TextFormat::new_test(true),
        &TextAnnotations::default(),
        0,
    )
    .collect_to_str()
}

#[test]
fn basic_softwrap() {
    assert_eq!(
        softwrap_text(&"foo ".repeat(10)),
        "foo foo foo foo \n.foo foo foo foo \n.foo foo  "
    );
    assert_eq!(
        softwrap_text(&"fooo ".repeat(10)),
        "fooo fooo fooo \n.fooo fooo fooo \n.fooo fooo fooo \n.fooo  "
    );

    // check that we don't wrap unnecessarily
    assert_eq!(softwrap_text("\t\txxxx1xxxx2xx\n"), "    xxxx1xxxx2xx \n ");
}

#[test]
fn softwrap_indentation() {
    assert_eq!(
        softwrap_text("\t\tfoo1 foo2 foo3 foo4 foo5 foo6\n"),
        "    foo1 foo2 \n.....foo3 foo4 \n.....foo5 foo6 \n "
    );
    assert_eq!(
        softwrap_text("\t\t\tfoo1 foo2 foo3 foo4 foo5 foo6\n"),
        "      foo1 foo2 \n.foo3 foo4 foo5 \n.foo6 \n "
    );
}

#[test]
fn long_word_softwrap() {
    assert_eq!(
        softwrap_text("\t\txxxx1xxxx2xxxx3xxxx4xxxx5xxxx6xxxx7xxxx8xxxx9xxx\n"),
        "    xxxx1xxxx2xxx\n.....x3xxxx4xxxx5\n.....xxxx6xxxx7xx\n.....xx8xxxx9xxx \n "
    );
    assert_eq!(
        softwrap_text("xxxxxxxx1xxxx2xxx\n"),
        "xxxxxxxx1xxxx2xxx\n. \n "
    );
    assert_eq!(
        softwrap_text("\t\txxxx1xxxx 2xxxx3xxxx4xxxx5xxxx6xxxx7xxxx8xxxx9xxx\n"),
        "    xxxx1xxxx \n.....2xxxx3xxxx4x\n.....xxx5xxxx6xxx\n.....x7xxxx8xxxx9\n.....xxx \n "
    );
    assert_eq!(
        softwrap_text("\t\txxxx1xxx 2xxxx3xxxx4xxxx5xxxx6xxxx7xxxx8xxxx9xxx\n"),
        "    xxxx1xxx 2xxx\n.....x3xxxx4xxxx5\n.....xxxx6xxxx7xx\n.....xx8xxxx9xxx \n "
    );
}

#[test]
fn softwrap_multichar_grapheme() {
    assert_eq!(
        softwrap_text("xxxx xxxx xxx a\u{0301}bc\n"),
        "xxxx xxxx xxx \n.ábc \n "
    )
}

fn softwrap_text_at_text_width(text: &str) -> String {
    let mut text_fmt = TextFormat::new_test(true);
    text_fmt.soft_wrap_at_text_width = true;
    let annotations = TextAnnotations::default();
    let mut formatter =
        DocumentFormatter::new_at_prev_checkpoint(text.into(), &text_fmt, &annotations, 0);
    formatter.collect_to_str()
}
#[test]
fn long_word_softwrap_text_width() {
    assert_eq!(
        softwrap_text_at_text_width("xxxxxxxx1xxxx2xxx\nxxxxxxxx1xxxx2xxx"),
        "xxxxxxxx1xxxx2xxx \nxxxxxxxx1xxxx2xxx "
    );
}

fn overlay_text(text: &str, char_pos: usize, softwrap: bool, overlays: &[Overlay]) -> String {
    DocumentFormatter::new_at_prev_checkpoint(
        text.into(),
        &TextFormat::new_test(softwrap),
        TextAnnotations::default().add_overlay(overlays, None),
        char_pos,
    )
    .collect_to_str()
}

#[test]
fn overlay() {
    assert_eq!(
        overlay_text(
            "foobar",
            0,
            false,
            &[Overlay::new(0, "X"), Overlay::new(2, "\t")],
        ),
        "Xo  bar "
    );
    assert_eq!(
        overlay_text(
            &"foo ".repeat(10),
            0,
            true,
            &[
                Overlay::new(2, "\t"),
                Overlay::new(5, "\t"),
                Overlay::new(16, "X"),
            ]
        ),
        "fo   f  o foo \n.foo Xoo foo foo \n.foo foo foo  "
    );
}

fn annotate_text(text: &str, softwrap: bool, annotations: &[InlineAnnotation]) -> String {
    DocumentFormatter::new_at_prev_checkpoint(
        text.into(),
        &TextFormat::new_test(softwrap),
        TextAnnotations::default().add_inline_annotations(annotations, None),
        0,
    )
    .collect_to_str()
}

#[test]
fn annotation() {
    assert_eq!(
        annotate_text("bar", false, &[InlineAnnotation::new(0, "foo")]),
        "foobar "
    );
    assert_eq!(
        annotate_text(
            &"foo ".repeat(10),
            true,
            &[InlineAnnotation::new(0, "foo ")]
        ),
        "foo foo foo foo \n.foo foo foo foo \n.foo foo foo  "
    );
}

#[test]
fn annotation_and_overlay() {
    let annotations = [InlineAnnotation {
        char_idx: 0,
        text: "fooo".into(),
    }];
    let overlay = [Overlay {
        char_idx: 0,
        grapheme: "\t".into(),
    }];
    assert_eq!(
        DocumentFormatter::new_at_prev_checkpoint(
            "bbar".into(),
            &TextFormat::new_test(false),
            TextAnnotations::default()
                .add_inline_annotations(annotations.as_slice(), None)
                .add_overlay(overlay.as_slice(), None),
            0,
        )
        .collect_to_str(),
        "fooo  bar "
    );
}

fn fold_text(
    text: &str,
    softwrap: bool,
    folds: &[(usize, usize)],
    annotations: &[InlineAnnotation],
    char_idx: usize,
) -> String {
    use crate::fold::Fold;
    let folds: Vec<_> = folds
        .iter()
        .map(|&(header, last)| Fold::new(text.into(), header, last).unwrap())
        .collect();
    let text_fmt = TextFormat::new_test(softwrap);
    let mut text_annotations = TextAnnotations::default();
    text_annotations
        .add_inline_annotations(annotations, None)
        .set_folds(&folds);
    let mut formatter =
        DocumentFormatter::new_at_prev_checkpoint(text.into(), &text_fmt, &text_annotations, char_idx);
    formatter.collect_to_str()
}

#[test]
fn fold_hides_lines() {
    let text = "0\n1\n2\n3\n4\n";
    for softwrap in [false, true] {
        assert_eq!(fold_text(text, softwrap, &[(0, 2)], &[], 0), "0 \n3 \n4 \n ");
        assert_eq!(
            fold_text(text, softwrap, &[(0, 1), (2, 3)], &[], 0),
            "0 \n2 \n4 \n "
        );
        // folded up to the end of the text
        assert_eq!(fold_text(text, softwrap, &[(1, 4)], &[], 0), "0 \n1 \n ");
    }
}

#[test]
fn fold_starts_at_header_when_asked_for_a_hidden_char() {
    let text = "0\n1\n2\n3\n4\n";
    // char 4 is on the hidden line 2
    assert_eq!(fold_text(text, false, &[(0, 2)], &[], 4), "0 \n3 \n4 \n ");
}

#[test]
fn fold_drops_annotations_on_hidden_lines() {
    let text = "0\n1\n2\n3\n4\n";
    let annotations = [
        // after the header, before its line break
        InlineAnnotation::new(1, "~"),
        // on hidden lines
        InlineAnnotation::new(2, "X"),
        InlineAnnotation::new(5, "X"),
        // on the first line after the fold
        InlineAnnotation::new(6, "Y"),
    ];
    for softwrap in [false, true] {
        assert_eq!(
            fold_text(text, softwrap, &[(0, 2)], &annotations, 0),
            "0~ \nY3 \n4 \n "
        );
    }
}

#[test]
fn fold_reports_document_lines_and_positions() {
    use crate::fold::Fold;
    use crate::{char_idx_at_visual_offset, visual_offset_from_block, Position};

    let text: crate::RopeSlice = "0\n1\n2\n3\n4\n".into();
    let folds = [Fold::new(text, 0, 2).unwrap()];
    let text_fmt = TextFormat::new_test(false);
    let mut annotations = TextAnnotations::default();
    annotations.set_folds(&folds);

    let rows: Vec<_> = DocumentFormatter::new_at_prev_checkpoint(text, &text_fmt, &annotations, 0)
        .filter(|grapheme| grapheme.visual_pos.col == 0)
        .map(|grapheme| (grapheme.visual_pos.row, grapheme.line_idx, grapheme.char_idx))
        .collect();
    assert_eq!(rows, [(0, 0, 0), (1, 3, 6), (2, 4, 8), (3, 5, 10)]);

    // a hidden char shows up at the end of the header
    let (pos, _) = visual_offset_from_block(text, 0, 4, &text_fmt, &annotations);
    assert_eq!(pos, Position::new(0, 1));
    // the row below the header is the first line after the fold
    assert_eq!(
        char_idx_at_visual_offset(text, 0, 1, 0, &text_fmt, &annotations),
        (6, 0)
    );
    // and going back up from there skips the fold too
    assert_eq!(
        char_idx_at_visual_offset(text, 6, -1, 0, &text_fmt, &annotations),
        (0, 0)
    );
}
