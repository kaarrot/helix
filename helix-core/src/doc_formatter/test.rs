use std::cell::RefCell;
use std::rc::Rc;

use crate::doc_formatter::{DocumentFormatter, TextFormat};
use crate::text_annotations::{InlineAnnotation, LineAnnotation, Overlay, TextAnnotations};
use crate::Position;

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

// ---------------------------------------------------------------------------
// `skip_to_next_line` differential tests
//
// Skipping the off-screen tail of a line must be indistinguishable from
// iterating it and throwing the graphemes away. These tests assert that
// equivalence rather than pinning specific output, so fixtures can be added
// without anyone having to hand-compute expected strings.
// ---------------------------------------------------------------------------

/// A grapheme as observed by a caller that only cares about what it renders.
type Observed = (usize, Position, String);

/// Iterate the whole document, discarding graphemes at or past `clip`.
fn unclipped_graphemes(
    text: &str,
    text_fmt: &TextFormat,
    annotations: &TextAnnotations,
    clip: usize,
) -> Vec<Observed> {
    let mut formatter =
        DocumentFormatter::new_at_prev_checkpoint(text.into(), text_fmt, annotations, 0);
    let mut res = Vec::new();
    while let Some(g) = formatter.next() {
        if g.visual_pos.col < clip {
            res.push((g.char_idx, g.visual_pos, g.raw.to_string()));
        }
    }
    res
}

/// Iterate the same document, but skip the remainder of a line as soon as
/// `clip` is reached. Mirrors the structure of the renderer's clip.
fn clipped_graphemes(
    text: &str,
    text_fmt: &TextFormat,
    annotations: &TextAnnotations,
    clip: usize,
) -> Vec<Observed> {
    let mut formatter =
        DocumentFormatter::new_at_prev_checkpoint(text.into(), text_fmt, annotations, 0);
    let mut res = Vec::new();
    let mut last_row = usize::MAX;
    let mut can_skip = true;
    while let Some(g) = formatter.next() {
        if g.visual_pos.row != last_row {
            last_row = g.visual_pos.row;
            can_skip = true;
        }
        if g.visual_pos.col < clip {
            res.push((g.char_idx, g.visual_pos, g.raw.to_string()));
            continue;
        }
        if can_skip {
            if formatter.skip_to_next_line().is_some() {
                continue;
            }
            // this line refuses to be skipped, fall back to iterating it
            can_skip = false;
        }
    }
    res
}

fn assert_clip_equivalent(name: &str, text: &str, annotations: &TextAnnotations) {
    for clip in [1, 2, 3, 5, 17] {
        let text_fmt = TextFormat::new_test(false);
        // annotation cursors are shared through `Cell`, so re-seed between runs
        annotations.reset_pos(0);
        let expected = unclipped_graphemes(text, &text_fmt, annotations, clip);
        annotations.reset_pos(0);
        let got = clipped_graphemes(text, &text_fmt, annotations, clip);
        annotations.reset_pos(0);
        assert_eq!(
            expected, got,
            "clipped iteration diverged for {name:?} at clip={clip}"
        );
    }
}

/// Documents exercising the grapheme, line-ending and boundary cases the skip
/// has to reproduce exactly.
fn fixtures() -> Vec<(&'static str, String)> {
    vec![
        ("plain", "hello world\nsecond line\nthird\n".to_string()),
        // two consecutive skippable lines: catches a wedged `Layer` cursor,
        // which only manifests from the second skip onwards
        (
            "two long lines",
            format!("{}\n{}\n", "a".repeat(60), "b".repeat(60)),
        ),
        (
            "no trailing newline",
            format!("{}\n{}", "a".repeat(60), "b".repeat(60)),
        ),
        (
            "empty line between",
            format!("{}\n\n{}\n", "a".repeat(60), "b".repeat(60)),
        ),
        // a tab straddling the clip column
        ("tabs", format!("a\t{}\nx\ty\n", "b".repeat(40))),
        // CJK is double width, so a grapheme can straddle the clip boundary
        ("cjk", format!("{}\nplain\n", "\u{4f60}\u{597d}".repeat(30))),
        (
            "crlf",
            format!("{}\r\n{}\r\n", "a".repeat(60), "b".repeat(60)),
        ),
        (
            "lone cr",
            format!("{}\r{}\r", "a".repeat(60), "b".repeat(60)),
        ),
        ("combining", format!("{}\nplain\n", "e\u{0301}".repeat(30))),
        ("single long line", format!("{}\n", "a".repeat(200))),
        ("empty", String::new()),
        ("only newline", "\n".to_string()),
    ]
}

/// Char indices at which an annotation may legally be anchored.
///
/// Annotations anchored *inside* a multi-char grapheme cluster are never
/// consumed - `char_pos` advances by whole graphemes - which leaves the layer
/// cursor behind and trips the `debug_assert` in `Layer::consume`. That is a
/// pre-existing invariant of `TextAnnotations`, unrelated to clipping, so the
/// fixtures below have to respect it.
fn grapheme_boundaries(text: &str) -> Vec<usize> {
    use helix_stdx::rope::RopeSliceExt;
    let rope = crate::Rope::from_str(text);
    let slice = rope.slice(..);
    let mut boundaries = Vec::new();
    let mut char_idx = 0;
    for grapheme in slice.graphemes() {
        boundaries.push(char_idx);
        char_idx += grapheme.len_chars();
    }
    boundaries
}

