# Integration Test Abort Investigation

Date: 2026-04-22

## Summary

The full `helix-term` integration test binary aborts under the default parallel test runner:

```bash
cargo test -p helix-term --features integration --test integration
```

This is not a normal test assertion failure. The process aborts after background worker panics.

The failure is not caused by `test::commands::surround_delete` itself. That test was simply the last one printed before the abort surfaced in one run.

## What Passed

Targeted new integration test passes:

```bash
cargo test -p helix-term --features integration --test integration statusline_completion_renders_without_popup
```

Existing unit test passes:

```bash
cargo test -p helix-term --lib statusline_completion_respects_display_mode_and_editor_mode
```

Full integration binary passes when forced single-threaded:

```bash
cargo test -p helix-term --features integration --test integration -- --test-threads=1
```

Observed result from the single-threaded run:

- `130 passed`

## Failure Signature

In the default parallel run, the first relevant panic was:

- `assertion failed: self.len_after == other.len`

at:

- [helix-core/src/transaction.rs](/home/kuba/SRC/helix/helix-core/src/transaction.rs:164)

That panic is reached from debounced word-index change merging:

- [helix-view/src/handlers/word_index.rs](/home/kuba/SRC/helix/helix-view/src/handlers/word_index.rs:85)

Specifically:

- `mem::take(&mut pending_change.changes).compose(change.changes);`

Later in the same failing run, a worker thread also panicked with:

- `there is no reactor running, must be called from the context of a Tokio 1.x runtime`

at:

- [helix-event/src/runtime.rs](/home/kuba/SRC/helix/helix-event/src/runtime.rs:68)

The reported crashing thread was:

- `nucleo worker 0`

After that, Rayon aborted the whole process:

- `Rayon: detected unexpected panic; aborting`

## Current Interpretation

- The abort appears to be a parallel integration-test issue involving background workers.
- The word-index debounce path is part of the stack, so word-completion activity is relevant.
- The new statusline integration test may make the issue easier to hit because it exercises manual word completion.
- The problem does not currently look like a failure in the new statusline assertions themselves.
- The issue may involve cross-test interaction, shared background state, or worker activity outliving an individual test's runtime context.

## Most Useful Repro Commands

Fails intermittently or consistently under default parallelism:

```bash
cargo test -p helix-term --features integration --test integration
```

Useful for noisy backtraces:

```bash
RUST_BACKTRACE=1 cargo test -p helix-term --features integration --test integration -- --nocapture
```

Reliable workaround:

```bash
cargo test -p helix-term --features integration --test integration -- --test-threads=1
```

## Suggested Next Steps

1. Isolate the smallest subset of integration tests that reproduces the abort under parallel execution.
2. Inspect assumptions in [helix-view/src/handlers/word_index.rs](/home/kuba/SRC/helix/helix-view/src/handlers/word_index.rs:78) around composing multiple pending `DocumentDidChange` changes.
3. Check whether background workers from one integration test can outlive the Tokio runtime context for that test.
4. Inspect why `nucleo` workers can call `request_redraw` after the runtime context is gone.
5. Decide whether the integration harness should serialize these tests by default, or whether the worker/runtime lifecycle bug should be fixed directly.
