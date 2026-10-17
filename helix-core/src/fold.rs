//! Folds hide runs of whole lines. The line above a fold stays on screen as its
//! header and the hidden lines take up no rows at all.
//!
//! Every function here takes the folds of one view as a slice that is sorted and
//! disjoint, which is what [`insert`] and [`map`] maintain.

use std::ops::Range;

use crate::movement::Direction;
use crate::{Assoc, ChangeSet, Range as Selected, RopeSlice};

/// A run of hidden lines. Both ends sit at line starts and the line before
/// `start` is the fold's visible header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fold {
    /// The first char of the first hidden line.
    pub start: usize,
    /// The first char after the last hidden line.
    pub end: usize,
}

impl Fold {
    /// Hides the lines after `header_line` up to and including `last_line`.
    ///
    /// The last line of the text has no line break for a fold to end on, so it is
    /// never hidden. Returns `None` when that leaves nothing to hide.
    pub fn new(text: RopeSlice, header_line: usize, last_line: usize) -> Option<Fold> {
        let last_line = last_line.min(text.len_lines().saturating_sub(2));
        (last_line > header_line).then(|| Fold {
            start: text.line_to_char(header_line + 1),
            end: text.line_to_char(last_line + 1),
        })
    }

    pub fn contains(&self, char_idx: usize) -> bool {
        self.start <= char_idx && char_idx < self.end
    }

    pub fn header_line(&self, text: RopeSlice) -> usize {
        text.char_to_line(self.start) - 1
    }

    /// The hidden document lines.
    pub fn lines(&self, text: RopeSlice) -> Range<usize> {
        text.char_to_line(self.start)..text.char_to_line(self.end)
    }
}

/// The fold that hides `char_idx`, if any.
pub fn fold_at(folds: &[Fold], char_idx: usize) -> Option<&Fold> {
    let idx = folds.partition_point(|fold| fold.end <= char_idx);
    folds.get(idx).filter(|fold| fold.start <= char_idx)
}

/// Adds `fold`, merging it with every fold it overlaps. Folds that merely touch
/// are merged too: the header of the later one would be hidden by the earlier one.
pub fn insert(folds: &mut Vec<Fold>, mut fold: Fold) {
    let first = folds.partition_point(|other| other.end < fold.start);
    let mut last = first;
    while let Some(other) = folds.get(last).filter(|other| other.start <= fold.end) {
        fold.start = fold.start.min(other.start);
        fold.end = fold.end.max(other.end);
        last += 1;
    }
    folds.splice(first..last, [fold]);
}

/// Maps `folds` through `changes`. `text` is the text after the change.
///
/// Text inserted right at either end of a fold stays visible. A fold whose ends
/// no longer sit at line starts (for example because its header was joined with
/// the first hidden line) shrinks to the whole lines it still covers, and a fold
/// left with no lines is dropped.
pub fn map(folds: &mut Vec<Fold>, changes: &ChangeSet, text: RopeSlice) {
    if changes.is_empty() || folds.is_empty() {
        return;
    }
    changes.update_positions(
        folds
            .iter_mut()
            .flat_map(|fold| [(&mut fold.start, Assoc::After), (&mut fold.end, Assoc::Before)]),
    );

    // The last line has no line break to end a fold on.
    let max_end = text.line_to_char(text.len_lines() - 1);
    let mapped = std::mem::take(folds);
    for fold in mapped {
        let start = line_start_at_or_after(text, fold.start);
        let end = line_start_at_or_after(text, fold.end).min(max_end);
        if start > 0 && start < end {
            insert(folds, Fold { start, end });
        }
    }
}

fn line_start_at_or_after(text: RopeSlice, char_idx: usize) -> usize {
    let char_idx = char_idx.min(text.len_chars());
    let line = text.char_to_line(char_idx);
    let line_start = text.line_to_char(line);
    if line_start == char_idx {
        char_idx
    } else {
        text.line_to_char(line + 1)
    }
}

