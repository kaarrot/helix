# Python debug console for Helix

## Context

Today the eval path is a one-shot single-line prompt. `<space>G p` opens
`Prompt`, evaluates one expression in the current frame, prints a truncated
result to the status line, and closes. Everything about it fights the actual
workflow of debugging Python:

- **One line only.** `Prompt` stores `line: String` and renders it on a single
  surface row (`ui/prompt.rs:31, 509-512`). `key!(Enter)` unconditionally means
  "submit" (`:683`); no key path inserts a `\n`. A `for` loop or a `def` cannot
  be typed.
- **It freezes the editor.** Every eval goes through `helix_lsp::block_on`
  (`commands/dap.rs:957`) with a hardcoded 20 s timeout in `Client::call`
  (`client.rs:281-287`). You cannot move around the source, open the variables
  picker, or copy a name while an expression runs.
- **Output is thrown away.** `print()` inside the debuggee — and inside an eval,
  since pydevd redirects stdout during a `repl` evaluate — arrives as DAP
  `Output` events. Helix's handler does `set_status(...)`
  (`helix-view/src/handlers/dap.rs:291-306`), so each line overwrites the
  previous one and nothing accumulates.
- **No transcript.** The full result is smuggled into `Editor::dap_eval_result`
  and the `+` register; the status line shows at most 2048 chars
  (`commands/dap.rs:979`). You cannot look back at what you ran.
- **Tracebacks are unreadable.** `transport.rs:186-199` discards the adapter's
  `message` field on failure and returns
  `Error::Other(format_err!("{:?}", res.body))`, so a Python exception surfaces
  as Debug-formatted JSON. pydevd puts the real formatted traceback in *both*
  `body.result` and `message` (`pydevd_comm.py:1242-1270`).

The good news is that the hard protocol work is already done on this branch.
`Client::eval` sends `context: "repl"` (`helix-dap/src/client.rs:581-590`),
which is the one context for which pydevd calls `exec` instead of `eval`. Read
`pydevd_vars.evaluate_expression` (`is_exec=True`): it **auto-dedents indented
source, compiles multi-line blocks, supports top-level `await`, and writes
results back into the real frame** via `update_globals_and_locals`. Multi-line
Python that mutates the stopped frame already works over the wire — Helix just
has no way to type it, to see what came back, or to do anything else while it
runs.

**Goal:** a real interactive Python console — a persistent pane where you write
normal, multi-line Python, press Enter, and see output and tracebacks
accumulate, with the frame updated underneath you — and where **the editor stays
live while an expression is running**, so you can navigate source, browse
variables, and paste names into the console.

## Approach

Back the console with a **real Helix `Document`** in a split, not a bespoke
`Component`.

This is a deliberate call. A custom `Component` would mean reimplementing
multi-line editing, cursor movement, selection, undo, search and syntax
highlighting from scratch (~300+ LOC of editor before a single line of Python
runs), and it would fight `EditorView::render`, which computes
`editor_area = area.clip_bottom(1)` and calls `cx.editor.resize(editor_area)`
(`ui/editor.rs:1578-1584`) — a compositor layer draws *over* the editor rather
than docking beside it. A document also makes the non-blocking requirement
natural: it is just another buffer, so while an eval is in flight you keep
normal focus switching, motions, yank and paste with no special "modal" state.

One fact makes Enter-to-submit almost free: **`ret` is bound only in insert
mode** (`keymap/default.rs:396`, `insert_newline`). Normal-mode `<ret>` is
unbound. So:

- **Insert mode Enter → newline.** Multi-line Python, naturally.
- **Normal mode Enter → submit.** A new static command that no-ops unless the
  console is focused, so binding it globally is harmless.

The buffer *is* the transcript, comint-style: input is echoed with `>>> `/`... `
markers, output and results append below, and the region after the last marker
is the live input. Scrollback is ordinary text — searchable, yankable, and
re-editable to resubmit a previous block.

## Implementation

### Phase 0 — Async foundation

