# Diff viewing, navigation and session lifecycle: UX audit of `dev-3-way-diff-second-attempt`

Scope: single-pane diff, split diff, changed-file picker, commit, range and buffer diffs. The 3-way merge view is out of scope.

Evidence tags used throughout:

- **[ran]** observed by driving the real `target/debug/hx` in tmux and reading the screen or its ANSI cell colours
- **[read]** verified by reading code at the cited line
- **[test]** behaviour specified by a named integration test in `helix-term/tests/test/diff_merge.rs` (read, not re-run)
- **[commit]** claim from a commit message, not reproduced
- Anything unverified sits under Inferences or Gaps

Citations are relative to the worktree root. `git log master..HEAD` has 55 commits.

Harness: `target/debug/hx` (built 14:48 today, after HEAD `de51c5ef2` at 12:05, tree clean), 130x40 tmux, truecolor, default theme unless stated, `mouse = true`, a throwaway `HOME` so paths take their real form (`~/.cache/helix/git-rev/<16 hex>/<file>`), throwaway git repos with modified, deleted, staged-renamed, binary and untracked files. No repo file was changed.

---

## What is the overall UX verdict, and what mental model must a user hold?

### Takeaway

The rendering core is good and the session machinery is now robust, but the feature does not tell users what they are looking at. Split-diff alignment, hunk jumps and suspend/resume work well, while the statusline shows a cache path instead of the promised `filename @ ref`, nothing on screen ever names the comparison, and the default open is almost indistinguishable from normal editing. The user must carry five layers of invisible state in their head, and most of the serious problems are failures to surface that state rather than logic bugs.

### Cited Findings

**What is good**

- Side-by-side alignment with virtual spacers works for insertions and deletions, and `]g` lands both panes on the same hunk at the top of the viewport. [ran]; `helix-view/src/annotations/diff.rs`, `helix-term/src/commands.rs:4037-4104`
- Leaving and returning to a diff restores it, including split state and cursor. [test] `jumping_back_to_a_split_diff_restores_it`, `ordinary_buffer_navigation_restores_diff`, `returning_both_diff_panes_restores_diff_in_either_order`; `helix-view/src/editor.rs:3411-3515`
- `Space m v` keeps the cursor line across a toggle, and the choice persists for later files. [ran] line 12 preserved through off/on; `helix-term/src/commands/diff.rs:715-822`, `:866`
- Lifecycle is hard to crash: `:bco`, `:bca!`, `C-w v` then `:q`, and `:rla` inside live sessions produced no panic or unexpected exit. [ran]
- Open failure is atomic: revisions are prepared before anything is replaced. `helix-view/src/editor.rs:3051`, `:3167-3168`; [test] `failed_diff_open_preserves_the_current_comparison`, `failed_split_toggle_keeps_the_original_comparison`
- Intraline highlighting has sensible heuristics (similarity thresholds, density caps) that avoid noise on rewrites. `helix-term/src/ui/diff.rs:13-22`, `:517-526`
- The commit-walk loop is pleasant once it is set up: with a log in a scratch buffer, `Space m q` on a single-pane range diff returns to that buffer so the next line can be picked. [ran]
- The picker pre-selects the current file and is fast on a large repo. [ran] for pre-selection; [commit] `a9b975212` for the numbers below

**Severity-ranked problems** (detail in the later sections)

| # | Sev | Problem | Evidence |
|---|-----|---------|----------|
| 1 | Critical | Revision panes show a cache path, not `filename @ ref`. A range diff shows two opaque hashes and no ref anywhere | [ran] |
| 2 | Critical | `HEAD~N` does not work and bad refs fail silently: success status, then an empty or partial picker | [ran] |
| 3 | High | Renames render as an entirely added file; the picker's `> renamed` state is not reached in the tested cases | [ran] |
| 4 | High | Switching panes after scrolling teleports both viewports to a stale cursor | [ran] |
| 5 | High | No next-file command, and in range diffs each step costs O(position) key presses | [ran] |
| 6 | High | Default single-pane open looks like nothing happened; deleted text cannot be seen at all | [ran] |
| 7 | High | Entering split closes all other windows, ending does not restore them, and direct-to-split loses the pane's jumplist and alternate file | [ran] |
| 8 | Medium | Single-pane view paints a spacer tint over real unchanged lines after a net deletion; base-pane spacers get no tint | [ran] |
| 9 | Medium | Read-only panes refuse edits silently, and `:diffget` there reports success | [ran] |
| 10 | Medium | Picker scope and split preference are sticky and invisible; `Loading changes for:` never resolves; no empty state | [ran] |
| 11 | Medium | Snapshot buffers accumulate at two per file while stepping | [ran] |
| 12 | Medium | Wheel over the unfocused pane is inert; `:N` does not sync until the next key | [ran] |
| 13 | Medium | Binary files open as garbage in the diff | [ran] |
| 14 | Medium | Alignment breaks with soft-wrap and for a deletion at the top of a file | [ran] |
| 15 | Low | `C-s`/`C-v` ignored in the changed-file picker, a regression from master | [ran] |
| 16 | Low | Picker order differs between first and later opens | [ran] |

**The state the user must hold** [read]

| Layer | Set by | Lives on | Visible? | Cleared by |
|-------|--------|----------|----------|------------|
| Buffer gutter base | `:diff-base REF` | `doc.diff_base_ref` | Only with the non-default `version-control` statusline element (`helix-term/src/ui/statusline.rs:545`, default list at `helix-view/src/editor.rs:628-651`) | `:diff-base`, `:diff-reset` |
| Picker scope | `:diff-commit`, `Space m c`, `Space m C` | global `diff.range` | One transient status line | `:diff-reset`, `Space m r`; not `Space m q` |
| Comparison session | Picker Enter, `:diff-buffer` | per-view `diff.views` | Not named anywhere | `Space m q`, `:q`, `:bc`, another diff |
| Suspended sessions | Navigating away | global list, max 32 (`helix-view/src/diff_view.rs:47`) | Never | Jumping back, reset, new diff in that view |
| Layout preference | `Space m v` | global `split_view_override` | Never | Nothing: `:diff-reset` does not touch it (only write is `helix-term/src/commands/diff.rs:866`) |