/// Anchors spread across the document, all at grapheme boundaries, including
/// the first char of the second line - the `partition_point` boundary that a
/// sloppy cursor advance would consume too eagerly.
fn annotation_anchors(text: &str) -> Vec<usize> {
    let boundaries = grapheme_boundaries(text);
    let mut anchors: Vec<usize> = [0usize, 1, 2, 5, 18]
        .iter()
        .filter_map(|&i| boundaries.get(i).copied())
        .collect();

    let rope = crate::Rope::from_str(text);
    if rope.len_lines() > 1 {
        let second_line = rope.line_to_char(1);
        if second_line < rope.len_chars() {
            anchors.push(second_line);
        }
    }

    anchors.sort_unstable();
    anchors.dedup();
    anchors
}

#[test]
fn skip_to_next_line_matches_unclipped() {
    for (name, text) in fixtures() {
        assert_clip_equivalent(name, &text, &TextAnnotations::default());
    }
}

#[test]
fn skip_to_next_line_with_inline_annotations() {
    for (name, text) in fixtures() {
        let len = text.chars().count();
        if len < 4 {
            continue;
        }
        // annotations before, at and after the clip column, plus one anchored on
        // the first char of the second line
        let annotations: Vec<_> = annotation_anchors(&text)
            .iter()
            .map(|&idx| InlineAnnotation::new(idx, "VIRT"))
            .collect();
        let mut text_annotations = TextAnnotations::default();
        text_annotations.add_inline_annotations(&annotations, None);
        assert_clip_equivalent(
            &format!("{name} + inline annotations"),
            &text,
            &text_annotations,
        );
    }
}

#[test]
fn skip_to_next_line_with_overlays() {
    for (name, text) in fixtures() {
        let len = text.chars().count();
        if len < 4 {
            continue;
        }
        let boundaries = grapheme_boundaries(&text);
        let mut overlays: Vec<Overlay> = [0usize, 2]
            .iter()
            .filter_map(|&i| boundaries.get(i).copied())
            .map(|idx| Overlay::new(idx, "X"))
            .collect();

        // An overlay *on the line break* must suppress the skip entirely: the
        // formatter does not run its line break bookkeeping for an overlaid
        // newline, so skipping would desync `visual_pos`. This is the case that
        // proves `has_overlay_at` guards the right char index.
        let rope = crate::Rope::from_str(&text);
        if rope.len_lines() > 1 {
            overlays.push(Overlay::new(
                crate::line_ending::line_end_char_index(&rope.slice(..), 0),
                "N",
            ));
        }
        overlays.sort_by_key(|o| o.char_idx);
        overlays.dedup_by_key(|o| o.char_idx);

        let mut text_annotations = TextAnnotations::default();
        text_annotations.add_overlay(&overlays, None);
        assert_clip_equivalent(&format!("{name} + overlays"), &text, &text_annotations);
    }
}

/// Records every call a `LineAnnotation` receives, and inserts a virtual line
/// after every other document line so the row arithmetic of a skip is actually
/// exercised.
#[derive(Default)]
struct RecordingLineAnnotation {
    /// `(line_end_char_idx, line_end_visual_pos, doc_line)`
    virt_line_calls: Rc<RefCell<Vec<(usize, Position, usize)>>>,
    anchors_processed: Rc<RefCell<Vec<usize>>>,
}

impl LineAnnotation for RecordingLineAnnotation {
    fn insert_virtual_lines(
        &mut self,
        line_end_char_idx: usize,
        line_end_visual_pos: Position,
        doc_line: usize,
    ) -> Position {
        self.virt_line_calls
            .borrow_mut()
            .push((line_end_char_idx, line_end_visual_pos, doc_line));
        Position::new(doc_line % 2, 0)
    }

    fn process_anchor(&mut self, grapheme: &crate::doc_formatter::FormattedGrapheme) -> usize {
        self.anchors_processed.borrow_mut().push(grapheme.char_idx);
        usize::MAX
    }
}

#[test]
fn skip_to_next_line_preserves_virtual_line_rows() {
    for (name, text) in fixtures() {
        if text.chars().count() < 4 {
            continue;
        }

        let run = |clip: usize, skip: bool| {
            let calls = Rc::new(RefCell::new(Vec::new()));
            let anchors = Rc::new(RefCell::new(Vec::new()));
            let annotation = RecordingLineAnnotation {
                virt_line_calls: Rc::clone(&calls),
                anchors_processed: Rc::clone(&anchors),
            };
            let mut text_annotations = TextAnnotations::default();
            text_annotations.add_line_annotation(Box::new(annotation));
            let text_fmt = TextFormat::new_test(false);
            let observed = if skip {
                clipped_graphemes(&text, &text_fmt, &text_annotations, clip)
            } else {
                unclipped_graphemes(&text, &text_fmt, &text_annotations, clip)
            };
            let calls = calls.borrow().clone();
            (observed, calls)
        };

        for clip in [1, 2, 3, 5, 17] {
            let (want_graphemes, want_calls) = run(clip, false);
            let (got_graphemes, got_calls) = run(clip, true);

            assert_eq!(
                want_graphemes, got_graphemes,
                "visible graphemes diverged for {name:?} at clip={clip} with virtual lines"
            );

            // `insert_virtual_lines` must be called once per line, for the same
            // line, at the same char index and on the same *row*. Only the
            // column is allowed to differ - that is the documented
            // approximation of the horizontal clip.
            assert_eq!(
                want_calls.len(),
                got_calls.len(),
                "virtual line call count diverged for {name:?} at clip={clip}"
            );
            for (want, got) in want_calls.iter().zip(got_calls.iter()) {
                assert_eq!(
                    (want.0, want.1.row, want.2),
                    (got.0, got.1.row, got.2),
                    "virtual line call diverged for {name:?} at clip={clip}"
                );
            }
        }
    }
}
