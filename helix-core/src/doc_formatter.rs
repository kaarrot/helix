//! The `DocumentFormatter` forms the bridge between the raw document text
//! and onscreen positioning. It yields the text graphemes as an iterator
//! and traverses (part) of the document text. During that traversal it
//! handles grapheme detection, softwrapping and annotations.
//! It yields `FormattedGrapheme`s and their corresponding visual coordinates.
//!
//! As both virtual text and softwrapping can insert additional lines into the document
//! it is generally not possible to find the start of the previous visual line.
//! Instead the `DocumentFormatter` starts at the last "checkpoint" (usually a linebreak)
//! called a "block" and the caller must advance it as needed.
//!
//! A block is usually a whole document line. Soft wrapped lines longer than
//! [`BLOCK_CHARS`] are additionally divided into blocks of roughly that size, so that
//! starting anywhere in such a line costs a bounded traversal instead of one proportional
//! to how far into the line the position sits. Each of those boundaries is a forced visual
//! line break, which is what makes the layout produced by *starting* at a boundary
//! identical to the layout produced by *iterating through* it -- the invariant the whole
//! scheme rests on.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::fmt::Debug;
use std::mem::replace;

#[cfg(test)]
mod test;

use unicode_segmentation::{Graphemes, UnicodeSegmentation};

use helix_stdx::rope::{RopeGraphemes, RopeSliceExt};

use crate::chars::char_is_word;
use crate::graphemes::{self, Grapheme, GraphemeStr};
use crate::syntax::Highlight;
use crate::text_annotations::TextAnnotations;
use crate::{Position, RopeSlice};

/// TODO make Highlight a u32 to reduce the size of this enum to a single word.
#[derive(Debug, Clone, Copy)]
pub enum GraphemeSource {
    Document {
        codepoints: u32,
    },
    /// Inline virtual text can not be highlighted with a `Highlight` iterator
    /// because it's not part of the document. Instead the `Highlight`
    /// is emitted right by the document formatter
    VirtualText {
        highlight: Option<Highlight>,
    },
}

impl GraphemeSource {
    /// Returns whether this grapheme is virtual inline text
    pub fn is_virtual(self) -> bool {
        matches!(self, GraphemeSource::VirtualText { .. })
    }

    pub fn is_eof(self) -> bool {
        // all doc chars except the EOF char have non-zero codepoints
        matches!(self, GraphemeSource::Document { codepoints: 0 })
    }

    pub fn doc_chars(self) -> usize {
        match self {
            GraphemeSource::Document { codepoints } => codepoints as usize,
            GraphemeSource::VirtualText { .. } => 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FormattedGrapheme<'a> {
    pub raw: Grapheme<'a>,
    pub source: GraphemeSource,
    pub visual_pos: Position,
    /// Document line at the start of the grapheme
    pub line_idx: usize,
    /// Document char position at the start of the grapheme
    pub char_idx: usize,
}

impl FormattedGrapheme<'_> {
    pub fn is_virtual(&self) -> bool {
        self.source.is_virtual()
    }

    pub fn doc_chars(&self) -> usize {
        self.source.doc_chars()
    }

    pub fn is_whitespace(&self) -> bool {
        self.raw.is_whitespace()
    }

    pub fn width(&self) -> usize {
        self.raw.width()
    }

    pub fn is_word_boundary(&self) -> bool {
        self.raw.is_word_boundary()
    }
}

#[derive(Debug, Clone)]
struct GraphemeWithSource<'a> {
    grapheme: Grapheme<'a>,
    source: GraphemeSource,
}

impl<'a> GraphemeWithSource<'a> {
    fn new(
        g: GraphemeStr<'a>,
        visual_x: usize,
        tab_width: u16,
        source: GraphemeSource,
    ) -> GraphemeWithSource<'a> {
        GraphemeWithSource {
            grapheme: Grapheme::new(g, visual_x, tab_width),
            source,
        }
    }
    fn placeholder() -> Self {
        GraphemeWithSource {
            grapheme: Grapheme::Other { g: " ".into() },
            source: GraphemeSource::Document { codepoints: 0 },
        }
    }

    fn doc_chars(&self) -> usize {
        self.source.doc_chars()
    }

    fn is_whitespace(&self) -> bool {
        self.grapheme.is_whitespace()
    }

    fn is_newline(&self) -> bool {
        matches!(self.grapheme, Grapheme::Newline)
    }

    fn is_eof(&self) -> bool {
        self.source.is_eof()
    }

    fn width(&self) -> usize {
        self.grapheme.width()
    }

    fn is_word_boundary(&self) -> bool {
        self.grapheme.is_word_boundary()
    }
}