/// Grows a selection of whole lines over every fold that it reaches into, so a
/// folded block is selected together with its header line, as if it were part
/// of that line.
pub fn grow_over_folds(folds: &[Fold], text: RopeSlice, range: Selected) -> Selected {
    let from = range.from();
    let to = range.to();
    let new_from = fold_at(folds, from).map_or(from, |fold| text.line_to_char(fold.header_line(text)));
    let new_to = fold_at(folds, to).map_or(to, |fold| fold.end);
    if (new_from, new_to) == (from, to) {
        return range;
    }
    Selected::new(new_from, new_to).with_direction(range.direction())
}

/// Moves `count` lines away from `line`. A fold counts as part of its header's
/// line, so the result is never a hidden line.
pub fn step_lines(
    folds: &[Fold],
    text: RopeSlice,
    line: usize,
    dir: Direction,
    count: usize,
) -> usize {
    let last_line = text.len_lines().saturating_sub(1);
    if folds.is_empty() {
        return match dir {
            Direction::Forward => line.saturating_add(count).min(last_line),
            Direction::Backward => line.saturating_sub(count),
        };
    }

    let mut line = line;
    for _ in 0..count {
        match dir {
            Direction::Forward => {
                if line >= last_line {
                    break;
                }
                line += 1;
                if let Some(fold) = fold_at(folds, text.line_to_char(line)) {
                    line = text.char_to_line(fold.end);
                }
            }
            Direction::Backward => {
                if line == 0 {
                    break;
                }
                line -= 1;
                if let Some(fold) = fold_at(folds, text.line_to_char(line)) {
                    line = fold.header_line(text);
                }
            }
        }
    }
    line
}

/// The distance between two lines when every fold counts as part of its header's
/// line, which is how far [`step_lines`] has to go to get from one to the other.
pub fn line_distance(folds: &[Fold], text: RopeSlice, a: usize, b: usize) -> usize {
    let (low, high) = (a.min(b), a.max(b));
    let (low_char, high_char) = (text.line_to_char(low), text.line_to_char(high));
    let hidden: usize = folds[folds.partition_point(|fold| fold.end <= low_char)..]
        .iter()
        .take_while(|fold| fold.start < high_char)
        .map(|fold| {
            let lines = fold.lines(text);
            lines.end.min(high).saturating_sub(lines.start.max(low))
        })
        .sum();
    high - low - hidden
}