- `:diff-commit` does not show a commit; it only sets picker scope. `Space m c` and `Space m C` differ by case and have different semantics. `:diff-base` does not start a session, so `Space m v` after it says `Not in a diff view`. [ran]
- `DiffViewState.git_ref` holds the comparison descriptor (`a..b`, `buffer 1..2`) but is never rendered: all `.git_ref` reads in `helix-term/src` and `helix-view/src` are on `GitRevision`, not the session (`helix-view/src/editor.rs:2872`, `:2928`, `helix-term/src/commands/typed.rs:597`). [read]

### Inferences

- Most defects in items 1, 2, 6, 9 and 10 are the same defect: session state exists in memory and is not projected onto the screen. A single "what am I comparing" statusline element would address a large share of the confusion, since `display_name()` and `git_ref` already hold the text.
- The five-layer model is too much to hold. The biggest conceptual simplification would be a session that owns its range and layout, rather than global sticky state plus per-view state.

### Gaps

- Keystroke and confusion claims come from a trace by one evaluator, not usability sessions.
- The 59 integration tests were read as a specification and not re-run. The commit message for `de51c5ef2` says they pass. [commit]

---

## What does each journey cost in keystrokes and context switches?

### Takeaway

The first diff is cheap at three presses, but the side-by-side view takes six by default, commit and range review require a separately made git log with no support from the editor, and stepping through a range costs more per file the further down you go. Ending a review completely takes several unrelated actions, and the review does not hand back the layout it took.

### Cited Findings

Modified keys count as one press, and `⏎` is Enter.

| Journey | Sequence | Presses | Context switches |
|---------|----------|---------|------------------|
| (a) Working tree, default | `Space g ⏎` | 3 | 1 overlay |
| (a) Working tree, side-by-side | `Space g ⏎ Space m v` | 6 | 1 overlay, focus lands right |
| (b) One commit, hash known | `:diff-commit HASH! ⏎ Space g ⏎` | about 25 (`:dc` about 16) | 1 prompt, 1 overlay |
| (b) One commit, log in a buffer | `x Space m C Space g ⏎` | 7 | 1 overlay |
| (b) Making that log | `:new ⏎ :insert-output git log --oneline ⏎` | 38 | 2 prompts |
| (c) Two commits from log | `x x x Space m c Space g ⏎` | 9, plus 3 for side-by-side | 1 overlay |
| (c) Range, typed | `:dc A..B ⏎ Space g ⏎` | 10 + ref lengths | 1 prompt, 1 overlay |
| (d) Two buffers | `:dbuf ⏎ ⏎` | 7 | 1 prompt, 1 overlay |
| (e) Next file, working tree | `Space g C-n ⏎` | 4 per file | 1 overlay per file |
| (e) Next file, range | `Space g (C-n × position) ⏎` | 3 + position | 1 overlay per file |
| (e) Workaround | `Space ' C-n ⏎` | 4 per file | 1 overlay per file |
| (f) End session only | `Space m q` | 3 | none |
| (f) Fully back to normal | `Space m q`, then `Space m r` or `:dr ⏎` | 6+ | rebuild layout by hand |

**(a) Working tree** [ran]

1. `Space g` opens the overlay with the file list on the left and the file's top in the preview. No title names the scope; `8/8` is the only count.
2. The status line says `Loading changes for: HEAD` and keeps saying it after the picker closes.
3. `⏎` opens the single-pane diff. The statusline reads ` NOR   main.rs`, identical to normal editing, and no message says a diff opened (`helix-term/src/commands/diff.rs:186-204`).
4. `Space m v` splits with base on the left and working on the right. Focus is on the right, `Split view on` appears.

**(b) One commit** [ran]

- Helix has no git-log view; `:insert-output git log --oneline` in a scratch buffer works. `:sh` output goes to a popup (`helix-term/src/commands/typed.rs:2612`), not a buffer. [read]; [ran] for `:insert-output`
- `x`, `Space m C` says `Diff set: showing changes in 10aff0a` and opens nothing. A second action, `Space g ⏎`, is needed (`helix-term/src/commands/diff.rs:998-1019`).
- The result is a read-only single pane with no view of the commit message, author or base.
- `extract_commit_hash` takes only the first whitespace token, 7 to 40 hex characters (`helix-term/src/commands/diff.rs:929-933`), so default `git log` (`commit <hash>`) and `--graph` (`* <hash>`) lines fail with `Invalid commit hash in selection`. [read]

**(c) Range** [ran]

- `x x x` on three log lines then `Space m c` gives `Diff set: 144908a..10aff0a`. The bottom line is excluded, so three selected lines cover two commits.
- One selected line means commit versus working tree, without that commit's own changes. `Space m C` is the command that shows it. The warning for the `HEAD` case is a thoughtful patch on a surprising default (`helix-term/src/commands/diff.rs:964-968`).
- On merge history the two ends are compared as snapshots, which can include another branch; a code comment defers this (`helix-term/src/commands/diff.rs:977-980`). [read]

**(d) Two buffers** [ran]

- `:dbuf ⏎` shows id, flags and name; `⏎` opens the split at once. The picked buffer is left, the current buffer right, and both stay editable (`main.rs [+]` after typing in the left pane).
- `Diffing buffer 1 against current buffer` uses an id, not a name.
- `:diff-buffer` always forces a split (`helix-term/src/commands/typed_diff.rs:156`, `:196`) and so closes other windows.

**(e) Stepping** [ran]