#[derive(Debug, Clone)]
pub struct TextFormat {
    pub soft_wrap: bool,
    pub tab_width: u16,
    pub max_wrap: u16,
    /// How much of a line's indentation soft wrapped rows carry over.
    ///
    /// Carry over is dropped after the first block of a line (see [`BLOCK_CHARS`]): a
    /// formatter starting mid-line cannot recover the indentation without walking back to
    /// the line start, so both paths pin it to 0 instead. Only affects lines long enough to
    /// be divided, which are generated or minified content.
    pub max_indent_retain: u16,
    pub wrap_indicator: Box<str>,
    pub wrap_indicator_highlight: Option<Highlight>,
    pub viewport_width: u16,
    pub soft_wrap_at_text_width: bool,
}

// test implementation is basically only used for testing or when softwrap is always disabled
impl Default for TextFormat {
    fn default() -> Self {
        TextFormat {
            soft_wrap: false,
            tab_width: 4,
            max_wrap: 3,
            max_indent_retain: 4,
            wrap_indicator: Box::from(" "),
            viewport_width: 17,
            wrap_indicator_highlight: None,
            soft_wrap_at_text_width: false,
        }
    }
}

/// Describes the document line that [`DocumentFormatter::skip_to_next_line`] discarded.
///
/// The fields describe the state the formatter *would* have reached had the remainder of
/// the line been iterated normally, so callers that track the last traversed grapheme can
/// keep their bookkeeping exact across a skip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkippedLine {
    /// The visual row the skipped line occupied, relative to the start of the block.
    pub row: usize,
    /// The char index of the line break that terminated the skipped line. This is the
    /// `char_idx` that the line's last non-virtual `FormattedGrapheme` would have carried.
    pub line_end_char_idx: usize,
    /// The document line index of the skipped line.
    pub line_idx: usize,
}

/// The number of document chars after which a soft wrapped line is split into a new block.
///
/// A block is the unit the formatter can start iterating at, so this is the worst case
/// number of graphemes any position query has to walk. Without it a query for a position
/// deep inside a line walks the whole line prefix, which is what makes vertical motion on
/// multi-megabyte lines lag.
///
/// The cost of a block is one forced visual line break, so this trades a layout artifact
/// (one short row every `BLOCK_CHARS` chars) against that worst case. Only lines longer
/// than this are affected at all.
const BLOCK_CHARS: usize = 4096;

/// How far past the arithmetic block boundary [`block_boundary`] looks for a word boundary
/// before giving up and breaking mid-word.
///
/// Must be smaller than the block size so that boundaries stay strictly increasing in `k`,
/// which [`boundary_search_window`] enforces.
const BOUNDARY_SEARCH_WINDOW: usize = 64;

#[cfg(not(test))]
#[inline(always)]
fn block_chars() -> usize {
    BLOCK_CHARS
}

/// Tests shrink the block size so that boundaries fall every few chars and every alignment
/// against tabs, wide graphemes, cluster breaks and annotations can be covered exhaustively
/// by small fixtures. In a real build this is a plain constant.
#[cfg(test)]
pub(crate) mod block_size {
    use std::cell::Cell;

    thread_local! {
        static OVERRIDE: Cell<usize> = const { Cell::new(super::BLOCK_CHARS) };
    }

    pub(crate) fn get() -> usize {
        OVERRIDE.with(|size| size.get())
    }

    /// Runs `f` with the block size set to `size`, restoring it afterwards.
    pub(crate) fn with<R>(size: usize, f: impl FnOnce() -> R) -> R {
        assert!(size >= 2, "a block must be able to hold a grapheme");
        let previous = OVERRIDE.with(|current| current.replace(size));
        let result = f();
        OVERRIDE.with(|current| current.set(previous));
        result
    }
}

#[cfg(test)]
fn block_chars() -> usize {
    block_size::get()
}

