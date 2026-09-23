use helix_event::register_hook;

use crate::{
    events::{DocumentDidChange, DocumentDidOpen},
    handlers::Handlers,
    review::remap_anchors,
};

pub(super) fn register_hooks(_handlers: &Handlers) {
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        // The first file opened in a worktree brings back that branch's saved
        // conversation, so it is on screen before anyone comments.
        event.editor.peek_reviews(event.doc);
        // Threads reloaded from disk carry only the line they were saved on.
        // Anchoring them as their document opens is what makes them track edits
        // again instead of staying frozen there.
        if !event.editor.diff.reviews.is_empty() {
            event.editor.seed_review_anchors(event.doc);
        }
        Ok(())
    });

    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        // Ghost transactions are previews that are rolled back, so moving
        // anchors for them would leave the threads offset once it is undone.
        if !event.ghost_transaction && !event.doc.review_anchors.is_empty() {
            remap_anchors(&mut event.doc.review_anchors, event.changes);
        }
        Ok(())
    });
}