- No next-file command exists. `]g` at the last hunk is a silent no-op: it returns the range unchanged and neither wraps nor moves on (`helix-term/src/commands.rs:4000-4012`). [read]
- Working tree: pre-selection puts the cursor on the current file, so `C-n ⏎` steps.
- Range diff: pre-selection compares against `doc.path()`, which for a snapshot is the cache path (`helix-term/src/commands/diff.rs:132-135`, `:208-212`), so the cursor always starts on row 0. Observed: from `d.txt` and `e.txt` the cursor was on `c.txt`, and reaching file N needed N-1 `C-n`.
- The same failure occurs in a working-tree diff with focus on the base pane: cursor on `x.txt` while reviewing `y.txt`. [ran]
- `DiffSession.last_changed_file_selection` is written but never read, so the data for fixing this already exists (`helix-term/src/commands/diff.rs:187`; only other hit is its declaration, `helix-view/src/diff_view.rs:18`). [read]
- `Space '` restores the last picker with its cursor, which gives 4 presses per file even in ranges. It is not mentioned in the diff docs. [ran]
- `j` in the picker types into the filter; `C-n`, `Tab` or Down moves. [ran]

**(f) Ending** [ran]

- `Space m q` closes the session and keeps the working buffer, or closes the current snapshots and falls back to the earlier buffer.
- The picker range persists: after `Space m q`, `Space g` still reported `Loading changes for: HEAD~4..HEAD`.
- Windows are not restored (see the lifecycle section).

### Inferences

- For 20 files in a range: 247 presses by `Space g`, 76 by `Space '`, and the former is what the docs imply. A next-file binding would cost about 2 per file.
- Commit review is the weakest journey. It depends on a log the editor cannot produce, and on two commands that could be one, since `Space m C` is almost always followed by `Space g`.

### Gaps

- `Space-g` key counts assume the target file is near the cursor. Filtering by name is a real alternative whose cost depends on naming.
- The cost of `git log` output with `--decorate` or custom formats was not tested beyond the code path.

---

## What visual feedback is given, and can the user always tell which side is which, what is compared, and what is editable?

### Takeaway

Left is always base and right is always working, which is consistent, but only the layout conveys that. The label users are promised (`filename @ ref`) never appears in the statusline, buffer picker or bufferline; a revision pane shows a cache path that also pushes out the `[readonly]` flag and can overwrite the neighbouring pane's mode indicator. Nothing shows what is being compared, and the single-pane view looks like ordinary editing.

### Cited Findings

**Pane identity**

- The base pane statusline shows the cache path: `~/.cache/helix/git-rev/10e8734d013ce0de/main.r`, cut at 65 columns. [ran] Cause: `render_file_name` uses `relative_path()` (`helix-term/src/ui/statusline.rs:445-459`) and ignores `display_name_override` (set at `helix-view/src/document.rs:2009`).
- A range diff has this on both sides: `…/71385960964975b0/main.r` and `…/bc712a6a1229b6cc/main.`, with no ref or commit visible on either. [ran]
- The buffer picker shows the same paths, and at normal widths truncates away the file name: `….cache/helix/git-rev/794ed667562f9aa2/`. [ran]
- The bufferline shows `doc.path().file_name()`, so a revision and its working file have identical labels (`helix-term/src/ui/editor.rs:642-648`). [read]
- The book says these buffers are "named `filename @ ref`" (`book/src/diff-and-merge.md:69`; `book/src/pickers.md`). `display_name()` reaches only `:diff-buffer`'s picker, buffer-close errors, the `%` register and logs (`helix-term/src/commands/typed_diff.rs:79`, `helix-view/src/editor.rs:2167`, `helix-term/src/commands/typed.rs:907`, `helix-view/src/register.rs:61`). [read]
- A deleted file's pane reads `[scratch]  [readonly]`, in the statusline and the buffer list. [ran]

**Read-only indication**

- `[readonly]` is in the default statusline (`helix-view/src/editor.rs:637`) and is fine in a wide single pane: `~/.cache/helix/git-rev/1d99eb83793f4a6b/top.txt  [readonly]`. [ran]
- At 65-column panes the left section overflows, and only `]` or `nly]` is visible. [ran]
- The overflow paints onto the next pane. Depending on render order it overwrites the focused pane's mode label: observed `1:1 nly]    staged.txt` with no `NOR`, `]       main.rs`, ` OR   y.txt`. The ANSI style of the left pane continues through those cells, confirming the cause. [ran]

**What is being compared**

- Nothing persistent shows it. The only indication is a transient status line such as `Loading changes for: 144908a..10aff0a`. [ran]
- The picker has no title and does not show the range. [ran]
- `head@ref` for `:diff-base` is rendered only by the `version-control` element, which is not in the default statusline (`helix-term/src/ui/statusline.rs:535-552`, `helix-view/src/editor.rs:628-651`). [read]

**Single-pane view**

- Statusline identical to normal, gutter identical to the normal git gutter when the base is HEAD. The visible differences are the intraline tint and the tint bug below. [ran]
- Deleted lines cannot be seen. A 10-line removal shows only `61 ▔fn item_70()`. [ran] `DiffAlignment` is split-only (`helix-view/src/view.rs:696`), and nothing renders base text inline.

**Gutter**

- In split, colour follows the side: all of the left pane is `diff.minus.gutter`, all of the right pane `diff.plus.gutter`, and the glyph still shows the shape (`helix-view/src/gutter.rs:110-118`, `:146-153`). [read]; [ran] for the left pane (`#f22c86`).
- This reads well as "old side, new side" but drops the modified colour, so in split only `▍` versus `▔` distinguish hunk kinds.

**Intraline**

- Working side: a soft background with syntax colours kept, `#3b4f6a` on `twenty changed` in the default theme. [ran] `helix-term/src/ui/document.rs:576-640`
- Base side: foreground only, `#f22c86` with the background unchanged. [ran] The contrast-aware branch applies only to `diff.plus`, so the two sides are treated differently. Syntax colour is lost on the left, and removed whitespace has no visible mark. [read for whitespace]
- On by default in sessions (`helix-view/src/document.rs:2030-2038`), and toggled per view with `:toggle-char-diff`.
- Hunks over 40 lines combined, or over 8 KiB, get none (`helix-term/src/ui/diff.rs:13-14`, `:164`), with no indication. [read]