/// The search window, clamped so it can never reach the next block. Without the clamp a
/// boundary could overtake its successor and `boundary_after` would stop making progress.
fn boundary_search_window() -> usize {
    BOUNDARY_SEARCH_WINDOW.min(block_chars() / 2)
}

/// The char index at which the `k`th block of the line starting at `line_start` begins.
///
/// This is a pure function of its arguments, which is what lets
/// [`DocumentFormatter::new_at_prev_checkpoint`] jump straight to a boundary and the
/// iteration path force a break at the same char without the two ever disagreeing. Every
/// property the formatter relies on comes from that:
///
/// * `k == 0` is the line start, so short lines behave exactly as they did before blocks.
/// * strictly increasing in `k` (because `BOUNDARY_SEARCH_WINDOW < BLOCK_CHARS`), so
///   advancing to the next boundary always makes progress.
/// * always on a grapheme cluster boundary, so re-seeding the grapheme iterator there
///   cannot split a cluster.
/// * clamped to `line_end`, so a boundary never lands on or past the line break. The line
///   break already ends the block.
///
/// It must also depend on nothing but the raw document text. In particular it must not
/// consult [`TextAnnotations`]: `advance_grapheme` substitutes overlays for document
/// graphemes, so a boundary derived from formatted graphemes would move when jump labels
/// appear and the layout would shift under the user. Hence the raw `char_is_word` below
/// rather than `FormattedGrapheme::is_word_boundary`.
fn block_boundary(text: RopeSlice, line_start: usize, line_end: usize, k: usize) -> Option<usize> {
    if k == 0 {
        return Some(line_start);
    }
    let base = line_start + k * block_chars();
    if base >= line_end {
        return None;
    }

    // Break at a word boundary rather than mid-word so the forced break reads as an
    // ordinary short row. `char_is_word` is the same predicate behind
    // `Grapheme::is_word_boundary`, so whitespace *and* punctuation qualify -- even
    // minified JSON finds a boundary within a few chars.
    let limit = (base + boundary_search_window()).min(line_end);
    // `chars_at` rather than `text.char(idx)` per step: the latter is a rope descent each
    // time, and this runs on every formatter construction on a long line.
    let mut chars = text.chars_at(base);
    let mut idx = base;
    while idx < limit && chars.next().is_some_and(char_is_word) {
        idx += 1;
    }

    // Snapping forward can push the boundary onto the line break, where it would force a
    // second break for a line that already ends there.
    let boundary = graphemes::ensure_grapheme_boundary_next(text, idx);
    (boundary < line_end).then_some(boundary)
}

#[derive(Debug)]
pub struct DocumentFormatter<'t> {
    text_fmt: &'t TextFormat,
    annotations: &'t TextAnnotations<'t>,

    /// The document text. Only needed to re-seed `graphemes` in
    /// [`DocumentFormatter::skip_to_next_line`].
    text: RopeSlice<'t>,
    /// The visual position at the end of the last yielded word boundary
    visual_pos: Position,
    graphemes: RopeGraphemes<'t>,
    /// The character pos of the `graphemes` iter used for inserting annotations
    char_pos: usize,
    /// The line pos of the `graphemes` iter used for inserting annotations
    line_pos: usize,
    exhausted: bool,

    inline_annotation_graphemes: Option<(Graphemes<'t>, Option<Highlight>)>,

    // softwrap specific
    /// The indentation of the current line
    /// Is set to `None` if the indentation level is not yet known
    /// because no non-whitespace graphemes have been encountered yet
    indent_level: Option<usize>,
    /// In case a long word needs to be split a single grapheme might need to be wrapped
    /// while the rest of the word stays on the same line
    peeked_grapheme: Option<GraphemeWithSource<'t>>,
    /// A first-in first-out (fifo) buffer for the Graphemes of any given word
    word_buf: Vec<GraphemeWithSource<'t>>,
    /// The index of the next grapheme that will be yielded from the `word_buf`
    word_i: usize,
    /// The char index at which the current document line starts. Cached so
    /// [`block_boundary`] does not re-derive it per grapheme; refreshed on line transitions.
    line_start: usize,
    /// Where the current document line ends, or `None` when it provably has no block
    /// boundary. See [`DocumentFormatter::block_line_end`].
    line_end: Option<usize>,
    /// The char index of the next forced block break, or `usize::MAX` when there is none
    /// (soft wrap off, or the current line has no further boundary) so the hot path check
    /// is a single compare.
    next_block_boundary: usize,
    /// Set when the formatter was constructed *at* a block boundary rather than at a line
    /// start. The first call to `advance_to_next_word` then reproduces the forced break
    /// that continuous iteration would have performed, minus the row advance -- the block
    /// it is starting is row 0.
    pending_block_start: bool,
}

