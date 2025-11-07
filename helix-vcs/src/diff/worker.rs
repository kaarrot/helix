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

        // Compute character-level hunks for modified lines
        diff.char_hunks.clear();
        // Clone hunks to avoid borrow checker issues
        let hunks_clone = diff.hunks.clone();
        for hunk in &hunks_clone {
            if let Some(char_hunks) = Self::compute_char_hunks_for_hunk(hunk, &diff_base, &doc) {
                // Store character hunks for each affected line in the "after" version
                for line in hunk.after.start..hunk.after.end {
                    if let Some(hunks_for_line) = Self::get_char_hunks_for_line(
                        &char_hunks,
                        line,
                        hunk,
                        &diff_base,
                        &doc,
                    ) {
                        diff.char_hunks.insert(line, hunks_for_line);
                    }
                }
            }
        }

        drop(diff);
        self.diff_finished_notify.notify_waiters();
    }

    /// Compute character-level hunks for a single line-level hunk.
    /// Returns None if the hunk is a pure insertion/removal or too large.
    fn compute_char_hunks_for_hunk(
        hunk: &imara_diff::Hunk,
        diff_base: &Rope,
        doc: &Rope,
    ) -> Option<Vec<imara_diff::Hunk>> {
        let len_before = hunk.before.len();
        let len_after = hunk.after.len();

        // Skip pure insertions/removals and very large changes (same heuristic as helix-core)
        if len_before == 0
            || len_after == 0
            || len_after > 5 * len_before
            || 5 * len_after < len_before && len_before > 10
            || len_before + len_after > 200
        {
            return None;
        }

        // Extract text from both versions
        let before_start = diff_base.line_to_char(hunk.before.start as usize);
        let before_end = diff_base.line_to_char(hunk.before.end as usize);
        let before_text: Vec<u8> = diff_base
            .slice(before_start..before_end)
            .to_string()
            .into_bytes();

        let after_start = doc.line_to_char(hunk.after.start as usize);
        let after_end = doc.line_to_char(hunk.after.end as usize);
        let after_text: Vec<u8> = doc.slice(after_start..after_end).to_string().into_bytes();

        // Compute character-level diff using Myers algorithm
        let input = InternedInput::new(&before_text[..], &after_text[..]);
        let mut char_diff = imara_diff::Diff::default();
        char_diff.compute_with(
            Algorithm::Myers,
            &input.before,
            &input.after,
            input.interner.num_tokens(),
        );

        Some(char_diff.hunks().collect())
    }

    /// Extract character hunks for a specific line from the full character hunks.
    fn get_char_hunks_for_line(
        char_hunks: &[imara_diff::Hunk],
        line: u32,
        line_hunk: &imara_diff::Hunk,
        _diff_base: &Rope,
        doc: &Rope,
    ) -> Option<Vec<CharHunk>> {
        // Calculate the character offset of this line relative to the hunk start
        let line_start_in_doc = doc.line_to_char(line as usize);
        let hunk_start_in_doc = doc.line_to_char(line_hunk.after.start as usize);
        let line_end_in_doc = doc.line_to_char((line + 1) as usize);

        let line_start_offset = line_start_in_doc - hunk_start_in_doc;
        let line_end_offset = line_end_in_doc - hunk_start_in_doc;

        let mut result = Vec::new();

        for char_hunk in char_hunks {
            let char_start = char_hunk.after.start as usize;
            let char_end = char_hunk.after.end as usize;

            // Check if this character hunk intersects with this line
            if char_end > line_start_offset && char_start < line_end_offset {
                // Adjust the character hunk to be relative to the line start
                let adjusted_start = char_start.saturating_sub(line_start_offset);
                let adjusted_end = (char_end - line_start_offset).min(line_end_offset - line_start_offset);

                let kind = if char_hunk.before.is_empty() {
                    CharHunkKind::Insert
                } else if char_hunk.after.is_empty() {
                    CharHunkKind::Delete
                } else {
                    CharHunkKind::Modify
                };

                result.push(CharHunk {
                    after: adjusted_start..adjusted_end,
                    kind,
                });
            }
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