*File: `helix-dap/src/client.rs`, `helix-term/src/commands/dap.rs`*

Everything else depends on this, so it goes first. Two things block async today:

1. **`Client::eval` borrows `&self`.** `Client::call` (`client.rs:257-296`) is
   already the right shape — it clones `server_tx` up front and returns a
   `'static` future that does **not** borrow the client, which is why
   `dap_callback` works for `terminate` and `set_exception_breakpoints`. But
   `request` is an `async fn(&self, …)` (`:299`) and `eval` /
   `eval_with_context` / `scopes` / `variables` are built on it, so their
   futures borrow and cannot be handed to `Jobs`.

2. **A chained call cannot be built inside the async block.** `eval_complete`
   needs a *conditional second request* (the elided-value retry), and
   `collect_variable_items` needs `scopes` followed by N `variables` calls.
   Issuing a second request needs `server_tx` **and** a fresh seq from
   `request_counter`, which is a bare `AtomicU64` (`client.rs:33`) and so cannot
   be moved into a `'static` future.

Fix both with a small cloneable handle:

- Change `request_counter: AtomicU64` → `Arc<AtomicU64>`.
- Add `pub struct Requester { server_tx: UnboundedSender<Payload>, request_counter: Arc<AtomicU64> }`
  with `call::<R>(args) -> impl Future<Output = Result<Value>> + Send + 'static`
  and a typed `request::<R>()` on top of it. Move the body of `Client::call`
  into it; `Client::call` becomes `self.requester().call::<R>(args)`, so every
  existing caller is unchanged.
- Add `Client::requester(&self) -> Requester` (cheap: one channel clone + one
  `Arc` clone).

Then add an error-preserving callback helper next to `dap_callback`
(`commands/dap.rs:232`). The existing one drops failures: on `Err` the future
returns early, `Jobs::add` calls `helix_event::status::report(err)`
(`job.rs:127`) and it lands on the status line — but a console needs the
traceback *in the transcript*.

```rust
fn dap_callback_result<T, F>(jobs: &mut Jobs, call: ..., callback: F)
where F: FnOnce(&mut Editor, &mut Compositor, dap::Result<T>) + Send + 'static
```

It awaits the call, deserializes, and moves the whole `Result` into the
`Callback::EditorCompositor` closure, so the future itself always returns `Ok`.

Also make the request timeout configurable rather than a hardcoded
`Duration::from_secs(20)` (`client.rs:287`) — see the runaway-eval note below.

### Phase 1 — Console buffer and output capture

*Files: `helix-view/src/editor.rs`, `helix-view/src/handlers/dap.rs`,
`helix-term/src/commands/dap.rs`, `helix-term/src/commands.rs`,
`helix-term/src/commands/typed.rs`, `helix-term/src/keymap/default.rs`*

1. Add `pub dap_console: Option<DocumentId>` to `Editor` (next to
   `dap_eval_result`, `editor.rs:1194` — same precedent for hanging DAP session
   state off `Editor`; initialize at `:1333`).

2. Add `Editor::dap_console_append(&mut self, text: &str)` in
   `helix-view/src/handlers/dap.rs`:
   - Resolve `dap_console` to a `DocumentId`; return if unset.
   - Find a `ViewId` showing it via `self.tree.views()`; fall back to any key of
     `doc.selections()` (`document.rs:1966`) so appends still work when the
     console is not visible. **`Document::selection(view_id)` panics on an
     unregistered view** (`document.rs:1961`), so this fallback is required.
   - Append with `Transaction::change(doc.text(), [(len, len, Some(text))])`,
     `doc.apply(&t, view_id)`.
   - Call `doc.reset_modified()` (`document.rs:1752`) so the console never
     blocks `:q`.
   - Only move the selection to the end **if the cursor was already at the end**,
     so streaming output does not yank the cursor away while you are editing an
     input block or reading scrollback.

   Model on `Editor::new_file_from_stdin` (`editor.rs:1855-1873`), which is the
   existing "create a doc and push text into it" template.

