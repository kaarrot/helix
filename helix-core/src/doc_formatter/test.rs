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
    let formatter =
        DocumentFormatter::new_at_prev_checkpoint(text.into(), text_fmt, annotations, 0);
    let mut res = Vec::new();
    for g in formatter {
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

/// The guard that stops a skip from discarding a fresh line must not
/// over-refuse: if it did, every differential test above would still pass while
/// the optimisation silently did nothing. These pin when a skip actually fires.
#[test]
fn skip_to_next_line_contract() {
    let text_fmt = TextFormat::new_test(false);
    let annotations = TextAnnotations::default();
    let text = "hello world\nsecond line\nthird\n";

    // mid-line: skips, and reports the line it discarded
    let mut formatter =
        DocumentFormatter::new_at_prev_checkpoint(text.into(), &text_fmt, &annotations, 0);
    formatter.next().unwrap(); // 'h', now mid-line
    let skipped = formatter.skip_to_next_line().expect("should skip mid-line");
    assert_eq!(skipped.line_idx, 0);
    assert_eq!(skipped.row, 0);
    assert_eq!(skipped.line_end_char_idx, 11); // the '\n' of line 0
    assert_eq!(formatter.next_visual_pos(), Position::new(1, 0));
    assert_eq!(formatter.next_char_pos(), 12);

    // immediately afterwards we are at a line start, so a second skip is refused
    assert_eq!(formatter.skip_to_next_line(), None);

    // ... but resumes once a grapheme of the new line has been consumed
    formatter.next().unwrap();
    assert!(formatter.skip_to_next_line().is_some());

    // a freshly constructed formatter is at a line start too
    let mut formatter =
        DocumentFormatter::new_at_prev_checkpoint(text.into(), &text_fmt, &annotations, 0);
    assert_eq!(formatter.skip_to_next_line(), None);

    // the last line has no line break to skip over
    let mut formatter =
        DocumentFormatter::new_at_prev_checkpoint("only line".into(), &text_fmt, &annotations, 0);
    formatter.next().unwrap();
    assert_eq!(formatter.skip_to_next_line(), None);

    // soft wrap is never skipped
    let soft = TextFormat::new_test(true);
    let mut formatter =
        DocumentFormatter::new_at_prev_checkpoint(text.into(), &soft, &annotations, 0);
    formatter.next().unwrap();
    assert_eq!(formatter.skip_to_next_line(), None);
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

// ---------------------------------------------------------------------------
// Sub-line blocks
//
// Long soft wrapped lines are divided into blocks so a formatter can start near
// any position instead of walking there from the line start. That is only sound
// if the layout is the same either way, so the tests below assert exactly that:
// a formatter constructed anywhere must yield the identical tail of what one
// full traversal produced.
//
// The block size is shrunk to a handful of chars so small fixtures cover every
// alignment of a boundary against tabs, wide graphemes, cluster breaks and
// annotations.
// ---------------------------------------------------------------------------

use crate::doc_formatter::block_size;

/// Everything about a yielded grapheme that callers can observe, except the
/// visual row -- which is block relative and so compared separately.
type Traced = (usize, usize, usize, String, bool);

fn trace(
    text: &str,
    text_fmt: &TextFormat,
    annotations: &TextAnnotations,
    start: usize,
) -> (Vec<Traced>, Vec<usize>) {
    let formatter =
        DocumentFormatter::new_at_prev_checkpoint(text.into(), text_fmt, annotations, start);
    let mut observed = Vec::new();
    let mut rows = Vec::new();
    for g in formatter {
        observed.push((
            g.char_idx,
            g.line_idx,
            g.visual_pos.col,
            g.raw.to_string(),
            g.is_virtual(),
        ));
        rows.push(g.visual_pos.row);
    }
    (observed, rows)
}

/// A formatter started at `start` must produce the exact tail of a full
/// traversal, with visual rows shifted by a single constant -- the row the
/// block begins on.
fn assert_blocks_agree(
    name: &str,
    text: &str,
    text_fmt: &TextFormat,
    annotations: &TextAnnotations,
) {
    let (full, full_rows) = trace(text, text_fmt, annotations, 0);
    let len_chars = text.chars().count();

    for start in 0..=len_chars {
        let (tail, tail_rows) = trace(text, text_fmt, annotations, start);
        assert!(
            tail.len() <= full.len(),
            "{name}: starting at {start} produced more graphemes than a full traversal"
        );
        let offset = full.len() - tail.len();

        assert_eq!(
            &full[offset..],
            &tail[..],
            "{name}: starting at {start} diverged from a full traversal (block starts at \
             char {:?})",
            tail.first().map(|g| g.0)
        );

        // Rows are relative to the start of the block, so the two traversals may
        // disagree by a constant -- but only by a constant. Deriving the shift from
        // the alignment point rather than asserting the block starts at row 0: a
        // block that begins with a word too wide for the viewport legitimately
        // opens on row 1, since `wrap_word` moves the whole word down.
        let Some(&first_tail_row) = tail_rows.first() else {
            continue;
        };
        let shift = full_rows[offset]
            .checked_sub(first_tail_row)
            .unwrap_or_else(|| {
                panic!(
                    "{name}: block starting at {start} opens on row {first_tail_row}, past its \
                 row {} in a full traversal",
                    full_rows[offset]
                )
            });
        for (i, (&want, &got)) in full_rows[offset..].iter().zip(tail_rows.iter()).enumerate() {
            assert_eq!(
                want,
                got + shift,
                "{name}: row diverged at grapheme {i} when starting at {start}"
            );
        }
    }
}

fn block_fixtures() -> Vec<(&'static str, String)> {
    vec![
        (
            "ascii words",
            "foo bar baz qux quux corge grault ".repeat(4),
        ),
        // no whitespace at all: every boundary falls back to punctuation, and
        // for `nowrap` to nothing at all -- the mid-word break path
        ("minified", "{\"a\":1,\"bb\":22,\"ccc\":333}".repeat(6)),
        ("nowrap", "a".repeat(120)),
        // double width, so a boundary can land between the two cells of a cell pair
        ("cjk", "\u{4f60}\u{597d}".repeat(40)),
        // multi-char clusters, the case where snapping to a cluster boundary moves
        ("combining", "a\u{310}e\u{301}o\u{332} ".repeat(24)),
        // regional indicators: cluster breaks depend on the parity of preceding
        // RI codepoints, so re-segmenting from a mid-line boundary is the risk
        ("flags", "\u{1F1E6}\u{1F1E7}\u{1F1E8}\u{1F1E9} ".repeat(16)),
        ("tabs", "\tfoo\tbar\t".repeat(16)),
        ("indented", format!("        {}", "word ".repeat(30))),
        ("multi line", "alpha beta gamma delta\n".repeat(12)),
        ("crlf", "alpha beta gamma delta\r\n".repeat(12)),
        (
            "empty lines",
            format!("{}\n\n\n{}\n", "x y ".repeat(20), "z w ".repeat(20)),
        ),
        (
            "no trailing newline",
            format!("{}{}", "alpha beta gamma\n".repeat(8), "tail ".repeat(20)),
        ),
    ]
}

#[test]
fn blocks_agree_with_full_traversal() {
    for size in [3usize, 5, 13] {
        block_size::with(size, || {
            for (name, text) in block_fixtures() {
                for wrap_indicator in ["", ".", "\u{21aa} "] {
                    for viewport_width in [5u16, 17] {
                        let text_fmt = TextFormat {
                            wrap_indicator: wrap_indicator.into(),
                            viewport_width,
                            ..TextFormat::new_test(true)
                        };
                        assert_blocks_agree(
                            &format!(
                                "{name} (block={size}, indicator={wrap_indicator:?}, \
                                 width={viewport_width})"
                            ),
                            &text,
                            &text_fmt,
                            &TextAnnotations::default(),
                        );
                    }
                }
            }
        });
    }
}

#[test]
fn blocks_agree_with_varied_wrap_settings() {
    for size in [4usize, 9] {
        block_size::with(size, || {
            for (name, text) in block_fixtures() {
                // `max_wrap` below a grapheme's width trips a pre-existing bug in
                // `advance_to_next_word` (it pops its only buffered grapheme and returns
                // an empty buffer, ending the iterator) -- unrelated to blocks.
                for max_wrap in [3u16, 20] {
                    for max_indent_retain in [0u16, 4, 40] {
                        for soft_wrap_at_text_width in [false, true] {
                            let text_fmt = TextFormat {
                                max_wrap,
                                max_indent_retain,
                                soft_wrap_at_text_width,
                                ..TextFormat::new_test(true)
                            };
                            assert_blocks_agree(
                                &format!(
                                    "{name} (block={size}, max_wrap={max_wrap}, \
                                     indent_retain={max_indent_retain}, \
                                     at_text_width={soft_wrap_at_text_width})"
                                ),
                                &text,
                                &text_fmt,
                                &TextAnnotations::default(),
                            );
                        }
                    }
                }
            }
        });
    }
}

#[test]
fn blocks_agree_with_annotations() {
    for size in [4usize, 7, 11] {
        block_size::with(size, || {
            for (name, text) in block_fixtures() {
                let len = text.chars().count();
                if len < 3 * size {
                    continue;
                }
                // anchor annotations either side of, and exactly on, a boundary
                let rope = crate::Rope::from(text.as_str());
                for delta in [-1i64, 0, 1] {
                    let at = (2 * size) as i64 + delta;
                    let at = at.clamp(0, len as i64 - 1) as usize;
                    // Annotations must sit on a grapheme cluster boundary. `Layer::consume`
                    // only advances on an exact `char_idx` match, so one anchored inside a
                    // cluster is never consumed and its layer stays wedged -- a pre-existing
                    // constraint on annotation producers, not something blocks introduce.
                    let at = crate::graphemes::ensure_grapheme_boundary_next(rope.slice(..), at);

                    let inline = [
                        InlineAnnotation::new(at, "VIRT"),
                        InlineAnnotation::new(at, "MORE VIRTUAL TEXT"),
                    ];
                    let text_fmt = TextFormat::new_test(true);
                    assert_blocks_agree(
                        &format!("{name} (block={size}, inline at {at})"),
                        &text,
                        &text_fmt,
                        TextAnnotations::default().add_inline_annotations(&inline, None),
                    );

                    // An overlay replacing a line break stops `Iterator::next` running its
                    // line break bookkeeping at all, so `line_idx` stops advancing -- the
                    // same case `skip_to_next_line` refuses to handle. Pre-existing, and
                    // not what overlays are used for. `at` must therefore land on the
                    // line's text, not inside its terminator (which is two chars for CRLF).
                    let at_line = rope.char_to_line(at);
                    let at_line_end =
                        crate::line_ending::line_end_char_index(&rope.slice(..), at_line);
                    if at < at_line_end {
                        let overlay = [Overlay::new(at, "#")];
                        assert_blocks_agree(
                            &format!("{name} (block={size}, overlay at {at})"),
                            &text,
                            &text_fmt,
                            TextAnnotations::default().add_overlay(&overlay, None),
                        );
                    }
                }
            }
        });
    }
}

/// Blocks exist only under soft wrap. With it off a document line is one visual
/// row, so a forced sub-line break would create a second row for a single line
/// and silently break `skip_to_next_line` and the renderer's horizontal clip.
#[test]
fn no_blocks_without_soft_wrap() {
    block_size::with(4, || {
        for (name, text) in block_fixtures() {
            let text_fmt = TextFormat::new_test(false);
            let annotations = TextAnnotations::default();
            let (full, _) = trace(&text, &text_fmt, &annotations, 0);
            for start in 0..=text.chars().count() {
                let (tail, _) = trace(&text, &text_fmt, &annotations, start);
                let offset = full.len() - tail.len();
                assert_eq!(&full[offset..], &tail[..], "{name}: diverged at {start}");
                // the block must still be the whole document line
                let line_start: usize = text
                    .char_indices()
                    .take_while(|(i, _)| *i < start)
                    .filter(|(_, c)| *c == '\n')
                    .count();
                assert_eq!(
                    tail.first().map(|g| g.1),
                    Some(line_start).filter(|_| !tail.is_empty()),
                    "{name}: block at {start} is not a whole line"
                );
            }
        }
    });
}