impl<'t> DocumentFormatter<'t> {
    /// Creates a new formatter at the last block before `char_idx`.
    /// A block is a chunk which always ends with a linebreak.
    /// This is usually just a normal line break.
    /// However very long lines are always wrapped at constant intervals that can be cheaply
    /// calculated to avoid pathological behaviour -- see [`BLOCK_CHARS`].
    pub fn new_at_prev_checkpoint(
        text: RopeSlice<'t>,
        text_fmt: &'t TextFormat,
        annotations: &'t TextAnnotations,
        char_idx: usize,
    ) -> Self {
        let char_idx = char_idx.min(text.len_chars());
        let block_line_idx = text.char_to_line(char_idx);
        let line_start = text.line_to_char(block_line_idx);
        let line_end = Self::block_line_end(text, text_fmt, line_start, block_line_idx);
        let block_char_idx = Self::block_start_in(text, line_start, line_end, char_idx);
        let pending_block_start = block_char_idx != line_start;

        annotations.reset_pos(block_char_idx);

        DocumentFormatter {
            text_fmt,
            annotations,
            text,
            visual_pos: Position { row: 0, col: 0 },
            graphemes: text.slice(block_char_idx..).graphemes(),
            char_pos: block_char_idx,
            exhausted: false,
            // A block after the first in a line does not carry the line's indentation
            // over: reconstructing it would mean walking back to the line start, and
            // virtual text there would make that walk disagree with what continuous
            // iteration computed. `advance_to_next_word` drops it on the iterating path
            // too, so both agree on `Some(0)`.
            indent_level: pending_block_start.then_some(0),
            peeked_grapheme: None,
            word_buf: Vec::with_capacity(64),
            word_i: 0,
            line_pos: block_line_idx,
            inline_annotation_graphemes: None,
            line_start,
            line_end,
            next_block_boundary: Self::boundary_after(text, line_start, line_end, block_char_idx),
            pending_block_start,
        }
    }

    /// The char index at which [`DocumentFormatter::new_at_prev_checkpoint`] would start
    /// iterating for `char_idx`.
    ///
    /// Useful to callers that need to bound work by the same block the formatter will
    /// traverse, such as the syntax highlight range of a frame.
    pub fn block_start(text: RopeSlice, text_fmt: &TextFormat, char_idx: usize) -> usize {
        let char_idx = char_idx.min(text.len_chars());
        let line_idx = text.char_to_line(char_idx);
        let line_start = text.line_to_char(line_idx);
        let line_end = Self::block_line_end(text, text_fmt, line_start, line_idx);
        Self::block_start_in(text, line_start, line_end, char_idx)
    }

    /// Soft wrapped lines longer than a block are divided so that starting at `char_idx`
    /// costs `O(BLOCK_CHARS)` rather than `O(char_idx - line_start)`. With soft wrap off a
    /// line is a single block: there is nothing to gain, since a line is one visual row and
    /// the callers already skip whole lines.
    fn block_start_in(
        text: RopeSlice,
        line_start: usize,
        line_end: Option<usize>,
        char_idx: usize,
    ) -> usize {
        let Some(line_end) = line_end else {
            return line_start;
        };
        if char_idx - line_start < block_chars() {
            return line_start;
        }
        // The boundary is snapped forward to a word boundary and dropped when it would land
        // on the line break, so the `k`th one can be past `char_idx` or absent. Walk back
        // until the block actually contains `char_idx`.
        let mut k = (char_idx - line_start) / block_chars();
        loop {
            if k == 0 {
                return line_start;
            }
            match block_boundary(text, line_start, line_end, k) {
                Some(boundary) if boundary <= char_idx => return boundary,
                _ => k -= 1,
            }
        }
    }