3. Route `Event::Output` into it. Replace the `set_status` in the
   `Event::Output` arm (`handlers/dap.rs:291-306`) with a
   `dap_console_append` call. Keep dropping `telemetry`; prefix or style
   `stderr`. Fall back to `set_status` when no console exists, so behaviour is
   unchanged for anyone who never opens one. The handler already has
   `&mut Editor` and already returns `true` to request a redraw, so streaming
   output needs no extra plumbing.

4. `dap_console` static command in `commands/dap.rs`:
   - If `dap_console` is set and a view shows it, `editor.focus(view_id)`.
   - Otherwise `editor.new_file(Action::HorizontalSplit)` (`editor.rs:1848`),
     then `doc.set_language_by_language_id("python", &loader)`
     (`document.rs:1315`) for tree-sitter highlighting, seed a `>>> ` marker,
     `reset_modified()`, and store the id.
   - Register in the `static_commands!` list (`commands.rs:585-602`, beside the
     other `dap_*` entries) and add a `:debug-console` `TypableCommand`
     (`typed.rs:2766` list) alongside the existing `debug-eval` (`typed.rs:3449`).
   - Bind under the sticky debug menu in `keymap/default.rs:239-261`, e.g.
     `"c" => dap_console`.

### Phase 2 — The non-blocking submit loop

*File: `helix-term/src/commands/dap.rs`*

5. `dap_console_submit` static command:
   - Return silently unless the focused document is `editor.dap_console` — this
     is what makes a global `ret` binding safe.
   - Take the text after the last prompt marker as the input block. Trim the
     trailing newline; if it is empty, just emit a fresh marker.
   - Resolve the frame the same way `dap_evaluate` does
     (`commands/dap.rs:1019-1027`): `debugger.stack_frames[&thread_id][frame].id`.
   - Grab a `Requester`, then **return immediately** — no `block_on`.

   Send the block **verbatim, newlines and indentation included** — pydevd
   dedents based on the first non-empty line (`pydevd_vars.py:211-230`), so a
   block copied out of the middle of a function body runs as-is.

6. Rewrite `eval_complete` (`commands/dap.rs:956`) as one `'static` async block
   over a `Requester`, preserving its current logic exactly:
   - `evaluate` in the `repl` context;
   - if the result contains `...` and either `quirks.full_value_expression` is
     set or the adapter supports the `clipboard` context, issue the retry and
     `unquote` it (`full_value_expression` `:879`, `unquote` `:900`).

   Both the quirk template (`repr({})` for Python, `languages.toml:1070`) and
   the capability bool are captured **by value** before the block is spawned, so
   nothing borrows the client. Deliver the outcome through
   `dap_callback_result`.

7. **Placeholders and ordering.** Several evals can be in flight, and results
   can arrive out of order or interleaved with `Output` events. On submit,
   append a unique sentinel line (`⋯ running #N`) after the echoed input; when
   the result arrives, find that exact sentinel in the buffer and replace it.
   Search for the sentinel text rather than storing byte offsets — the buffer is
   user-editable and offsets go stale. If it is gone (the user deleted it),
   append at the end instead. Keep the counter and any per-eval state on
   `Editor` beside `dap_console`.

8. Push the block into the `=` history register via the existing
   `save_eval_history` (`commands/dap.rs:858`) so console and prompt share
   history and it still persists to `~/.cache/helix/dap-eval-history`.

9. **Drop the `assignment_target` special case for console submits.** That
   heuristic (`commands/dap.rs:914`) exists only because the status line had
   nothing to show after an exec. In the console, pydevd already echoes the
   value of anything that compiles as an eval through an `Output` event
   (`pydevd_vars.py`, `is_exec=True` branch), so the transcript fills in on its
   own — and dropping it removes a second round trip. Leave the function in
   place for `dap_evaluate`, which still needs it.

10. Bind `"ret" => dap_console_submit` in **normal** mode
    (`keymap/default.rs:8`). Insert-mode `ret` stays `insert_newline` (`:396`).