**Spacers**

- Aligned correctly in split. [ran]
- Working pane spacers are tinted `#4a315b`. Base pane spacers stay plain `#3b224c`, so "nothing here" looks different on each side. [ran]
- Cause: `helix-term/src/ui/editor.rs:105-109` uses `doc.diff_handle()`, the buffer's own Git diff, rather than `diff_handle_for_view`. A revision has none. This is the only remaining `.diff_handle()` caller in `helix-term/src` and `helix-view/src`, and `de51c5ef2` converted the others. [read]
- Same line, second symptom: the decoration is added for single-pane sessions too, without the `is_split` check `apply_diff_alignment` has, and it paints rows for virtual lines that do not exist. After a 10-line deletion, the six visible real lines `item_70` to `item_75` carry background `74,49,91`, and the rest of the ten rows fell below the viewport. [ran]

**Other**

- `]g` positions differently by mode: split aligns the hunk to the top with scrolloff context, single-pane leaves it at the bottom scrolloff edge (row 32 of 38). [ran]
- There is no hunk counter. `merge_jump_impl` reports `Conflict 2/5`; diff has no equivalent (`helix-term/src/commands/diff.rs:625-626`). [read]

### Inferences

- From the `read` of the working pane: in a buffer diff or a diff against an older ref, the working pane's spacer tint comes from HEAD's hunks and can land on rows where no spacer exists. This was reasoned from the code path and not captured on screen. With `:diff-commit 144908a` the two hunk sets coincided at the rows checked, so that run neither confirms nor refutes it.
- The overflow threshold is roughly a 75 to 80 column pane, or about 160 terminal columns, derived from path length plus indicator width. Common terminals are below that, so the mode label loss is not an edge case.
- Red and green are the only side cues in the gutter, so colour-blind users rely on layout and glyph shape.

### Gaps

- Bufferline behaviour is read, not run.
- Which view renders last depends on view id slot order, so whether the mode label is overwritten varies by how the session was opened: toggling kept `NOR`, opening direct to split lost it in the tested runs. The exact rule was not traced.

---

## How good is navigation, and which movements do not sync?

### Takeaway

Hunk jumps are the strongest part of the feature and keyboard scrolling syncs well, but sync copies viewports and not cursors, so the two panes hold different positions and the difference bites when focus moves. Sync is a list of wired call sites rather than a post-render invariant, which is why gaps keep appearing.

### Cited Findings

| Movement | Result | Evidence |
|----------|--------|----------|
| `j/k`, `C-d/C-u`, any key in the editor | Viewport syncs after each key | [ran] `helix-term/src/ui/editor.rs:1518` |
| `/`, `n`, `N`, prompt abort | Syncs | [test] `split_diff_keyboard_and_search_sync_scroll`; `helix-term/src/commands.rs:2245`, `helix-term/src/ui/mod.rs:117-166` |
| Wheel over the focused pane | Syncs | [ran] both panes top at line 19 after 6 ticks |
| `]g`/`[g` | Moves both cursors and viewports, even with sync off | [ran] `180`/`176` with sync disabled; `helix-term/src/commands.rs:4037-4104` |
| Paste | Syncs | [read] `helix-term/src/ui/editor.rs:1415` |
| Wheel over the **unfocused** pane | **Inert** | [ran] 6 ticks, top stayed at 19 |
| Switching focus after scrolling | **Both viewports jump to the partner's stale cursor** | [ran] |
| `:150` goto line | **No sync until the next key** | [ran] 19 vs 118; `Esc` then gave 128/118 |
| Cursor in the partner pane | **Never follows, except `]g`** | [ran] left `1:1`, right `24:1` |
| Soft-wrapped lines | **Rows drift** | [ran] |

- **Pane switch**: `C-d` ×4 in the right pane put both tops at 82/72. `C-w h` moved both to 16/16, around the left cursor at line 21. `C-w l` then gave 45/45, not the original place. [ran] Cause: `Editor::focus` calls `ensure_cursor_in_view(view_id)` for the target (`helix-view/src/editor.rs:2323`), which scrolls to its stale cursor, then `sync_scroll_to_linked_views` pushes that onto the other pane (`:2372`). A comment in `resume_diff_view` acknowledges this very risk for the restored pane (`helix-view/src/editor.rs:3495-3498`).
- **Unfocused wheel**: the handler scrolls the hovered pane, restores focus, then `ensure_cursor_in_view(current_view)` syncs from the focused pane, overwriting what the wheel did (`helix-term/src/ui/editor.rs:1263-1272`). The unfocused cursor still moved, 1:1 to 27:1. [ran] The book says the wheel keeps viewports aligned (`book/src/diff-and-merge.md:53`), which is true only in this degenerate sense.
- **`:N`**: the goto-line path calls the view-level `ensure_cursor_in_view` and bypasses the editor-level one that syncs (`helix-term/src/commands/typed.rs:2160-2161`). The key-path hook does not run because the prompt owns the key. [read]
- **Cursor on toggle**: preserved and mapped between sides, including from the base pane to the working side (`helix-term/src/commands/diff.rs:715-822`). [ran] for the working side, [read] for the base side
- **Alignment at file top**: a pure deletion at line 0 puts spacers after the first line because `emit_after = start.saturating_sub(1)` (`helix-view/src/annotations/diff.rs:42`). Observed: right `1 ▔top line 3` on the same row as left `1 ▍top line 0`, while left `top line 3` sits on row 4. Everything below re-aligns. [ran]
- **Soft-wrap**: alignment counts document lines only. A line wrapping to six rows left the panes 5 rows apart for the rest of the file: left `16` beside right `11`. [ran]
- **Replacements**: spacers go before the hunk, so in a 5-to-2 replacement the first old lines face blanks instead of the new lines. [read] `helix-view/src/annotations/diff.rs:36-43`
- **`]g` semantics change inside a diff**: it collapses to a point (`helix-term/src/commands.rs:4015-4019`), so it no longer selects the hunk, and this also applies in select mode. [read]
- **Hunk pairing**: uses the mirrored hunk index, with a mapped-line fallback (`helix-term/src/commands.rs:4076-4081`). [read]