    /// Where the document line `line_idx` ends, or `None` when it provably contains no
    /// block boundary and the block machinery can be skipped entirely.
    ///
    /// `len_chars() - line_start` is an `O(1)` upper bound on the length of the line, so
    /// ordinary documents never pay for the rope query.
    fn block_line_end(
        text: RopeSlice,
        text_fmt: &TextFormat,
        line_start: usize,
        line_idx: usize,
    ) -> Option<usize> {
        if !text_fmt.soft_wrap || text.len_chars() - line_start < block_chars() {
            return None;
        }
        Some(crate::line_ending::line_end_char_index(&text, line_idx))
    }

    /// Refreshes the cached per line state after a line transition. `line_idx` must be the
    /// line the formatter has just moved onto, with `char_pos` at its first char.
    fn enter_line(&mut self, line_idx: usize) {
        self.line_pos = line_idx;
        self.line_start = self.char_pos;
        self.line_end = Self::block_line_end(self.text, self.text_fmt, self.line_start, line_idx);
        self.next_block_boundary =
            Self::boundary_after(self.text, self.line_start, self.line_end, self.char_pos);
    }

    /// The first block boundary strictly after `char_idx` within the line spanning
    /// `line_start..=line_end`, or `usize::MAX` when the line has none left.
    fn boundary_after(
        text: RopeSlice,
        line_start: usize,
        line_end: Option<usize>,
        char_idx: usize,
    ) -> usize {
        let Some(line_end) = line_end else {
            return usize::MAX;
        };
        // Walk forward from the arithmetic block `char_idx` falls in. Boundaries are
        // snapped forward to a word boundary, so `block_boundary(k)` can still be at or
        // before `char_idx`; it is strictly increasing and gains at least `BLOCK_CHARS`
        // per step, so this terminates after a couple of iterations.
        let mut k = (char_idx - line_start) / block_chars();
        loop {
            match block_boundary(text, line_start, line_end, k) {
                Some(boundary) if boundary > char_idx => return boundary,
                // at or before `char_idx`: this block has already been entered
                Some(_) => k += 1,
                // the line has no `k`th block, and none beyond it either
                None => return usize::MAX,
            }
        }
    }

    fn next_inline_annotation_grapheme(
        &mut self,
        char_pos: usize,
    ) -> Option<(&'t str, Option<Highlight>)> {
        loop {
            if let Some(&mut (ref mut annotation, highlight)) =
                self.inline_annotation_graphemes.as_mut()
            {
                if let Some(grapheme) = annotation.next() {
                    return Some((grapheme, highlight));
                }
            }

            if let Some((annotation, highlight)) =
                self.annotations.next_inline_annotation_at(char_pos)
            {
                self.inline_annotation_graphemes = Some((
                    UnicodeSegmentation::graphemes(&*annotation.text, true),
                    highlight,
                ))
            } else {
                return None;
            }
        }
    }

    fn advance_grapheme(&mut self, col: usize, char_pos: usize) -> Option<GraphemeWithSource<'t>> {
        let (grapheme, source) =
            if let Some((grapheme, highlight)) = self.next_inline_annotation_grapheme(char_pos) {
                (grapheme.into(), GraphemeSource::VirtualText { highlight })
            } else if let Some(grapheme) = self.graphemes.next() {
                let codepoints = grapheme.len_chars() as u32;

                let overlay = self.annotations.overlay_at(char_pos);
                let grapheme = match overlay {
                    Some((overlay, _)) => overlay.grapheme.as_str().into(),
                    None => Cow::from(grapheme).into(),
                };

                (grapheme, GraphemeSource::Document { codepoints })
            } else {
                if self.exhausted {
                    return None;
                }
                self.exhausted = true;
                // EOF grapheme is required for rendering
                // and correct position computations
                return Some(GraphemeWithSource {
                    grapheme: Grapheme::Other { g: " ".into() },
                    source: GraphemeSource::Document { codepoints: 0 },
                });
            };

        let grapheme = GraphemeWithSource::new(grapheme, col, self.text_fmt.tab_width, source);