### Phase 3 — Real Python tracebacks

*Files: `helix-dap/src/transport.rs`, `helix-dap/src/lib.rs`*

11. Add an `Error::Adapter { command: String, message: Option<String>, body: Option<Value> }`
    variant to `helix-dap/src/lib.rs:13`, and return it from the failure branch
    at `transport.rs:196-199` instead of `Error::Other(format_err!("{:?}", body))`.
    Its `Display` should prefer `message`, falling back to `body.result`, then
    the raw body.

    pydevd's `_evaluate_response_return_exception` puts the formatted traceback —
    with pydevd's own frames stripped — into both fields
    (`pydevd_comm.py:1242-1270`), so this turns a JSON blob into an actual
    Python traceback. Print it into the transcript on error.

    This touches every DAP call site that formats an error, but they all just
    `format!("{}", e)`, so the change is additive.

### Phase 4 — Tab completion

*Files: `helix-dap/src/types.rs`, `helix-dap/src/client.rs`,
`helix-term/src/commands/dap.rs`*

12. Add the `completions` request — it is the one REPL-relevant DAP request with
    **no type at all**, even though `supports_completions_request` and
    `completion_trigger_characters` are already deserialized
    (`types.rs:76, :128`). Add `CompletionsArguments { frame_id, text, column, line }`,
    `CompletionsResponse { targets: Vec<CompletionItem> }` and
    `CompletionItem { label, text, sort_text, ty, start, length, .. }` to
    `mod requests` (`types.rs:368+`), plus `Client::completions(...)` and
    `supports_completions()` beside the existing capability helpers
    (`client.rs:612-627`).

    **`column` is 1-based** — debugpy does `column = arguments.column - 1`
    (`pydevd_process_net_command_json.py:312`).

13. `dap_console_complete` static command: send the current input block and
    cursor column asynchronously, then show the targets in a `Menu`/`Picker`
    overlay that inserts the chosen `text`/`label` at the reported
    `start`/`length`. Bind to a key in the debug menu.

    Prefer this over the LSP: debugpy's completions are **frame-aware** (jedi
    against live locals), so they know `items` is a list *right now*.

### Phase 5 — Variables: non-blocking, expandable, yankable

*File: `helix-term/src/commands/dap.rs`*

This phase is what makes "select variables and paste their names into the
console" work *while an eval is running*.

14. Make the variables picker async. `collect_variable_items` (`:1153`) blocks
    on `scopes` (`:1192`) and then on `variables` per scope (`:1203`). This
    matters more than it looks: pydevd services requests on the suspended
    thread, so while a `repl` eval is executing Python, a blocking `scopes` call
    **queues behind it** and freezes the editor for up to the full timeout.
    Move the whole fetch into one async block over a `Requester` (scopes, then
    the per-scope `variables` calls) and open the picker from the callback.
    Do the same for `set_expression` / `set_variable` (`:1298`, `:1303`).

15. Add a **yank-name** action to the variables picker so a name reaches the
    console without retyping: bind a key that writes the item's `evaluate_name`
    (already on `VariableItem`, `:1077`) to a register — and, when the console
    is open, offers to insert it at the console's input point directly.

16. Make results expandable. `eval_complete` currently returns
    `dap::Result<String>` and throws away `EvaluateResponse::variables_reference`
    and `ty` (`:956-975`). Return the response instead, and when
    `variables_reference != 0`, let the console open the result in the variables
    picker.

17. Make the picker recursive — this is the `// TODO: allow expanding variables
    into sub-fields` at `:1200`. `collect_variable_items` already discards each
    child's `variables_reference`; keep it on `VariableItem` (which already
    carries `container_reference`) and add a "descend" action that re-runs
    `variables(reference)` and reopens through the existing
    `reopen_variables_picker` (`:1319`). Without this, `dict`/`list`/`self`
    debugging stays flat.

## Runaway evaluations