### Inferences

- From the `:N` result, async jumps that finish outside the key path should behave the same way: LSP goto-definition within a file, symbol picker, global search. All would stay unsynced until the next key. Not run; the LSP in the harness had no project.
- The fix for pane-switch teleport is either to move the partner cursor with the viewport or to make focus pull the cursor to the viewport. Copying only offsets leaves the bug class open.

### Gaps

- Mouse click and drag across panes, terminal resize, and horizontal scrolling were not exercised.
- Long-line horizontal offset sync is read only (`helix-view/src/editor.rs:2717-2721`).

---

## What happens across the session lifecycle, and which UX failure classes kept recurring?

### Takeaway

Crashes, accidental quits and wrong-buffer panes were the repeated failure classes, and those are now likely solved, by a good choke point in `replace_document_in_view` and per-view diff handles. The classes tied to feedback (scroll sync, silent failure, read-only behaviour, snapshot naming) were fixed at the symptom and still have verified siblings.

### Cited Findings

**Lifecycle behaviour**

| Trigger | Result | Evidence |
|---------|--------|----------|
| `:o`, `gd`, `Space f`, `Space b`, `ga`, `gn` from the working pane | Suspends; base pane closes; returning restores with cursor and scroll | [test] `ordinary_buffer_navigation_restores_diff`; `helix-view/src/editor.rs:1968-1976` |
| Same, from the base pane | Working pane stays as a plain view; diff returns when both panes show their buffers | [test] `returning_both_diff_panes_restores_diff_in_either_order` |
| `C-o`/`C-i` | Same, via `show_jump` | `helix-term/src/commands.rs:5627-5650` |
| `Space g` to another file | **Detaches and forgets**: no restore on `C-o` back | [test] `a_diff_opened_or_closed_since_is_not_brought_back`; `helix-view/src/editor.rs:3078-3082` |
| `:q` in working pane | Base closes, working stays; a second `:q` quits and checks unsaved buffers | [test] `quit_in_split_diff_working_pane_keeps_unsaved_buffer`; `helix-view/src/editor.rs:2145-2152` |
| `C-w o` | Ends session and keeps a view | [test] `window_only_in_split_diff_base_pane_keeps_a_view` |
| `C-w v` in a pane | New pane is not a session member and uses the buffer's own diff; session continues; closing the working pane still ends it | [ran] three panes, no panic; [read] `helix-view/src/editor.rs:1995-2016` |
| `:bc` on either side | Ends the session, including a never-shown single-pane base | [test] `closing_the_base_revision_ends_the_split_diff`, `closing_the_working_file_ends_a_single_pane_diff`; `helix-view/src/editor.rs:2173-2186` |
| `:bco`, `:bca!` inside a range split | Clean, ends on `[scratch]` | [ran] |
| `:rl`, `:rla` | Base stays; base pane reload is a no-op | [ran]; [test] `reloading_a_diffed_file_keeps_the_diff_s_base`; `helix-view/src/document.rs:1282` |
| HEAD or branch moves | **Open session keeps the old snapshot with no signal.** `:rl` does not change it. Toggling or reopening reads the new commit | [ran] `12`/`twelve` persisted after an external commit; after toggling `twelve`/`twelve`, new hash |
| Open failure | Current diff preserved | [test] `failed_diff_open_preserves_the_current_comparison`, `failed_diff_open_keeps_a_suspended_comparison` |
| Deleted file | Empty read-only pathless pane, `:w` cannot recreate it | [ran]; [test] `changed_file_picker_opens_deleted_file_read_only`; `helix-view/src/editor.rs:3052-3071` |
| Permission read-only file | Editable, fails at `:w`, as upstream | `helix-view/src/document.rs:1633-1638`; [commit] `9ec8f6369` |

**Window and history damage**

- `close_other_views_for_diff` closes every non-focused view whenever a split opens (`helix-view/src/editor.rs:3010-3028`, called at `:3087`, `:3187`, `:3279`). A three-pane layout collapsed to two on `Space m v`, then to one on `Space m q`. No warning, no restore. [ran]
- With `split-view` on or the sticky override set, every open from the picker does this. `:diff-buffer` always does.
- Opening directly into split puts the base in the original view and the working file in a new `View` (`helix-view/src/editor.rs:1998-2003`, `:3090-3093`). Closing removes the original view (`:3592-3597`), taking its jumplist and alternate file with it. After `Space m q`, `ga` said `no last accessed buffer` and `C-o` did nothing. [ran]
- The toggle path copies `docs_access_history` only (`helix-term/src/commands/diff.rs:867-879`), so `ga` works there [ran], but the jumplist is not copied. [read]

**Snapshot accumulation**

- Stepping uses `detach_diff_view`, which keeps revision buffers (`helix-view/src/editor.rs:3517-3532`), so 4 files in a range left 8 snapshots plus the original: `9` in `Space b`. [ran]
- `gn` from there walks into those buffers or out of the diff. [ran]
- `Space m q` closes only the current pair (`helix-view/src/editor.rs:3577-3586`), and `:diff-reset` closes none (`helix-term/src/commands/diff.rs:899-926`). [read]
- Each snapshot launches language servers (`helix-view/src/editor.rs:2949-2957`) and writes a cache file; rust-analyzer's workspace error reappeared per new snapshot. [ran]
- Cache files are removed only by `close_document` (`helix-view/src/editor.rs:2233-2239`). 13 directories remained under `~/.cache/helix/git-rev` after the sessions exited. [ran]