/// The number of document lines, starting at `first_line`, that it takes to fill
/// `rows` screen rows when every fold collapses into its header's row. The result
/// may run past the end of the text.
pub fn lines_for_rows(folds: &[Fold], text: RopeSlice, first_line: usize, rows: usize) -> usize {
    let first_char = text.line_to_char(first_line.min(text.len_lines()));
    let mut line = first_line;
    let mut rows_left = rows;
    for fold in &folds[folds.partition_point(|fold| fold.end <= first_char)..] {
        let hidden = fold.lines(text);
        let visible = hidden.start.saturating_sub(line);
        if visible >= rows_left {
            break;
        }
        rows_left -= visible;
        line = hidden.end;
    }
    line + rows_left - first_line
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{Rope, Transaction};

    fn text(lines: usize) -> Rope {
        let mut text = String::new();
        for i in 0..lines {
            text.push_str(&format!("line {i}\n"));
        }
        Rope::from(text)
    }

    fn lines(text: &Rope, folds: &[Fold]) -> Vec<Range<usize>> {
        folds.iter().map(|fold| fold.lines(text.slice(..))).collect()
    }

    #[test]
    fn new_hides_the_lines_below_the_header() {
        let text = text(10);
        let fold = Fold::new(text.slice(..), 2, 5).unwrap();
        assert_eq!(fold.lines(text.slice(..)), 3..6);
        assert_eq!(fold.header_line(text.slice(..)), 2);
        assert!(Fold::new(text.slice(..), 2, 2).is_none());
    }

    #[test]
    fn new_never_hides_the_last_line() {
        // "line 9\n" is followed by the empty last line, so line 9 can be hidden
        let text = text(10);
        let fold = Fold::new(text.slice(..), 7, 100).unwrap();
        assert_eq!(fold.lines(text.slice(..)), 8..10);
        assert_eq!(fold.end, text.len_chars());

        // without a trailing line break the last line stays visible
        let text = Rope::from("a\nb\nc");
        let fold = Fold::new(text.slice(..), 0, 2).unwrap();
        assert_eq!(fold.lines(text.slice(..)), 1..2);
        assert!(Fold::new(text.slice(..), 1, 2).is_none());
    }

    #[test]
    fn fold_at_finds_hidden_chars_only() {
        let text = text(10);
        let slice = text.slice(..);
        let folds = [Fold::new(slice, 1, 3).unwrap(), Fold::new(slice, 5, 7).unwrap()];
        assert_eq!(fold_at(&folds, slice.line_to_char(1)), None);
        assert_eq!(fold_at(&folds, slice.line_to_char(2)), Some(&folds[0]));
        assert_eq!(fold_at(&folds, slice.line_to_char(4) - 1), Some(&folds[0]));
        assert_eq!(fold_at(&folds, slice.line_to_char(4)), None);
        assert_eq!(fold_at(&folds, slice.line_to_char(7)), Some(&folds[1]));
        assert_eq!(fold_at(&folds, slice.line_to_char(8)), None);
    }

    #[test]
    fn insert_keeps_folds_sorted_and_merges_overlaps() {
        let text = text(20);
        let slice = text.slice(..);
        let mut folds = Vec::new();
        insert(&mut folds, Fold::new(slice, 10, 12).unwrap());
        insert(&mut folds, Fold::new(slice, 1, 3).unwrap());
        insert(&mut folds, Fold::new(slice, 5, 6).unwrap());
        assert_eq!(lines(&text, &folds), [2..4, 6..7, 11..13]);

        // the header of this one is hidden by the first fold
        insert(&mut folds, Fold::new(slice, 3, 4).unwrap());
        assert_eq!(lines(&text, &folds), [2..5, 6..7, 11..13]);

        // covering several folds swallows them
        insert(&mut folds, Fold::new(slice, 5, 15).unwrap());
        assert_eq!(lines(&text, &folds), [2..5, 6..16]);
    }

    #[test]
    fn map_keeps_insertions_at_the_ends_visible() {
        let mut text = text(10);
        let mut folds = vec![Fold::new(text.slice(..), 2, 4).unwrap()];

        // typing at the start of the first line after the fold
        let at = text.line_to_char(5);
        let transaction = Transaction::change(&text, [(at, at, Some("new\n".into()))].into_iter());
        transaction.apply(&mut text);
        map(&mut folds, transaction.changes(), text.slice(..));
        assert_eq!(lines(&text, &folds), [3..5]);

        // inserting lines above shifts the fold
        let transaction = Transaction::change(&text, [(0, 0, Some("a\nb\n".into()))].into_iter());
        transaction.apply(&mut text);
        map(&mut folds, transaction.changes(), text.slice(..));
        assert_eq!(lines(&text, &folds), [5..7]);
    }

    #[test]
    fn map_shrinks_a_fold_whose_header_is_joined() {
        let mut text = text(10);
        let mut folds = vec![Fold::new(text.slice(..), 2, 5).unwrap()];

        // delete the header's line break, pulling the first hidden line up
        let at = text.line_to_char(3) - 1;
        let transaction = Transaction::change(&text, [(at, at + 1, None)].into_iter());
        transaction.apply(&mut text);
        map(&mut folds, transaction.changes(), text.slice(..));
        assert_eq!(lines(&text, &folds), [3..5]);
    }

    #[test]
    fn map_drops_a_fold_with_nothing_left() {
        let mut text = text(10);
        let mut folds = vec![Fold::new(text.slice(..), 2, 3).unwrap()];
        let (from, to) = (text.line_to_char(3), text.line_to_char(4));
        let transaction = Transaction::change(&text, [(from, to, None)].into_iter());
        transaction.apply(&mut text);
        map(&mut folds, transaction.changes(), text.slice(..));
        assert!(folds.is_empty());
    }

    #[test]
    fn grow_over_folds_takes_header_and_block_together() {
        let text = text(10);
        let slice = text.slice(..);
        let folds = [Fold::new(slice, 2, 4).unwrap()];
        let line = |line| slice.line_to_char(line);

        // the header line alone grows down over the block
        let range = grow_over_folds(&folds, slice, Selected::new(line(2), line(3)));
        assert_eq!(range, Selected::new(line(2), line(5)));
        // reaching up into the block takes the header too
        let range = grow_over_folds(&folds, slice, Selected::new(line(7), line(4)));
        assert_eq!(range, Selected::new(line(7), line(2)));
        // ranges that stay clear of the fold are left alone
        let range = Selected::new(line(5), line(7));
        assert_eq!(grow_over_folds(&folds, slice, range), range);
        let range = Selected::new(line(0), line(2));
        assert_eq!(grow_over_folds(&folds, slice, range), range);
    }

    #[test]
    fn step_lines_jumps_over_folds() {
        let text = text(20);
        let slice = text.slice(..);
        let folds = [Fold::new(slice, 2, 5).unwrap(), Fold::new(slice, 6, 9).unwrap()];

        assert_eq!(step_lines(&folds, slice, 1, Direction::Forward, 1), 2);
        assert_eq!(step_lines(&folds, slice, 2, Direction::Forward, 1), 6);
        assert_eq!(step_lines(&folds, slice, 2, Direction::Forward, 2), 10);
        assert_eq!(step_lines(&folds, slice, 10, Direction::Backward, 1), 6);
        assert_eq!(step_lines(&folds, slice, 10, Direction::Backward, 2), 2);
        assert_eq!(step_lines(&folds, slice, 10, Direction::Backward, 100), 0);
        assert_eq!(step_lines(&folds, slice, 10, Direction::Forward, 100), 20);
        assert_eq!(step_lines(&[], slice, 10, Direction::Forward, 100), 20);
    }

    #[test]
    fn line_distance_matches_step_lines() {
        let text = text(20);
        let slice = text.slice(..);
        let folds = [Fold::new(slice, 2, 5).unwrap(), Fold::new(slice, 6, 9).unwrap()];

        assert_eq!(line_distance(&folds, slice, 0, 2), 2);
        assert_eq!(line_distance(&folds, slice, 2, 6), 1);
        assert_eq!(line_distance(&folds, slice, 6, 2), 1);
        assert_eq!(line_distance(&folds, slice, 2, 10), 2);
        assert_eq!(line_distance(&folds, slice, 0, 12), 6);
        assert_eq!(line_distance(&[], slice, 0, 12), 12);
        for line in [0, 2, 6, 10, 12] {
            for count in 0..8 {
                let target = step_lines(&folds, slice, line, Direction::Forward, count);
                assert_eq!(line_distance(&folds, slice, line, target), count);
            }
        }
    }

    #[test]
    fn lines_for_rows_counts_folded_lines() {
        let text = text(30);
        let slice = text.slice(..);
        let folds = [Fold::new(slice, 1, 9).unwrap(), Fold::new(slice, 12, 20).unwrap()];

        assert_eq!(lines_for_rows(&[], slice, 0, 5), 5);
        // rows show lines 0, 1, 10, 11, 12
        assert_eq!(lines_for_rows(&folds, slice, 0, 5), 13);
        // rows show lines 0, 1, 10, 11, 12, 21
        assert_eq!(lines_for_rows(&folds, slice, 0, 6), 22);
        // a fold that starts after the last row does not count
        assert_eq!(lines_for_rows(&folds, slice, 0, 2), 2);
        // starting below the first fold
        assert_eq!(lines_for_rows(&folds, slice, 10, 4), 12);
    }
}