Async fixes the editor freeze, not the debuggee. If you submit `while True:`,
the editor stays fully responsive but that Python thread is wedged, the request
eventually hits the timeout, and every later DAP request queues behind it in
pydevd — so the session becomes unusable even though the UI is not.

Mitigations, in order of cost:

- Print the timeout into the transcript as a distinct message ("no response
  after Ns — the debuggee may still be running it") rather than a generic error,
  so the state is legible.
- Make the timeout configurable (Phase 0) and give console evals a longer one
  than the 20 s that suits ordinary requests — a deliberate long computation is
  a normal thing to run in a REPL.
- DAP's `cancel` request exists and is unimplemented here; pydevd's support for
  cancelling an in-flight evaluate is limited, so treat "terminate and restart
  the session" as the real escape hatch. Not worth building until it hurts.

## Effort

| Phase | Rough LOC | Days |
|---|---|---|
| 0 — async foundation (`Requester`, result-preserving callback) | ~180 | 1.5 |
| 1 — console buffer + output capture | ~350 | 2–3 |
| 2 — non-blocking submit loop + placeholders | ~300 | 2.5 |
| 3 — real tracebacks | ~80 | 0.5 |
| 4 — tab completion | ~200 | 1–1.5 |
| 5 — async/expandable/yankable variables | ~300 | 2 |
| docs + tests | ~150 | 1 |
| **total** | **~1560** | **~11–13** |

Phases 0+1+2 (~6-7 days) give a working non-blocking console. 3, 4 and 5 are
independent after that and can land in any order.

## Verification

Build and run (see `project_helix_build_grammars` — this repo needs the flag and
a clean config):

```sh
HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo build
cargo test --workspace
cargo xtask docgen          # required: new static + typable commands
```

Unit tests, next to the existing ones in `commands/dap.rs:1525`
(`substitutes_numbered_params`, `finds_the_target_of_an_assignment`, …):

- input-region extraction from a transcript containing several `>>> ` markers
- a multi-line block with blank lines and trailing whitespace round-trips
- `dap_console_submit` is a no-op when the focused doc is not the console
- placeholder replacement: sentinel found, sentinel deleted by the user, two
  sentinels resolved out of order

End-to-end against real debugpy (installed: 1.6.3, `python3` 3.11):

```python
# scratchpad/console_demo.py
import debugpy, time
debugpy.listen(("127.0.0.1", 5678))
debugpy.wait_for_client()
items = [1, 2, 3]
count = 0
print("running")
```

1. `hx --config <clean> console_demo.py`, breakpoint after `count = 0`,
   `<space>G l` → **Attach to port** → `5678`.
2. `<space>G c` opens the console.
3. Type a multi-line block in insert mode, `Esc`, `<ret>`:
   ```python
   for i in items:
       print(i * 2)
   ```
   → transcript shows `2`, `4`, `6` from `Output` events. **This is the case
   that is impossible today.**
4. **The non-blocking check.** Submit `import time; time.sleep(15)`. While it
   runs: switch to the source split, move around, open `<space>G v`, yank a
   variable name, paste it into the console, and start typing the next block.
   Nothing blocks; the `⋯ running` placeholder resolves in place when the sleep
   returns, without disturbing the cursor.
5. Submit two evals with different sleeps back to back and confirm each result
   lands at its own placeholder, in the right order.
6. `count = 41` then `count += 1` then `count` → `42`; resume and confirm the
   program sees 42. Verifies the frame is really being written.
7. `1/0` → a formatted Python traceback in the transcript, not JSON (Phase 3).
8. `items.` + complete → frame-aware members (Phase 4).
9. `{"a": [1,2], "b": 3}` → expandable, drill in via the picker (Phase 5).
10. `:q` with a populated console → closes without a "modified buffer" prompt.

Update `book/src/debugger.md` — the "Inspecting and changing state" section
(line 66) and the "debugpy behavior worth knowing" section (line 137), which
already documents the `repl` context and the `full-value-expression` quirk.