**Failure classes from `git log master..HEAD --grep=fix`**

| Class | Commits | Verdict |
|-------|---------|---------|
| Teardown panic, empty tree, accidental quit | `a02d16393`, `f153b900e`, `9988007b4`, part of `e9d5662da` | **Likely solved.** Adversarial pass found nothing. `close` handles teardown explicitly (`helix-view/src/editor.rs:2139-2152`) |
| Diff state stored on the shared document | `e9d5662da`, `4157585d1`, `12dd2f23e`, `de51c5ef2` | **Structurally solved**, with per-view handles. **One verified miss**: the spacer decoration at `helix-term/src/ui/editor.rs:106` |
| Pane shows a buffer it does not own | `7031fbe0f`, `182cae713`, `98c8b07dd` | **Mostly solved.** `replace_document_in_view` is a good choke point. `show_jump` still writes `view.doc` directly, guarded by a prior suspend (`helix-term/src/commands.rs:5627-5650`) |
| Scroll sync coverage | `171aa8ab2`, `9c70bf5bc` | **Not solved.** Three verified siblings: `:N`, unfocused wheel, pane switch |
| Silent empty picker | `c6461ffe3`, `d2386b6b3` | **Symptom only.** `c6461ffe3` fixed one trigger of "the scan error left an empty picker". The `let _ =` remains at `helix-term/src/commands/diff.rs:39`, `:49`, `:247`, `:265`. Verified siblings: unknown ref, `HEAD~N` |
| Read-only semantics | `9ec8f6369`, `88be85087`, `83cb6933e`, `e9d5662da` | **Safety solved, feedback not.** Silent refusal; false `:diffget` success |
| Snapshot identity and lifetime | `c722a0045`, `4a34e637a`, `98884514d` | **Identity solved; naming and cleanup not** |
| `Space m c` meaning | `554e8faaf`, then `e1be05feb`; also `c6461ffe3` | **Stable now, still surprising.** One line reversed direction twice |
| Picker scale | `7f40cfc79`, `a9b975212`, `49513ead3`, `27664ee4a` | **Solved for speed**, paid for with rename UX |

### Inferences

- The recurring pattern: bugs in logic got structural fixes, and bugs in feedback got local ones. The next round of reports is more likely to be about the same sync and silent-failure classes than about crashes.
- A stale session after a commit is defensible for a snapshot view, but only if the screen says what commit it is, which returns to the labelling problem.
- `Space g` from one diff to another not being restorable on `C-o` is deliberate, but it makes the common stepping path the one place where "jumping back restores the diff" is false. That is a hole in a rule users will otherwise learn.

### Gaps

- DAP frame display in a diff pane is covered by a code comment only (`helix-view/src/editor.rs:1877-1880`) and was not exercised.
- Multi-repo sessions and `:cd` during a session were not tested.
- Auto-reload and focus-gained reload paths are absent on this branch per `182cae713`'s note, so they are not evaluated.

---

## Can the user act on what they see while reviewing?

### Takeaway

Review is nearly read-only in practice. Editing the working pane works and updates the diff live, and path and permalink copying is well done, but there is no staging, no bound revert-hunk key, no way to carry a change across panes, and no jump from a revision pane to the real file at the same line.

### Cited Findings

| Action | Status | Evidence |
|--------|--------|----------|
| Edit working pane, gutters update live | Works | [test] `git_split_diff_stays_in_sync_after_edits`, `scratch_buffers_can_be_diffed_and_update_live` |
| Edit either side of a buffer diff | Works | [ran] `[+]` on the left pane |
| Edit a revision pane | **Silently refused** | [ran] |
| `:w` in a revision pane | Clear error: `Error saving: Cannot save a git revision document` | [ran]; `helix-view/src/document.rs:1013` |
| Revert hunk in working pane | `:reset-diff-change`, `:diffget`, `:diffg`; view-aware, so it reverts to the compared base | [read] `helix-term/src/commands/typed.rs:2640` |
| Revert hunk, key | **None**, and not in `book/src/diff-and-merge.md` | [read] listed only in `book/src/generated/typable-cmd.md:90` |
| Stage file or hunk | **None**. `git add` exists only in `merge_finish` | [read] `helix-term/src/commands/diff.rs:665-669` |
| Copy path | `:copy-path` gives the real path, even from a revision | [ran] |
| Copy URL | `:copy-path url` links the pane's commit at the cursor line, with forge detection; on a working pane it falls back to the path | [read] `helix-term/src/commands/typed.rs:638-690`, `book/src/editor.md` |
| Copy a hunk across panes | Manual `y`, `C-w`, `p` | [read] no command exists |
| Jump to real file from a revision | **No direct command**; type `:o path`, losing the line | [read] |
| Commit metadata | **None shown** | [ran] |

- **Refusal is silent**: `i` in the base pane enters ` INS `, and typed text vanishes with no message; `x d` does nothing. `apply_inner` returns `false` (`helix-view/src/document.rs:1636-1638`), and insert mode has no read-only check. [ran]
- **False success**: `]g` then `:diffget` in the base pane reports `Reset 1 change`, and the pane is unchanged. `doc.apply`'s result is ignored and the status is unconditional (`helix-term/src/commands/typed.rs:2669-2675`). [ran]
- `Space-m` lists merge and diff commands together regardless of context (`helix-term/src/keymap/default.rs` `"m"` block). [read]

### Inferences

- By the same code path, `:diffget` in either pane of a range diff also reports a success it did not perform. Not run on a range diff.
- Revert is in the hands of those who already know vim's `:diffget`. Its docs are absent from the page users would read, which makes a working feature look missing.
- `ig` works in a diff (`helix-term/src/commands.rs` textobject path uses `diff_handle_for_view`), so `mig` plus yank is a usable hunk-copy idiom, untested.

### Gaps

- `:copy-path url` was not run against a real remote; `helix-vcs/src/web_url.rs` has tests on its own.
- LSP features in snapshots are claimed by the book but were not usefully testable without a real project.

