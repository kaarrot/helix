//! Commands that fold and unfold blocks of lines in the current view.

use helix_core::{fold, Range};
use helix_view::{Document, ViewId};

use super::Context;

/// Folds the innermost foldable block around the primary cursor, or unfolds the
/// fold whose header the cursor is on.
pub fn toggle_fold(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    let text = doc.text().slice(..);
    let line = text.char_to_line(doc.selection(view.id).primary().cursor(text));
    let mut folds = doc.folds(view.id).to_vec();

    if let Some(idx) = folds
        .iter()
        .position(|fold| fold.header_line(text) == line)
    {
        folds.remove(idx);
        doc.set_folds(view.id, folds);
        return;
    }

    let block = doc
        .foldable_blocks()
        .into_iter()
        .filter(|block| block.first_line <= line && line <= block.last_line)
        .min_by_key(|block| block.last_line - block.first_line);
    let Some(new_fold) = block.and_then(|block| block.fold(text)) else {
        cx.editor.set_status("Nothing to fold here");
        return;
    };
    fold::insert(&mut folds, new_fold);
    doc.set_folds(view.id, folds);
    move_selection_out_of_folds(doc, view.id);
}

/// Folds the lines that each selection spans.
pub fn fold_selection(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    let text = doc.text().slice(..);
    let mut folds = doc.folds(view.id).to_vec();
    let mut folded = false;
    for range in doc.selection(view.id) {
        let (first, last) = range.line_range(text);
        if let Some(new_fold) = fold::Fold::new(text, first, last) {
            fold::insert(&mut folds, new_fold);
            folded = true;
        }
    }
    if !folded {
        cx.editor.set_status("Select more than one line to fold");
        return;
    }
    doc.set_folds(view.id, folds);
    move_selection_out_of_folds(doc, view.id);
}

/// Folds every block that the language's `folds.scm` query finds.
pub fn fold_all(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    let text = doc.text().slice(..);
    let mut folds = doc.folds(view.id).to_vec();
    let mut folded = false;
    for block in doc.foldable_blocks() {
        if let Some(new_fold) = block.fold(text) {
            fold::insert(&mut folds, new_fold);
            folded = true;
        }
    }
    if !folded {
        cx.editor.set_status("Nothing to fold");
        return;
    }
    doc.set_folds(view.id, folds);
    move_selection_out_of_folds(doc, view.id);
}

pub fn unfold_all(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    doc.set_folds(view.id, Vec::new());
}

/// Moves every cursor that a fold hides to the start of the fold's header line.
/// Otherwise the next redraw would open the fold right back up.
fn move_selection_out_of_folds(doc: &mut Document, view_id: ViewId) {
    let text = doc.text().slice(..);
    let folds = doc.folds(view_id);
    let selection = doc.selection(view_id).clone().transform(|range| {
        match fold::fold_at(folds, range.cursor(text)) {
            Some(fold) => Range::point(text.line_to_char(fold.header_line(text))),
            None => range,
        }
    });
    doc.set_selection(view_id, selection);
}
