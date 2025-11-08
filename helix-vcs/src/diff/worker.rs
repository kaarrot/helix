use std::sync::Arc;

use helix_core::{Rope, RopeSlice};
use imara_diff::{Algorithm, IndentHeuristic, IndentLevel, InternedInput};
use parking_lot::RwLock;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::Notify;
use tokio::time::{timeout, timeout_at, Duration};

use crate::diff::{
    CharHunk, CharHunkKind, DiffInner, Event, RenderLock, ALGORITHM, DIFF_DEBOUNCE_TIME_ASYNC,
    DIFF_DEBOUNCE_TIME_SYNC,
};

use super::line_cache::InternedRopeLines;

#[cfg(test)]
mod test;

pub(super) struct DiffWorker {
    pub channel: UnboundedReceiver<Event>,
    pub diff: Arc<RwLock<DiffInner>>,
    pub diff_finished_notify: Arc<Notify>,
    pub diff_alloc: imara_diff::Diff,
}

impl DiffWorker {
    async fn accumulate_events(&mut self, event: Event) -> (Option<Rope>, Option<Rope>) {
        let mut accumulator = EventAccumulator::new();
        accumulator.handle_event(event).await;
        accumulator
            .accumulate_debounced_events(&mut self.channel, self.diff_finished_notify.clone())
            .await;
        (accumulator.doc, accumulator.diff_base)
    }

    pub async fn run(mut self, diff_base: Rope, doc: Rope) {
        let mut interner = InternedRopeLines::new(diff_base, doc);
        if let Some(lines) = interner.interned_lines() {
            self.perform_diff(lines);
        }
        self.apply_hunks(interner.diff_base(), interner.doc());
        while let Some(event) = self.channel.recv().await {
            let (doc, diff_base) = self.accumulate_events(event).await;

            let process_accumulated_events = || {
                if let Some(new_base) = diff_base {
                    interner.update_diff_base(new_base, doc)
                } else {
                    interner.update_doc(doc.unwrap())
                }

                if let Some(lines) = interner.interned_lines() {
                    self.perform_diff(lines)
                }
            };

            // Calculating diffs is computationally expensive and should
            // not run inside an async function to avoid blocking other futures.
            // Note: tokio::task::block_in_place does not work during tests
            #[cfg(test)]
            process_accumulated_events();
            #[cfg(not(test))]
            tokio::task::block_in_place(process_accumulated_events);

            self.apply_hunks(interner.diff_base(), interner.doc());
        }
    }

    /// update the hunks (used by the gutter) by replacing it with `self.new_hunks`.
    /// `self.new_hunks` is always empty after this function runs.
    /// To improve performance this function tries to reuse the allocation of the old diff previously stored in `self.line_diffs`
    fn apply_hunks(&mut self, diff_base: Rope, doc: Rope) {
        let mut diff = self.diff.write();
        diff.diff_base = diff_base.clone();
        diff.doc = doc.clone();
        diff.hunks.clear();
        diff.hunks.extend(self.diff_alloc.hunks());

        // Compute character-level hunks for modified lines (per-line basis)
        diff.char_hunks.clear();
        // Clone hunks to avoid borrow checker issues
        let hunks_clone = diff.hunks.clone();
        for hunk in &hunks_clone {
            // Skip pure insertions or removals - they don't have line-to-line correspondence
            if hunk.before.is_empty() || hunk.after.is_empty() {
                continue;
            }

            // For modifications, compare lines one-by-one
            // Take the minimum to handle cases where line counts differ
            let lines_to_compare = hunk.before.len().min(hunk.after.len());

            for i in 0..lines_to_compare {
                let before_line_idx = hunk.before.start + i as u32;
                let after_line_idx = hunk.after.start + i as u32;

                if let Some(char_hunks) = Self::compute_char_hunks_for_line(
                    before_line_idx,
                    after_line_idx,
                    &diff_base,
                    &doc,
                ) {
                    diff.char_hunks.insert(after_line_idx, char_hunks);
                }
            }
        }

        drop(diff);
        self.diff_finished_notify.notify_waiters();
    }