---

## How does performance feel: picker streaming, cache staleness, large files and repos?

### Takeaway

The picker is engineered well for scale on the evidence available, and the cost was moved onto rename handling. What the user feels as a problem is less speed than the absence of any finished or empty state, plus unguarded binary files. Per-frame intraline work has no cache, which is worth watching but was not measured.

### Cited Findings

- [commit] `a9b975212`, on a 19k-file repo with about 4k untracked files on a network filesystem: first listed file 2.1s to 41ms, full scan 4.3s to 2.0s. Not reproduced here.
- Tracked changes stream first, then untracked from a parallel walker (`helix-term/src/commands/diff.rs:242-274`, `helix-vcs/src/git.rs:433-495`). Reopens show a cache instantly and reconcile only if different, preserving query and selection (`helix-term/src/commands/diff.rs:298-305`). [read]
- **Order changes between opens.** First open is discovery order (`new_name.txt, old_name.txt, staged.txt, gone.txt, main.rs…`); second is sorted by rank then path (`blob.bin, main.rs, new_name.txt…`). [ran] Only the cached listing goes through `sort_changes`; streamed rows arrive as discovered (`helix-term/src/commands/diff.rs:252-254`, `:275`).
- **Never-ending loading message.** `Loading changes for: HEAD` stays after the scan and after the picker closes. [ran] There is no completion state and no file count in the status.
- **No empty state.** `HEAD..HEAD` shows `0/0` with an empty list and `Loading changes for: HEAD..HEAD`, which reads as still loading. [ran]
- **Silent scan failure is a regression.** Master reported scan errors via `status::report_blocking(err)` (the block removed from `helix-term/src/commands.rs`); this branch discards them with `let _ =`. [read]
- **Cache key omits cwd**: only `base_ref` and `target_ref` are compared (`helix-term/src/commands/diff.rs:107`), so after `:cd` to another repo the old list can flash. [read] The struct's own doc accepts a brief stale flash (`helix-view/src/diff_view.rs:19-23`).
- **Preview does not show the diff.** The right panel is the plain working-tree file from the top, even for range entries (`helix-term/src/commands/diff.rs:206`). [ran] for the top-of-file view, [read] for the working-tree source
- **Opening a diff is synchronous**: commit resolution and blob read run on the main thread (`helix-view/src/editor.rs:2888-2914`). [read]
- **Binary files**: `blob.bin` opened as garbage on both sides, with mismatched detected encodings (`windows-1252`, `UTF-16LE`). [ran] `from_git_revision` decodes unconditionally (`helix-view/src/document.rs:1991`), while the picker's preview has its own binary and 10 MiB guards (`helix-term/src/ui/picker.rs:61`, `:89-90`). No guard exists in the diff path.
- **Intraline runs in the render path** with no memo (`helix-term/src/ui/editor.rs:157-164`), walks all hunks each frame, and uses a 4 ms timeout per text diff (`helix-term/src/ui/diff.rs:22`, `:145-195`). It is capped by hunk size. [read]
- **Limits**: line diff stops at `64 * u16::MAX` lines or that times 128 bytes (`helix-vcs/src/diff.rs:122-124`). [read]
- The status scan compares HEAD tree to index as well, so staged changes are listed (`d2386b6b3`). [ran]

### Inferences

- Worst-case intraline cost: `best_line_match` is O(old × new) similarity calls, each allowed 4 ms, for up to a 40-line hunk. A pathological 20-by-20 replacement could reach hundreds of milliseconds per frame with several such hunks on screen. This is an upper bound from reading, not a measurement; typical lines complete far faster than the timeout.
- Disabling rename detection was reasonable for the listing but need not affect opening. Resolving a rename for the one chosen file would be cheap, since the deleted partner is already in the list.

### Gaps

- No measurements on a real large repo or large file. The debug binary is not representative, so none were attempted.
- Editing latency with several comparisons of one buffer, each with its own worker, was not measured.

---

## What happens with renames, refs and other inputs the feature must recognise?

### Takeaway

These two inputs produce the most misleading results in the feature. Renames look like whole-file additions, and the most common ref spelling in git is rejected or silently yields an empty picker while the status line reports success.

### Cited Findings

**Renames**

- `git mv` plus a one-line edit shows two rows, `~ modified new_name.txt` and `- deleted old_name.txt`. Opening the first shows an empty base and 40 added lines for a 1-line change. [ran]
- The same happens in a range: `~ modified moved_a.txt`, whole file added. [ran]
- `book/src/pickers.md` says the picker lists renamed files, and the `> renamed` label exists (`helix-term/src/commands/diff.rs:150`), but `status` turns rewrites off (`helix-vcs/src/git.rs:697-707`), and in tree diffs `Change::Rewrite` is reported as `Modified` (`helix-vcs/src/git.rs` `for_each_change_impl`). [read]
- The picker passes `meta.path()` for both sides (`helix-term/src/commands/diff.rs:197-199`), and `FileChange::Renamed`'s `from_path` is not used at open (`helix-vcs/src/status.rs:22-30`). [read]
- `a9b975212` documents the trade-off; the commit text calls it "an untracked + deleted pair". [commit]

**Refs**

- `resolve_commit` accepts a ref name, a hex prefix, or trailing `^`. It does not accept `~` or general revspecs (`helix-vcs/src/git.rs:570-612`). [read]
- `:diff-base HEAD~1` fails with `'HEAD~1' is not a valid reference or commit hash`, while the command's own doc says "branch, tag, SHA, or HEAD~N" (`helix-term/src/commands/typed.rs:3477`). `HEAD^^^^` works. [ran]
- `:diff-commit HEAD~4..HEAD` is accepted, and `Space g` shows `0/0` with no error. [ran]
- `:diff-commit nosuchref` reports `Diff range set to: nosuchref — use space+g to browse files`. `Space g` then shows only untracked files, `1/1`, which reads as "no tracked changes". The tracked scan failed and the untracked walk does not depend on it. [ran] `:diff-files nosuch` behaves the same. [ran]
- `DiffRange::parse` validates shape only (`helix-view/src/editor.rs:1207-1233`). [read]
- `:diff-commit` and `:diff-files` have no ref completion (`CommandCompleter::none()`, `helix-term/src/commands/typed.rs:3956`, `:3978`). [read]
- `:diff-files` is described as "between current branch and specified commit" but compares with the working tree, including untracked files (`helix-term/src/commands/typed_diff.rs:308-312`; book `diff-and-merge.md:40-41`). It also ignores and does not set the picker range. [read]