        Some(grapheme)
    }

    /// Start a new soft wrapped visual row: apply the indent carry over, account for any
    /// virtual lines, and prepend the wrap indicator to `word_buf`.
    ///
    /// Returns the width the indicator occupies, i.e. the column the buffered word now
    /// starts at, and the number of indicator graphemes that were prepended.
    ///
    /// `advance_row` is false only when [`DocumentFormatter::new_at_prev_checkpoint`]
    /// constructed the formatter *at* a block boundary. The row it is starting is row 0 of
    /// that block, and the virtual lines belong to the preceding block -- the same way the
    /// virtual lines after a line break are accounted to the block that contained it.
    fn begin_wrapped_row(&mut self, advance_row: bool) -> (usize, usize) {
        let indent_carry_over = if let Some(indent) = self.indent_level {
            if indent as u16 <= self.text_fmt.max_indent_retain {
                indent as u16
            } else {
                0
            }
        } else {
            // ensure the indent stays 0
            self.indent_level = Some(0);
            0
        };

        if advance_row {
            let virtual_lines =
                self.annotations
                    .virtual_lines_at(self.char_pos, self.visual_pos, self.line_pos);
            self.visual_pos.row += 1 + virtual_lines;
        }
        self.visual_pos.col = indent_carry_over as usize;

        let mut i = 0;
        let mut word_width = 0;
        let wrap_indicator = UnicodeSegmentation::graphemes(&*self.text_fmt.wrap_indicator, true)
            .map(|g| {
                i += 1;
                let grapheme = GraphemeWithSource::new(
                    g.into(),
                    self.visual_pos.col + word_width,
                    self.text_fmt.tab_width,
                    GraphemeSource::VirtualText {
                        highlight: self.text_fmt.wrap_indicator_highlight,
                    },
                );
                word_width += grapheme.width();
                grapheme
            });
        self.word_buf.splice(0..0, wrap_indicator);

        (word_width, i)
    }

    /// Move a word to the next visual line
    fn wrap_word(&mut self) -> usize {
        // softwrap this word to the next line
        let (mut word_width, i) = self.begin_wrapped_row(true);

        for grapheme in &mut self.word_buf[i..] {
            let visual_x = self.visual_pos.col + word_width;
            grapheme
                .grapheme
                .change_position(visual_x, self.text_fmt.tab_width);
            word_width += grapheme.width();
        }
        if let Some(grapheme) = &mut self.peeked_grapheme {
            let visual_x = self.visual_pos.col + word_width;
            grapheme
                .grapheme
                .change_position(visual_x, self.text_fmt.tab_width);
        }
        word_width
    }

    fn peek_grapheme(&mut self, col: usize, char_pos: usize) -> Option<&GraphemeWithSource<'t>> {
        if self.peeked_grapheme.is_none() {
            self.peeked_grapheme = self.advance_grapheme(col, char_pos);
        }
        self.peeked_grapheme.as_ref()
    }

    fn next_grapheme(&mut self, col: usize, char_pos: usize) -> Option<GraphemeWithSource<'t>> {
        self.peek_grapheme(col, char_pos);
        self.peeked_grapheme.take()
    }

    fn advance_to_next_word(&mut self) {
        self.word_buf.clear();
        let mut word_width = 0;
        let mut word_chars = 0;

        if self.exhausted {
            return;
        }

        if self.pending_block_start {
            // This formatter was constructed *at* a block boundary. Reproduce the break
            // that continuous iteration performs there, minus the row advance: the block
            // being started is row 0, and the virtual lines at the boundary belong to the
            // preceding block.
            self.pending_block_start = false;
            word_width = self.begin_wrapped_row(false).0;
        } else if self.char_pos == self.next_block_boundary {
            // Long lines are divided into blocks so that a formatter can be constructed
            // near any position instead of walking there from the line start. That is only
            // sound if the layout is identical whether a boundary is iterated through or
            // jumped to, which means the boundary has to force a break on both paths.
            //
            // `wrap_word` rather than `begin_wrapped_row` so a forced break is byte for
            // byte an ordinary soft wrap.
            //
            // The word buffer was just cleared, and `peeked_grapheme` is always spent by
            // the time a boundary is reached: the only path that leaves one set is the
            // `word_width > max_wrap` arm below, which pops a grapheme from *before* the
            // boundary, so the following call consumes it before reaching the boundary.
            debug_assert!(self.word_buf.is_empty());
            debug_assert!(self.peeked_grapheme.is_none());
            self.indent_level = Some(0);
            self.next_block_boundary =
                Self::boundary_after(self.text, self.line_start, self.line_end, self.char_pos);
            word_width = self.wrap_word();
        }

        loop {
            let mut col = self.visual_pos.col + word_width;
            let char_pos = self.char_pos + word_chars;

            // End the word on the block boundary so the next call starts exactly on it and
            // takes the forced break above. Swallowing the boundary into this word would
            // move the break to wherever the word happened to end -- and only formatters
            // *started* at the boundary would then emit a wrap indicator, drifting the
            // cursor a couple of columns away from what was rendered.
            if char_pos == self.next_block_boundary {
                // Never on the first iteration: the entry check above consumed a boundary
                // at `self.char_pos` and moved `next_block_boundary` past it. Returning
                // with nothing buffered would end the iterator.
                debug_assert!(word_chars != 0 || !self.word_buf.is_empty());
                return;
            }

            match col.cmp(&(self.text_fmt.viewport_width as usize)) {
                // The EOF char and newline chars are always selectable in helix. That means
                // that wrapping happens "too-early" if a word fits a line perfectly. This
                // is intentional so that all selectable graphemes are always visible (and
                // therefore the cursor never disappears). However if the user manually set a
                // lower softwrap width then this is undesirable. Just increasing the viewport-
                // width by one doesn't work because if a line is wrapped multiple times then
                // some words may extend past the specified width.
                //
                // So we special case a word that ends exactly at line bounds and is followed
                // by a newline/eof character here.
                Ordering::Equal
                    if self.text_fmt.soft_wrap_at_text_width
                        && self
                            .peek_grapheme(col, char_pos)
                            .is_some_and(|grapheme| grapheme.is_newline() || grapheme.is_eof()) => {
                }
                Ordering::Equal if word_width > self.text_fmt.max_wrap as usize => return,
                Ordering::Greater if word_width > self.text_fmt.max_wrap as usize => {
                    self.peeked_grapheme = self.word_buf.pop();
                    return;
                }
                Ordering::Equal | Ordering::Greater => {
                    word_width = self.wrap_word();
                    col = self.visual_pos.col + word_width;
                }
                Ordering::Less => (),
            }

            let Some(grapheme) = self.next_grapheme(col, char_pos) else {
                return;
            };
            word_chars += grapheme.doc_chars();

            // Track indentation
            if !grapheme.is_whitespace() && self.indent_level.is_none() {
                self.indent_level = Some(self.visual_pos.col);
            } else if grapheme.grapheme == Grapheme::Newline {
                self.indent_level = None;
            }

            let is_word_boundary = grapheme.is_word_boundary();
            word_width += grapheme.width();
            self.word_buf.push(grapheme);

            if is_word_boundary {
                return;
            }
        }
    }

    /// Discards the remainder of the current document line and repositions the formatter
    /// at the start of the next one, exactly as if every remaining grapheme (including the
    /// line break) had been yielded and thrown away.
    ///
    /// This is `O(log N)` rather than `O(remaining line length)`, which is what makes
    /// rendering and vertical motion independent of line length when soft wrap is off.
    /// Callers must only use it where the discarded graphemes provably cannot affect the
    /// result — because they are past the right edge of the viewport, or on a visual row
    /// that is not the one being searched for.
    ///
    /// Returns `None`, leaving the formatter completely untouched, when the skip does not
    /// apply:
    ///
    /// * soft wrap is enabled — `word_buf`/`peeked_grapheme`/`indent_level` cannot be
    ///   reconstructed, and one document line is not one visual row
    /// * the formatter is exhausted, or is on the last line and so has no line break left
    /// * the formatter is already at the start of a line — the caller has just consumed a
    ///   line break, and skipping would discard the *following* line rather than the tail
    ///   of the one it was looking at
    /// * an [`crate::text_annotations::Overlay`] replaces the line break grapheme.
    ///   `advance_grapheme` substitutes the overlay for *any* char including the line
    ///   feed, and `next` then does not run its line break bookkeeping at all, so skipping
    ///   would desync `visual_pos`.
    ///
    /// # Approximation
    ///
    /// The `line_end_visual_pos` handed to
    /// [`crate::text_annotations::LineAnnotation::insert_virtual_lines`] has an exact
    /// `row` but a `col` equal to the column the caller stopped at rather than the true
    /// width of the line. See the note on that trait method.
    pub fn skip_to_next_line(&mut self) -> Option<SkippedLine> {
        if self.text_fmt.soft_wrap || self.exhausted {
            return None;
        }

        let line_idx = self.line_pos;
        // the last line has no line break to skip over
        if line_idx + 1 >= self.text.len_lines() {
            return None;
        }

        // Refuse when the formatter already sits at the start of a line. The caller has
        // just consumed a line break, so `line_pos` has moved on and the line that would
        // be skipped is a fresh one the caller has not looked at yet. Skipping here would
        // silently discard a whole line of content rather than an off-screen tail.
        if self.char_pos == self.text.line_to_char(line_idx) {
            return None;
        }

        let next_line_char_idx = self.text.line_to_char(line_idx + 1);
        let line_end_char_idx = crate::line_ending::line_end_char_index(&self.text, line_idx);
        // never skip backwards
        if line_end_char_idx < self.char_pos {
            return None;
        }
        if self.annotations.has_overlay_at(line_end_char_idx) {
            return None;
        }

        let row = self.visual_pos.row;

        // Stand in for the `process_virtual_text_anchors` calls that `next` would have
        // made for the discarded graphemes. This must happen before `virtual_lines_at`,
        // matching the order that normal iteration produces.
        self.annotations.skip_to(next_line_char_idx);

        // Reproduce the line break bookkeeping of `Iterator::next`.
        let line_end_visual_pos = Position::new(row, self.visual_pos.col + 1);
        let virtual_lines =
            self.annotations
                .virtual_lines_at(next_line_char_idx, line_end_visual_pos, line_idx);
        self.visual_pos.row = row + 1 + virtual_lines;
        self.visual_pos.col = 0;
        self.line_pos = line_idx + 1;

        // Re-seed the grapheme iterator the same way `new_at_prev_checkpoint` does, so
        // there is no chance of a cluster boundary behaving differently.
        self.char_pos = next_line_char_idx;
        self.graphemes = self.text.slice(next_line_char_idx..).graphemes();
        self.inline_annotation_graphemes = None;
        self.indent_level = None;
        // keeps the cached line state in step with `Iterator::next`. A no-op in practice
        // -- this path only runs with soft wrap off, where there are no blocks -- but the
        // two line transitions must not be allowed to drift apart.
        self.enter_line(line_idx + 1);

        Some(SkippedLine {
            row,
            line_end_char_idx,
            line_idx,
        })
    }

    /// returns the char index at the end of the last yielded grapheme
    pub fn next_char_pos(&self) -> usize {
        self.char_pos
    }
    /// returns the visual position at the end of the last yielded grapheme
    pub fn next_visual_pos(&self) -> Position {
        self.visual_pos
    }
}