    /// Compute character-level hunks for a single line by comparing before and after versions.
    /// Returns None if the lines are identical or too different to meaningfully compare.
    fn compute_char_hunks_for_line(
        before_line_idx: u32,
        after_line_idx: u32,
        diff_base: &Rope,
        doc: &Rope,
    ) -> Option<Vec<CharHunk>> {
        // Extract the text for both lines
        let before_start = diff_base.line_to_char(before_line_idx as usize);
        let before_end = diff_base.line_to_char((before_line_idx + 1) as usize);
        let before_text: Vec<u8> = diff_base
            .slice(before_start..before_end)
            .to_string()
            .into_bytes();

        let after_start = doc.line_to_char(after_line_idx as usize);
        let after_end = doc.line_to_char((after_line_idx + 1) as usize);
        let after_text: Vec<u8> = doc.slice(after_start..after_end).to_string().into_bytes();

        // Skip if lines are identical
        if before_text == after_text {
            return None;
        }

        // Skip very large lines (performance)
        if before_text.len() + after_text.len() > 2000 {
            return None;
        }

        // Compute character-level diff using Myers algorithm
        let input = InternedInput::new(&before_text[..], &after_text[..]);
        let mut char_diff = imara_diff::Diff::default();
        char_diff.compute_with(
            Algorithm::Myers,
            &input.before,
            &input.after,
            input.interner.num_tokens(),
        );

        // Convert byte-level hunks to character hunks
        let mut result = Vec::new();
        for hunk in char_diff.hunks() {
            let kind = if hunk.before.is_empty() {
                CharHunkKind::Insert
            } else if hunk.after.is_empty() {
                CharHunkKind::Delete
            } else {
                CharHunkKind::Modify
            };

            result.push(CharHunk {
                after: hunk.after.start as usize..hunk.after.end as usize,
                kind,
            });
        }

        if result.is_empty() {
            None
        } else {
            Some(result)
        }
    }

    fn perform_diff(&mut self, input: &InternedInput<RopeSlice>) {
        self.diff_alloc.compute_with(
            ALGORITHM,
            &input.before,
            &input.after,
            input.interner.num_tokens(),
        );
        self.diff_alloc.postprocess_with(
            &input.before,
            &input.after,
            IndentHeuristic::new(|token| {
                IndentLevel::for_ascii_line(input.interner[token].bytes(), 4)
            }),
        );
    }
}

struct EventAccumulator {
    diff_base: Option<Rope>,
    doc: Option<Rope>,
    render_lock: Option<RenderLock>,
}

impl EventAccumulator {
    fn new() -> EventAccumulator {
        EventAccumulator {
            diff_base: None,
            doc: None,
            render_lock: None,
        }
    }

    async fn handle_event(&mut self, event: Event) {
        let dst = if event.is_base {
            &mut self.diff_base
        } else {
            &mut self.doc
        };

        *dst = Some(event.text);

        // always prefer the most synchronous requested render mode
        if let Some(render_lock) = event.render_lock {
            match &mut self.render_lock {
                Some(RenderLock { timeout, .. }) => {
                    // A timeout of `None` means that the render should
                    // always wait for the diff to complete (so no timeout)
                    // remove the existing timeout, otherwise keep the previous timeout
                    // because it will be shorter then the current timeout
                    if render_lock.timeout.is_none() {
                        timeout.take();
                    }
                }
                None => self.render_lock = Some(render_lock),
            }
        }
    }

    async fn accumulate_debounced_events(
        &mut self,
        channel: &mut UnboundedReceiver<Event>,
        diff_finished_notify: Arc<Notify>,
    ) {
        let async_debounce = Duration::from_millis(DIFF_DEBOUNCE_TIME_ASYNC);
        let sync_debounce = Duration::from_millis(DIFF_DEBOUNCE_TIME_SYNC);
        loop {
            // if we are not blocking rendering use a much longer timeout
            let debounce = if self.render_lock.is_none() {
                async_debounce
            } else {
                sync_debounce
            };

            if let Ok(Some(event)) = timeout(debounce, channel.recv()).await {
                self.handle_event(event).await;
            } else {
                break;
            }
        }

        // setup task to trigger the rendering
        match self.render_lock.take() {
            // diff is performed outside of the rendering loop
            // request a redraw after the diff is done
            None => {
                tokio::spawn(async move {
                    diff_finished_notify.notified().await;
                    helix_event::request_redraw();
                });
            }
            // diff is performed inside the rendering loop
            // block redraw until the diff is done or the timeout is expired
            Some(RenderLock {
                lock,
                timeout: Some(timeout),
            }) => {
                tokio::spawn(async move {
                    let res = {
                        // Acquire a lock on the redraw handle.
                        // The lock will block the rendering from occurring while held.
                        // The rendering waits for the diff if it doesn't time out
                        timeout_at(timeout, diff_finished_notify.notified()).await
                    };
                    // we either reached the timeout or the diff is finished, release the render lock
                    drop(lock);
                    if res.is_ok() {
                        // Diff finished in time we are done.
                        return;
                    }
                    // Diff failed to complete in time log the event
                    // and wait until the diff occurs to trigger an async redraw
                    log::info!("Diff computation timed out, update of diffs might appear delayed");
                    diff_finished_notify.notified().await;
                    helix_event::request_redraw()
                });
            }
            // a blocking diff is performed inside the rendering loop
            // block redraw until the diff is done
            Some(RenderLock {
                lock,
                timeout: None,
            }) => {
                tokio::spawn(async move {
                    diff_finished_notify.notified().await;
                    // diff is done release the lock
                    drop(lock)
                });
            }
        };
    }
}