### Inferences

- `HEAD~N` is likely the first thing many users type. Combined with success-styled status text, this could read as "the feature is broken" on first contact.
- Replacing the hand-rolled resolver with gix's revision parsing and validating at `:diff-commit` time would fix this class at once, including the failure to report. `Space m c` already calls `resolve_commit_id` for its `HEAD` check, so validation has a ready hook (`helix-term/src/commands/diff.rs:961-963`).

### Gaps

- `@{u}`, `main@{1}`, tags pointing at non-commits and ambiguous short hashes were not tested.
- Whether `FileChange::Renamed` can be reached by any current path was not exhaustively traced; it was not reached in the tested rename scenarios.

---

## How well are themes supported, and are the new keys documented with fallbacks?

### Takeaway

The default and mainstream RGB themes look right. Support weakens at the edges: 9 themes with no background get a hard-coded dark green tint, 6 themes get no intraline highlighting at all, and themes that define `diff.plus` with their own background have it overridden. `ui.diff.spacer` is defined in no theme and documented nowhere, and the split-view config key is undocumented too.

### Cited Findings

Counts come from resolving every theme's `inherits` chain across 195 themes (193 in `runtime/themes` plus `default` and `base16_default`). [read]

| Key | Themes defining it | Effect when missing |
|-----|-------------------|---------------------|
| `ui.diff.spacer` | 0 | Fallback from `ui.background` |
| `diff.plus` (exact) | 189 | Intraline silently absent |
| `diff.minus` (exact) | 191 | Intraline absent on base side |
| `diff.plus.gutter`, `diff.minus.gutter` | 18 each | Falls back to `diff.plus`, `diff.minus` |
| `ui.background.bg` | 186 | Fixed colours below |
| `diff.delta.moved` | 24 | Style for a label not reached in testing |

- Intraline uses `find_highlight_exact` (`helix-term/src/ui/diff.rs:140`, `helix-view/src/theme.rs:466`), so unlike the gutter's `theme.get` there is no prefix fallback. Themes missing `diff.plus`: `ao`, `yo`, `yo_berry`, `yo_light`, `dark-synthwave`, `iroaseta`. On `ao` the changed token has no highlight. [ran]

Measured cells [ran]:

| Theme | Intraline bg (right) | Right spacer | Left spacer |
|-------|---------------------|--------------|-------------|
| `default` | `#3b4f6a` | `#4a315b` | plain |
| `catppuccin_latte` | `#dae7dc` | `#e0e2e6` | plain |
| `base16_transparent` | `#1e321e` | ANSI 100 | plain |
| `term16_light` | `#1e321e` | ANSI 100 | plain |
| `term16_dark` | `#1e321e` | not measured with trailing cells | plain |
| `ao` | none | not measured with trailing cells | plain |

- Tint derivation: capped per-channel addition on dark themes and an eighth mix on light ones (`helix-term/src/ui/document.rs:587-630`). The result is close to green on some themes and not on others: `default` gets a slate blue because it adds to a purple background. [ran]
- When there is no RGB `ui.background`, the fallback is a fixed `Rgb(30,50,30)` (`helix-term/src/ui/document.rs:632-634`), truecolor even in 16-colour themes. [ran]
- The spacer fallback is ±15 per channel on RGB backgrounds, else `Color::Gray` (`helix-term/src/ui/diff.rs:52-74`).
- 14 themes define `diff.plus` with their own `bg`, among them `modus_vivendi`, `modus_operandi`, `zenburn`. That bg is replaced and the fg dropped (`helix-term/src/ui/document.rs:637-640`). [read] These are accessibility-minded themes that chose those colours deliberately.
- Syntax highlighting of `.diff` and `.patch` files is unaffected: the override is in the overlay path only. `+new line` keeps `#35bf86` with no background. [ran] A suspected regression was checked and ruled out.
- Docs: `book/src/themes.md:266-271` documents `diff.plus` and friends but not `ui.diff.spacer`, nor their intraline use. [read]
- `[editor.diff] split-view` is undocumented: no match for `split-view` in `book/src`, although the struct exists (`helix-view/src/editor.rs:515-526`). [read] Users cannot discover that the default can change.
- `book/src/editor.md` gained `[editor.git-remote]` on this branch, so the omission is an oversight rather than a policy. [read]

### Inferences

- On `term16_light` and `base16_transparent` over a light terminal, dark green `#1e321e` behind dark text will likely be unreadable. The colour is verified; the contrast depends on the terminal palette.
- `#1e321e` on 16-colour setups relies on a truecolor escape in a theme chosen because truecolor may be absent.
- Honouring a theme's explicit `bg` on `diff.plus`, and adding a `diff.plus.intraline` style key with this tint as the fallback, would respect theme authors without changing the look elsewhere. A matching `diff.minus` treatment would fix the two-sides asymmetry.

### Gaps

- Themes were sampled, not viewed as images: colours are cell-level from ANSI capture, so perceived contrast is judged only by the values.
- Only 10 of 195 themes were run: `default`, `catppuccin_latte`, `gruvbox`, `gruvbox_light`, `base16_transparent`, `base16_default`, `term16_dark`, `term16_light`, `ao`, `modus_vivendi`. `term16_dark` and `ao` right spacers were captured before trailing cells were preserved, so they are marked not measured.