impl<'t> Iterator for DocumentFormatter<'t> {
    type Item = FormattedGrapheme<'t>;

    fn next(&mut self) -> Option<Self::Item> {
        let grapheme = if self.text_fmt.soft_wrap {
            if self.word_i >= self.word_buf.len() {
                self.advance_to_next_word();
                self.word_i = 0;
            }
            let grapheme = replace(
                self.word_buf.get_mut(self.word_i)?,
                GraphemeWithSource::placeholder(),
            );
            self.word_i += 1;
            grapheme
        } else {
            self.advance_grapheme(self.visual_pos.col, self.char_pos)?
        };

        let grapheme = FormattedGrapheme {
            raw: grapheme.grapheme,
            source: grapheme.source,
            visual_pos: self.visual_pos,
            line_idx: self.line_pos,
            char_idx: self.char_pos,
        };

        self.char_pos += grapheme.doc_chars();
        if !grapheme.is_virtual() {
            self.annotations.process_virtual_text_anchors(&grapheme);
        }
        if grapheme.raw == Grapheme::Newline {
            // move to end of newline char
            self.visual_pos.col += 1;
            let virtual_lines =
                self.annotations
                    .virtual_lines_at(self.char_pos, self.visual_pos, self.line_pos);
            self.visual_pos.row += 1 + virtual_lines;
            self.visual_pos.col = 0;
            if !grapheme.is_virtual() {
                // `char_pos` was advanced past the line break above, so it is the first
                // char of the new line -- exactly what `enter_line` expects.
                self.enter_line(self.line_pos + 1);
            }
        } else {
            self.visual_pos.col += grapheme.width();
        }
        Some(grapheme)
    }
}
