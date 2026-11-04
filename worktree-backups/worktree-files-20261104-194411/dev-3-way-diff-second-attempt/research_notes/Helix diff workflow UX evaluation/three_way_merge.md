# 3-way merge workflow: UX audit of `dev-3-way-diff-second-attempt` (HEAD de51c5ef2, base `master`)

## What is the overall UX verdict, and what does "buffer as source of truth" mean for the user?

### Takeaway
The core loop is sound and hard to lose work in: conflicts are easy to reach, accept works from any pane, undo is one step, and quitting cannot discard RESULT. But what shipped is a marker editor with two static reference panes, not a true 3-way merge tool. It has no BASE pane, no on-screen pane labels, side panes that do not follow the conflict, no lasting progress display and no way to finish the git operation. `Space-m f` can also stage leftover conflict markers, contrary to the docs. "Buffer as source of truth" gives excellent resilience and hand-edit freedom, at the cost of any memory of what was resolved or how.

### Cited Findings

#### Evidence legend
- `[CODE]` means verified by reading the source. Links are repo-relative.
- `[RUN x]` means observed by driving the real `hx` TUI through tmux against scratch git repos with a clean `HOME` and an empty config.
- Scripts are in `/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/`. The driver is `drv.py` and the fixtures are `mk1.sh` to `mk5.sh`.
- Probe map: A, B, C = `t4.py`. D, Db = `t5.py`. E, Eb, F = `t6.py`. G, H, I = `t7.py`. J = `t7.py`, `t8.py`. L = `t9.py`. K, Kc, P = `t10.py`. Q, R = `t11.py`. W, N, N2, O0 = `t12.py`. M, O1-O4, N3, Y, S, U = `t13.py`. T = `t14.py`. Z = `t14.py`, `t15.py`. AA, AB, AC = `t15.py`. BL, IF, HG, PF, EOF, ES, PM, NEST = `t16.py`, where ES0-ES4 print as D0-D4 and NEST prints as N1-N5. SP, JM, LY, MS = `t17.py`. UF = `t18.py`. `RUN t1` and `RUN t2` refer to those scripts as a whole.
- Binary used: `target/debug/hx`. `hx --version` prints the stale hash `e1be05fe`, but `cargo build` reported nothing to rebuild and its mtime (14:48) is later than the last source edit (12:01). It therefore matches HEAD source.
- Git is 2.34.1, so zdiff3 could not be produced for a live run.

#### Ranked judgement
Severity: S1 = integrity risk or defeats the feature's purpose. S2 = major friction or a dead end. S3 = moderate. S4 = minor.

| ID | Sev | Judgement | Key evidence |
|---|---|---|---|
| B1 | S1 | Side panes never follow the conflict. After `Space-m n` to line 124, OURS and THEIRS still show line 1. On any file longer than a screen they show unrelated code. | [RUN t2]; `sync_scroll` set at [editor.rs:3711](helix-view/src/editor.rs#L3711), never read; [editor.rs:2764](helix-view/src/editor.rs#L2764) consults `diff.views` only |
| B2 | S1 | No pane label is visible. Both side status lines show a cache path. `b.txt @ HEAD` exists only in a field nothing visible renders. | [RUN t2, O0, BL]; [statusline.rs:450](helix-term/src/ui/statusline.rs#L450) |
| B3 | S1 | `Space-m f` stages files that still contain markers whenever the `<<<<<<<` / `>>>>>>>` pair is broken. The docs say it cannot. | [RUN F2, G3, R4, R5]; [merge_view.rs:43](helix-view/src/merge_view.rs#L43); [diff-and-merge.md:91](book/src/diff-and-merge.md#L91) |
| B4 | S1 | There is no BASE. "3-way" shows two sides and the result. The ancestor is neither fetched nor shown unless the user's git config writes it inline. | [git.rs:236](helix-vcs/src/git.rs#L236); [commands/diff.rs:502](helix-term/src/commands/diff.rs#L502) |
| B5 | S2 | No finish flow: no commit or continue, no operation awareness, no next-file step, and the session stays open after finishing. | Only git spawn is [commands/diff.rs:665](helix-term/src/commands/diff.rs#L665); [RUN Q4, Eb1] |
| B6 | S2 | Conflicts without markers are dead ends. Delete/modify, rename and binary rows read `x conflict`, but Enter gives an error and opens nothing. `Space-m f` meanwhile silently resolves them. | [RUN P, K2] |
| B7 | S2 | Stale buffer false negative. If the file was open before the merge, the picker says `x conflict` and Enter says "No conflict markers found in file". | [RUN M2-M5] |
| B8 | S2 | Progress is ephemeral. "Conflict i/N" appears only on a jump, in the message line. Nothing shows which side was taken. | [commands/diff.rs:625](helix-term/src/commands/diff.rs#L625); [RUN AC] |
| B9 | S3 | RESULT has no conflict rendering beyond the ordinary gutter bar. Markers are plain text, with no region tint. | [RUN AB]; no `conflict` in [ui/editor.rs](helix-term/src/ui/editor.rs) or [ui/diff.rs](helix-term/src/ui/diff.rs) |
| B10 | S3 | Every action is a 3-key chord, with no auto-advance and no landing on the first conflict. A 3-conflict file costs 27 keys. | [default.rs:239](helix-term/src/keymap/default.rs#L239); see the journey section |
| B11 | S3 | Finishing from a side pane leaves `[+]` stuck on RESULT, and `:q` then refuses to quit although disk and index are correct. | [RUN E2-E7] |
| B12 | S3 | `:qa!` and `:wqa` need two invocations from a merge view. `:wq` and `:x` write but do not exit. | [RUN N, N3]; [editor.rs:2149](helix-view/src/editor.rs#L2149) |
| B13 | S3 | Merge view destroys the user's split layout and does not restore it. | [editor.rs:3682](helix-view/src/editor.rs#L3682); [RUN LY3-LY4] |
| B14 | S3 | Switch-side silently expires in several ordinary cases, giving only "No conflict at cursor". | [RUN A9, I7, ES3, JM2] |
| B15 | S3 | Lines resembling markers inside content are misparsed. `=======` blocks the accept safely. `>>>>>>>` or `<<<<<<<` drops lines and leaves a real marker that then stages. | [RUN R3, R4, R5] |
| B16 | S4 | A failed open tears down the live session, including `:merge` from a side pane. | [RUN IF1, MS1]; [editor.rs:3630](helix-view/src/editor.rs#L3630) |
| B17 | S4 | A non-HEAD `:diff-commit` range relabels conflicts as `~ modified`, so Enter opens a diff rather than a merge. | [RUN T2]; [git.rs:370](helix-vcs/src/git.rs#L370) |
| B18 | S4 | `git add` stderr is written over the TUI. | [RUN AA] |

#### What works well (verified)
- Conflicted rows get a distinct `x conflict` label and sort first on a cached listing. Enter opens merge view directly. — [commands/diff.rs:148](helix-term/src/commands/diff.rs#L148), [commands/diff.rs:75](helix-term/src/commands/diff.rs#L75), [commands/diff.rs:190](helix-term/src/commands/diff.rs#L190), [RUN t1, t2]
- The picker filter `%change conf` narrows to conflicts. — [RUN PF]
- Accept and switch work from any line of the conflict, and the cursor lands at the start of the replacement. — [RUN B1-B4, C1-C2]
- Each accept is one undo step, undo restores markers exactly, and `[+]` clears when back at the on-disk state. — [RUN A4-A6]
- Accept uses the current text between the markers, so hand edits to a side survive accepting that side. — [RUN I4, I6]
- Partial work survives: `:w`, leave, `:merge` resumes at "Conflict 1/2". — [RUN W1-W3]
- CRLF bytes are preserved, including on accept-both (`OURS\r\nTHEIRS\r\n`). — [RUN R2]
- A save over an externally changed file is refused and `git add` does not run. — [RUN S2], [document.rs:1061](helix-view/src/document.rs#L1061)
- Quitting cannot discard RESULT by accident. `:q` and `:q!` close the session but keep the pane and its `[+]`. — [RUN N, N2]

#### "Buffer as source of truth": design and consequence
- The stated design: "Buffer is the only state — no ConflictHunk tracking, no line-delta math. Conflict resolution is a single Transaction; undo works for free." — commit b11087624 (`git show b11087624`)
- This replaced a first attempt on `origin/dev-3-way-diff`, whose merge_view held `MergeViewState, ConflictHunk, and conflict parser`. — commit da733dfd7 (2025-11-13)
- Mechanically, every command rescans the rope for markers. — [merge_view.rs:43-68](helix-view/src/merge_view.rs#L43)
- The only state that was ever added is `LastResolution`, which exists to support switching side. — [merge_view.rs:3-17](helix-view/src/merge_view.rs#L3), commit 378847550
- User benefits:
  - Accepts and free-form edits are the same kind of operation in the same buffer.
  - Half-finished work persists to disk as ordinary git markers.
  - Nothing can desynchronise from a hidden hunk model.
- User costs:
  - "Resolved" means only that the marker text is gone. A resolved conflict cannot be revisited, because `n`/`p` find only markers. — [commands/diff.rs:581](helix-term/src/commands/diff.rs#L581)
  - Hand-removing a single marker line makes the conflict stop existing as far as Helix is concerned. — [RUN G2]
  - No record of the side taken survives, beyond the last accept. — [merge_view.rs:6-17](helix-view/src/merge_view.rs#L6)
  - Parsing is purely textual, so content lines can impersonate markers. — [RUN R3-R5]

### Inferences
- B1 and B2 combine into the worst first impression. On opening, the user sees two unlabelled panes of correct-looking code that may have nothing to do with the conflict in RESULT, so the inline `<<<<<<<` block remains the only reliable view. That makes the side panes decoration on long files.
- The design choice itself looks right for Helix. Most defects are missing affordances layered on top, not faults of the model. B3 and B15 are the exceptions, since they follow directly from textual parsing with a weak grammar.
- Smallest high-value fixes, by my reading:
  - Sync side panes on jump, using the conflict's line mapped through the side text.
  - Have the statusline use `display_name()` for virtual docs.
  - Make `conflict_count` also flag lone `<<<<<<<`, `=======` and `>>>>>>>` lines at finish.
  - Add `sticky=true` to the `Space-m` menu, as the Debug and View menus already have.
  - Jump to the first conflict on open.
  - Move to the next conflict after an accept.

### Gaps
- No user testing. Severity ratings are this auditor's judgement.
- Behaviour with language servers attached, the mouse, and narrow terminals beyond the 100, 130 and 230 column widths used was not exercised.

## What is the full journey from noticing a conflict to leaving, and what does each step cost?

### Takeaway
Reaching a conflict takes 3 keys, which is good, but everything after that is a stream of 3-key chords, and a file with N conflicts costs 6N + 9 keys. The user must supply the structure of the task themselves: go to the first conflict, go to the next one, go to the next file, and then commit somewhere else.

### Cited Findings

#### Step-by-step

| Step | Keys | Count | What happens | Evidence |
|---|---|---|---|---|
| Notice | none | n/a | Helix never announces a conflict. There is no on-open notice or statusline segment. The user finds out from git or from seeing markers. | No `conflict` hits in document.rs, statusline.rs, application.rs (grep) |
| Open list | `Space g` | 2 | `x conflict` rows. On a cache miss rows stream in arrival order, so a `~ modified` row can sit above conflicts. | [RUN t1, L1]; [commands/diff.rs:222](helix-term/src/commands/diff.rs#L222) |
| Enter merge | `Enter` | 1 | Opens merge view for a conflict row, an error for marker-less conflicts | [commands/diff.rs:190](helix-term/src/commands/diff.rs#L190), [RUN P] |
| Manual entry | `:merge⏎` / `:3way⏎` | 7 / 6 | Current buffer only, takes no path argument | [typed.rs:3985](helix-term/src/commands/typed.rs#L3985), [typed_diff.rs:228](helix-term/src/commands/typed_diff.rs#L228) |
| First conflict | `Space m n` | 3 | Not automatic: cursor stays at its prior position (1:1 on a fresh buffer), and "Loading changes for: HEAD" lingers in the message line | [RUN t2], [editor.rs:3721](helix-view/src/editor.rs#L3721) sets focus only |
| Next / previous | `Space m n` / `p` | 3 | Wraps silently, shows "Conflict i/N" | [commands/diff.rs:592](helix-term/src/commands/diff.rs#L592) |
| Accept | `Space m o` / `t` / `b` | 3 | Replaces the whole block, cursor stays at start | [commands/diff.rs:518](helix-term/src/commands/diff.rs#L518), [RUN A1, A2] |
| Hand edit | ordinary editing | n/a | RESULT is a normal buffer | [RUN I1] |
| Switch side | `Space m t` etc. | 3 | "Switched to X version" | [commands/diff.rs:437](helix-term/src/commands/diff.rs#L437), [RUN A2, A3] |
| Undo | `u` | 1 | Only from RESULT. From a side pane, `C-w k u` = 3 keys. | [RUN Db2, Db5] |
| Save | `:w⏎` | 3 | Writes, no marker warning, stays `UU` | [RUN F1] |
| Mark resolved | `Space m f` | 3 | Saves and runs `git add`. Session stays open. | [RUN Eb1] |
| Next file | `Space g`, move, `Enter` | 3+ | Cursor pre-selects the file just finished. There is no next-file command. | [commands/diff.rs:208](helix-term/src/commands/diff.rs#L208), [RUN L1, Y1] |
| Leave | `Space m q` | 3 | Back to one pane, previous splits lost | [RUN LY3-LY4] |
| Commit | `:sh git commit --no-edit⏎` | 25 | Works, shows a popup | [RUN commit path in t16.py] |

- Totals: open (3) + N × (`n` + accept, 6) + finish (3) + leave (3) = 6N + 9, or 27 for three conflicts. This is arithmetic from the table above.
- Chords are non-sticky, while the Debug and View menus use `sticky=true`. — [default.rs:239](helix-term/src/keymap/default.rs#L239), [default.rs:253](helix-term/src/keymap/default.rs#L253), [default.rs:327](helix-term/src/keymap/default.rs#L327)
- `]g` does not navigate conflicts:
  - In RESULT it steps through hunks against HEAD, landing on marker fragments.
  - In OURS it stops at auto-merged, non-conflict hunks (lines 60, 90) and leaves THEIRS at 1:1. — [RUN Z1-Z3]
- The `Space-m` popup shows the commands, but also shows diff-only entries `v`, `s`, `c`, `C` that do not apply here. `v` and `s` both error with "Not in a diff view". — [RUN t2, Z4]

#### What the merge view looks like
- Actual layout: RESULT is full-width on **top**, with OURS bottom-left and THEIRS bottom-right. — [RUN t2]
- The doc comment and commit message claim the reverse ("top-left: OURS... bottom: RESULT"). The code keeps RESULT in place and splits after it, and `Tree::split` inserts after the focused node. — [editor.rs:3611-3614](helix-view/src/editor.rs#L3611), [editor.rs:3691-3701](helix-view/src/editor.rs#L3691), [tree.rs:185](helix-view/src/tree.rs#L185)
- Focus starts in RESULT. — [editor.rs:3721](helix-view/src/editor.rs#L3721), [RUN t2]
- All other panes are closed on entry. — [editor.rs:3682-3689](helix-view/src/editor.rs#L3682)
- OURS and THEIRS are read-only snapshots of index stages 2 and 3. — [git.rs:217](helix-vcs/src/git.rs#L217), [diff_merge.rs:1506](helix-term/tests/test/diff_merge.rs#L1506)
- BASE is absent. Only stages 2 and 3 are read. — [git.rs:236-244](helix-vcs/src/git.rs#L236)
- With `merge.conflictStyle=diff3` the base appears inline in RESULT, and the parser reads it, but the accept path discards it, so "accept base" is not available. — [RUN R1], [merge_view.rs:92](helix-view/src/merge_view.rs#L92), [commands/diff.rs:502](helix-term/src/commands/diff.rs#L502)
- Side panes show a whole-file OURS vs THEIRS diff with intraline highlighting. — [editor.rs:3679](helix-view/src/editor.rs#L3679), [RUN AB]
- Both side gutters use the same purple. The side-based red/green colouring only applies to `diff.views`. — [gutter.rs:110](helix-view/src/gutter.rs#L110), [RUN AB]
- Side panes show each side's full text without markers, so line numbers differ from RESULT: 124 vs 120 for the same region. — [RUN t2]
- Pane labels: Helix passes `"HEAD"` and `"incoming"` into `display_name_override`, but `doc.path` is then set to a cache file. — [editor.rs:3661](helix-view/src/editor.rs#L3661), [document.rs:2009-2013](helix-view/src/document.rs#L2009)
- The label never reaches the user:
  - Status line: uses `relative_path()`, so both panes show `<cache>/git-rev/<16 hex>/<file>`. — [statusline.rs:450](helix-term/src/ui/statusline.rs#L450), [document.rs:1970](helix-view/src/document.rs#L1970), [RUN t2]
  - Buffer picker: shows `…/cache/helix/git-rev/<hash>/b.txt`. — [RUN O0]
  - Bufferline: shows `b.txt  b.txt  b.txt`. — [ui/editor.rs:642](helix-term/src/ui/editor.rs#L642), [RUN BL]
  - `display_name()` is used only in error text, the `%` register, `:diff-buffer`, and logging. — grep across helix-term and helix-view
- RESULT's own label is just the filename. Nothing says RESULT. — [RUN t2]
- After a jump, RESULT scrolls minimally. Line 124 landed near the bottom of a 20-row pane with 5 lines below it. — [RUN t2]

### Inferences
- A conflict longer than a few lines will run off the bottom of RESULT after a jump. Combined with half-height panes, this suggests centring on jump.
- The typical user's real path on long files is RESULT-only, reading the inline block and pressing accept. That path works, but it does not need the side panes.
- `Enter` on a row that is not first in the picker adds movement keys, so 6N + 9 is a floor.

### Gaps
- No cached-picker probe shows the sort order after finishing. On cached opens `sort_changes` should place the finished file below the conflicts, but I observed only the cache-miss order.
- The typical `~/.cache/...` status line appearance was inferred from the code, because the probe's cache path was much longer.

## What does the user see about progress, and can they save or stage with markers remaining or mark resolved silently?

### Takeaway
Progress exists only as a transient message after a jump or a failed finish. `:w` never warns about markers. `Space-m f` catches normal unresolved conflicts, but any broken marker pair gets past it and is staged with a success message. It also works on any file at all, so it can mark delete/modify conflicts resolved, or stage unrelated work, without any prompt.

### Cited Findings

#### Progress
- Counts appear only in "Conflict i/N" after `n`/`p`, "No conflicts remaining", and "N conflict(s) still unresolved" on a blocked finish. — [commands/diff.rs:625](helix-term/src/commands/diff.rs#L625), [commands/diff.rs:587](helix-term/src/commands/diff.rs#L587), [commands/diff.rs:645](helix-term/src/commands/diff.rs#L645)
- Accept messages carry no remaining count ("Accepted HEAD version"). — [commands/diff.rs:540](helix-term/src/commands/diff.rs#L540)
- `i/N` is relative to remaining conflicts, so it restarts: after one of three is resolved, the next reads "Conflict 1/2". — [RUN D2, W3], [diff_merge.rs:966](helix-term/tests/test/diff_merge.rs#L966)
- No statusline element exists. The only statusline change on this branch is the version-control ref. — `git diff master..HEAD -- helix-term/src/ui/statusline.rs`
- Which side was taken is not recorded. Side panes do not change after an accept. — [RUN AC]
- RESULT's gutter bar is against HEAD, so accepted-ours lines show no bar and accepted-theirs lines do. — [RUN A1, A2]
- There is no file-level progress in the picker beyond `x conflict` becoming `~ modified` after staging. — [RUN L1]

#### Saving and staging with markers
- `:w` with markers succeeds with only `'b.txt' written, 8L 76B`, and the file stays `UU`. — [RUN F1]
- `write_impl` has no merge check. — [typed.rs:378](helix-term/src/commands/typed.rs#L378)
- The finish gate is `conflict_count > 0`, and that count is only paired `<<<<<<<` ... `>>>>>>>` regions. Lone markers and `=======` are not counted. — [commands/diff.rs:643](helix-term/src/commands/diff.rs#L643), [merge_view.rs:43-63](helix-view/src/merge_view.rs#L43)
- Verified bad outcomes, each ending in "File saved and staged (git add)":
  - Typing text before `<<<<<<<` (`edit <<<<<<< HEAD`) makes the line unrecognised, so `=======` and `>>>>>>>` get staged. — [RUN F2]
  - Deleting only the `<<<<<<<` line: `Space-m n` says "No conflicts remaining", and the staged blob contains `=======` and `>>>>>>> feature`. — [RUN G2, G3]
- Deleting only the `=======` line is caught, because the region is still paired. — [RUN H2, H3]
- The docs overstate this: "`Space-m f` saves the file and stages it... when no conflict markers remain" and "If conflict markers are still present, `Space-m f` reports an error". — [diff-and-merge.md:91-95](book/src/diff-and-merge.md#L91)
- The normal case is tested and works. — [diff_merge.rs:973](helix-term/tests/test/diff_merge.rs#L973)

#### Silent resolution
- `merge_target` falls back to the current view and document when no merge session exists, so `Space-m f` works anywhere. — [commands/diff.rs:413-420](helix-term/src/commands/diff.rs#L413)
- On a modify/delete (`UD`) file it staged the file with no prompt, which resolves the conflict in favour of keeping it. — [RUN K2]
- In a plain non-conflicted file it stages all of that file's work. — [RUN Kc1]
- `Space-m f` does not `git rm` or otherwise choose for deletes, so "accept the delete" is impossible from Helix. — [commands/diff.rs:669](helix-term/src/commands/diff.rs#L669)
- There is no confirmation at any point. — [commands/diff.rs:630-682](helix-term/src/commands/diff.rs#L630)

### Inferences
- The practical risk of B3 is greatest with hand editing, which the design explicitly encourages: a partial manual cleanup that removes the top marker but leaves the rest is a natural slip. A pre-commit hook or `git diff --check` might catch it, but Helix will not.
- Using `git diff --check`-style line-level detection at finish, rather than paired regions, would close B3 at low cost.
- `Space-m f` working on any file is useful for delete/modify, but its behaviour should be named and documented rather than accidental.

### Gaps
- Whether users expect `:w` to warn is a preference question. Git itself allows it, so this is rated as a missing cue, not a bug.

## Does Helix touch the index, how is the operation finished, and are merge, rebase and cherry-pick distinguished?

### Takeaway
Helix runs exactly one mutating git command, `git add -- <file>`. It has no knowledge of whether a merge, rebase or cherry-pick is in progress and offers no way to finish one. The neutral "HEAD" and "incoming" wording happens to stay correct during rebase, but nothing explains that `incoming` is then the user's own work, and the wording never appears on the panes themselves.

### Cited Findings
- The sole git process spawn in non-test code is `git -C <parent> add -- <path>`. Everything else reads via gix. — [commands/diff.rs:665-669](helix-term/src/commands/diff.rs#L665), grep of helix-term, helix-view and helix-vcs
- The save is flushed before `git add`, fixing an earlier race that could stage conflicted content. — [commands/diff.rs:649-657](helix-term/src/commands/diff.rs#L649), commit 4a34e637a
- `merge_finish` calls `editor.save` directly and so skips what `:w` does: trim trailing whitespace, trim final newlines, insert final newline, the undo checkpoint, and auto-format. — [typed.rs:387-402](helix-term/src/commands/typed.rs#L387), [commands/diff.rs:649](helix-term/src/commands/diff.rs#L649)
- `git add` inherits stdio. Outside a repo, `fatal: not a git reposi...` was drawn over the buffer, alongside a correct error message. — [RUN AA], [commands/diff.rs:669](helix-term/src/commands/diff.rs#L669)
- The session stays open after finishing, and `Space-m o` can still flip the buffer. — [RUN Eb1, Eb2]
- There is no operation detection. Greps for `REBASE_HEAD`, `CHERRY_PICK_HEAD`, `MERGE_HEAD`, `rebase-merge` and `rebase-apply` find nothing in helix-vcs, helix-view or helix-term.
- The book never mentions rebase or cherry-pick. — grep of [diff-and-merge.md](book/src/diff-and-merge.md), keymap.md, pickers.md
- Merge finish from inside Helix: `:sh git commit --no-edit` worked (`[main 593b68a] Merge branch 'feature'`). With other files still conflicted it shows git's error in a popup. — [RUN commit path, J2]
- Rebase finish: `:sh git rebase --continue` **failed** with "Too many errors from stdin / There was a problem with the editor 'editor'", leaving the rebase in progress. — [RUN Q4]
- `:sh git -c core.editor=true rebase --continue` succeeded. — [RUN rebase continue in t16.py]
- Rebase labelling, in the merge view during `git rebase main` on `topic`:
  - OURS, HEAD: `value = UPSTREAM`.
  - THEIRS, "incoming": `value = MY-WORK`.
  - `Space-m o` reports "Accepted HEAD version" and takes upstream.
  - `Space-m t` reports "Accepted incoming version" and takes the user's commit. — [RUN Q0-Q2]
- The labels are constants, so they read the same for every operation. — [commands/diff.rs:390-396](helix-term/src/commands/diff.rs#L390), [editor.rs:3661](helix-view/src/editor.rs#L3661)
- The `>>>>>>> 5e2ef1a (my topic work)` marker line in RESULT is the only on-screen clue to which commit is incoming. — [RUN Q0]
- There is no `git mergetool` support or documentation. — grep of book, contrib, helix-term/src

### Inferences
- "HEAD" and "incoming" are a better vocabulary than ours and theirs during a rebase, because they stay literally true. The keys `o` and `t` still teach the ours/theirs mnemonic, and most users think of `t` as "not mine". During a rebase it is exactly theirs-means-mine, which is the classic inversion.
- A one-line header per pane ("Current change (upstream)" and "Your commit abc123 being replayed") would need only a read of `REBASE_HEAD` or `CHERRY_PICK_HEAD`.
- The `:sh git rebase --continue` failure is likely general. `:sh` provides no terminal, so any editor launch should fail. Only the unset-`EDITOR` case was tested.
- Cherry-pick and stash-pop should behave like rebase, since the same code path applies. Not run.
- The user must leave Helix (or memorise a `:sh` incantation) to finish a rebase or cherry-pick, so the claim "resolve conflicts without leaving the editor" holds only up to staging. — [diff-and-merge.md:3](book/src/diff-and-merge.md#L3)

### Gaps
- `:sh git rebase --continue` was not tested with `EDITOR=hx` or `GIT_EDITOR` set.
- Cherry-pick, `git am -3`, `git revert` and `git stash pop` conflicts were not run live.

## Which edge cases are handled, which fail, and how visibly?

### Takeaway
Plain content conflicts, add/add, diff3 and CRLF work. Anything without paired markers is an unexplained dead end. Hand-resolved but unstaged files reach a state where merge view refuses to open, with a working escape hatch that nothing mentions. External changes are guarded at save time but not noticed sooner.

### Cited Findings

| Case | Outcome | Visibility | Evidence |
|---|---|---|---|
| Content conflict, default style | Works | Good | [RUN A, B] |
| add/add | Works, no base needed | Good | [RUN P addadd.txt] |
| diff3 | Works. Base shown inline only, dropped on accept. | Good | [RUN R1] |
| zdiff3 | Not run (git 2.34.1). Parser is exercised on the diff3 grammar. | Unknown | [merge_view.rs:162](helix-view/src/merge_view.rs#L162) |
| CRLF | Works, bytes preserved | Good | [RUN R2] |
| Delete/modify (`UD`, `DU`) | Row shows `x conflict`. Enter: "Failed to open merge view: No conflict markers found in file". File not opened. | Misleading: the conflict is real | [RUN P, K1] |
| Rename conflicts (`AU`, `UA`) | "Failed to open merge view: Failed to fetch merge versions from git index" | Cause hidden | [RUN P] |
| Rename `DD` | "No conflict markers found in file" | Misleading | [RUN P] |
| Binary | "No conflict markers found in file" | Misleading | [RUN P bin.dat] |
| `=======` line inside a side | Accept: "Malformed conflict markers" (safe). Must hand-edit. Common with setext headings. | Clear but unhelpful | [RUN R3] |
| `>>>>>>>` line inside theirs | Region ends early. Accept drops the quoted line, leaves `>>>>>>> feature`. `n` says none remain. Finish stages it. | **Silent, wrong** | [RUN R4] |
| `<<<<<<<` line inside ours | Region starts late. Accept ours leaves `<<<<<<< HEAD`, drops the side. Finish stages it. | **Silent, wrong** | [RUN R5], [merge_view.rs:50](helix-view/src/merge_view.rs#L50) |
| Nested longer markers (handcrafted, plain file) | `starts_with` matches 9-char markers. Inner region only, count 1. Cursor on outer ours: "No conflict at cursor". Accept peels the inner one first. | Confusing, recoverable | [RUN NEST] |
| Empty side | Accept works. Switching away from the empty side works once, then strands. | Partly silent | [RUN ES1-ES3] |
| Conflict at EOF, no final newline | Accept adds a final newline neither side had | Minor | [RUN EOF2] |
| Unsaved changes when entering | The open buffer is used as-is, including any unsaved edits | n/a | [editor.rs:3638](helix-view/src/editor.rs#L3638) |
| Buffer open before merge started | Picker `x conflict`, Enter and `:merge`: "No conflict markers found in file" while stale text is on screen. `:reload` fixes it. | **Misleading** | [RUN M2-M5] |
| File changes on disk mid-merge | RESULT does not update. Accept continues in memory. Finish: "Failed to save: file modified by an external process, use :w! to overwrite". | Late but clear | [RUN S1, S2, S4a] |
| `git merge --abort` mid-merge | Session stays open with stale panes. Same save guard applies. `:reload` clears `[+]` and resyncs with no prompt. | Late | [RUN S1-S3] |
| Half-resolved, re-enter | Works, side panes rebuilt from the index | Good | [RUN W1-W3] |
| All resolved via `:w`, not staged | `:merge` and picker Enter both error, row stays `x conflict`. `Space-m f` still works outside a session. | Dead end with hidden exit | [RUN W5-W7] |
| Finished, then `u` and `:w` | File has markers, index holds the resolution, conflict stages are gone. `:merge`: "Failed to fetch merge versions". Row is `~ modified`. | Hidden | [RUN UF1-UF4] |
| File with markers, no merge running | `:merge` errors, but `Space-m n` and `t` work without panes | Undocumented fallback | [RUN PM1-PM3] |

- Quit and leave behaviour with dirty RESULT:

| Command | Effect | Evidence |
|---|---|---|
| `:q`, `:q!` | Session closes, RESULT stays with `[+]`, hx stays running | [RUN N] |
| `:wq`, `:x` | Writes RESULT, session closes, hx stays running, file stays `UU` | [RUN N] |
| `:qa` | Refused: "1 unsaved buffer remaining", session intact | [RUN N] |
| `:qa!`, `:wqa` | First run closes the session only. Second run exits. | [RUN N3] |
| `:q` from OURS | Session closes, RESULT kept | [RUN N2] |
| `C-w q` in OURS | Whole session closes, RESULT kept | [RUN O3] |
| `:o other` from RESULT or OURS | Session ends silently. Dirty RESULT stays in the buffer list. | [RUN O1, O4] |
| `ga` back to RESULT | No session restored | [RUN O2] |
| `Space g` to another conflict | Old session replaced, dirty RESULT stays hidden in the buffer list | [RUN Y1] |

- `:qa!` needing two runs comes from `quit_all_impl` closing views in order while `Editor::close` keeps the last view when a session closes. — [typed.rs:1083](helix-term/src/commands/typed.rs#L1083), [editor.rs:2149](helix-view/src/editor.rs#L2149)
- Cause detail is lost in error messages. "OURS (stage 2) not in index — is there an active merge?" is produced, but only the outer anyhow context is displayed. — [git.rs:246](helix-vcs/src/git.rs#L246), [editor.rs:3655](helix-view/src/editor.rs#L3655), [commands/diff.rs:193](helix-term/src/commands/diff.rs#L193), [typed_diff.rs:242](helix-term/src/commands/typed_diff.rs#L242), [RUN PM1]
- `:diff-commit <sha>` with base ≠ HEAD relabels conflicted files, because paths already seen in the tree diff are filtered out of `status`. Rows showed `~ modified` and `- deleted`, with `x conflict` only on the remainder. — [git.rs:370](helix-vcs/src/git.rs#L370), [RUN T2]
- A two-ref range lists only tree changes, so conflicts cannot appear. — [git.rs:375](helix-vcs/src/git.rs#L375)
- `:diff-commit HEAD~1` produced an empty 0/0 picker with no error during the merge. — [RUN T1]
- Conflict labels were stable across 6 cold starts, so a labelling race I suspected in `status()`, where the two scan halves report in undefined order, did not appear. — [RUN U], [git.rs:717](helix-vcs/src/git.rs#L717)
- Test coverage: integration tests use only plain content conflicts. None cover rebase, diff3, CRLF, delete/modify or binary. — grep of [diff_merge.rs](helix-term/tests/test/diff_merge.rs). Unit tests cover diff3 parsing and malformed markers. — [merge_view.rs:162-199](helix-view/src/merge_view.rs#L162)

### Inferences
- The marker-less conflict dead end is the most likely edge case to hit, since delete/modify and rename conflicts are routine in refactoring branches. Opening the file (or a two-pane view of the surviving side) with a message naming the conflict type would remove the worst of it.
- The stale-buffer case is likely very common, because running `git merge` in another terminal with `hx` open is a normal workflow. A reload prompt, or reading from disk when the buffer is unmodified, would resolve it.
- zdiff3 uses the same marker lines as diff3, so it should parse identically, but this was not run.
- `self.open(&path, Action::Load)` runs before the marker check, so a failed open likely leaves a hidden buffer loaded. — [editor.rs:3641](helix-view/src/editor.rs#L3641)
- The real nested case needs a criss-cross merge with diff3 style. The handcrafted file mirrors the expected shape, but its fidelity to git's real output is unconfirmed.

### Gaps
- Submodule and symlink conflicts, large files, `conflict-marker-size`, `core.autocrlf` smudging, and a `:merge` buffer with unsaved edits were not exercised.
- The cause of the 0/0 picker for `HEAD~1` was not diagnosed. It looks like a general revision-parsing issue rather than a merge one.

## Do accept commands act on the correct conflict and pane wherever the focus is, and why was `merge_commands_target_result_from_ours_pane` failing?

### Takeaway
Yes: accept, jump and finish all target RESULT from any pane. The named test **passes at HEAD**, 7 runs out of 7 with `--exact`, so the "known to fail" premise is out of date. It used to fail on a stale assertion about document paths, not on targeting. That same path change does underlie the real label bug (B2), and the test's own scenario exposes a separate bug it does not assert: the stuck `[+]` (B11).

### Cited Findings

#### Targeting
- `merge_target` maps any of the three merge views to `result_view_id` and `result_doc_id`. Accept, jump and finish all use it. — [commands/diff.rs:413-420](helix-term/src/commands/diff.rs#L413), commit e9d5662da
- Live, accept from OURS edited RESULT at RESULT's cursor, and switch from OURS worked. — [RUN D1, Db1, Db3]
- Finish from OURS staged the right content. — [RUN E2]
- The side pane's own cursor is ignored. With OURS parked on line 120 and RESULT's cursor on conflict 1, `Space-m t` resolved conflict 1. — [RUN SP1]
- Focus behaviour is asymmetric: accept leaves focus where it was, while `n`/`p` move focus to RESULT. — [commands/diff.rs:617](helix-term/src/commands/diff.rs#L617), [RUN D2, E0]
- A cursor outside any conflict gives "No conflict at cursor" at info severity, in the same style as success messages. — [commands/diff.rs:498](helix-term/src/commands/diff.rs#L498)

#### The test
- Passed in 7 of 7 isolated runs at HEAD (`cargo test -p helix-term --features integration --test integration -- --exact test::diff_merge::merge_commands_target_result_from_ours_pane`, then the test binary directly).
- The historic failure, per the project memory note, was `ours_doc_id ... path().is_none()` at diff_merge.rs:1184.
- Commit c722a0045 (2026-09-18) gave virtual revisions a real cache path for LSP, which made that assertion false. — [document.rs:2013](helix-view/src/document.rs#L2013)
- Commit 98c8b07dd (2026-10-30) rewrote the assertion to `assert_ne!(side.path(), Some(&conflict_path))` plus a `git_revision.path` check. — [diff_merge.rs:1194](helix-term/tests/test/diff_merge.rs#L1194)
- So the test failure never reflected wrong targeting.
- The test still does not cover the accompanying bug. After accept and finish from OURS, RESULT keeps `[+]`, and the helper's trailing `:q!` hides this. — [RUN E2-E7], [helpers.rs:170](helix-term/tests/test/helpers.rs#L170)

### Inferences
- The memory note saying this test fails should be updated for this branch. It may still be true on other branches.
- Parked-side-pane targeting is defensible, since RESULT stays visible, but with unsynced panes a user can scroll OURS to conflict 2, press accept, and resolve conflict 1. Syncing side panes (B1) or moving the RESULT cursor to follow the side pane would remove the trap.

### Gaps
- Mouse-driven focus changes between panes were not tested.
- The test was run from a `cargo test --features integration` build, not under the `cargo integration-test` profile.

## How do undo and redo interact with accepting a side, and is any data-loss path possible?

### Takeaway
From RESULT, undo is clean: one step per accept, with exact restoration. Undo does not work from side panes, side-pane accepts can merge into one undo step, and switch-side is fragile. I found no path that destroys committed or on-disk data irrecoverably within a session. The real loss risks are silent content loss via false markers, and accept discarding hand edits, which is undoable only if noticed.

### Cited Findings
- History commits happen after each key for the **focused** document only. — [ui/editor.rs:1510-1515](helix-term/src/ui/editor.rs#L1510)
- Accept is one `Transaction`, applied to RESULT with no explicit checkpoint. — [commands/diff.rs:518-525](helix-term/src/commands/diff.rs#L518)
- From RESULT: accept, switch, switch then `u` × 3 walks back exactly through each state to markers, and `U` replays. — [RUN A3-A8]
- From a side pane: `u` acts on the read-only snapshot ("Already at oldest change") and RESULT is untouched. — [RUN Db2]
- Accept then switch from a side pane is stored as one revision, so a single `u` in RESULT reverts both. — [RUN Db3-Db6]
- Returning to RESULT between accepts commits them separately. — [RUN HG1-HG2]
- Stuck `[+]`:
  - `is_modified` includes uncommitted changes. — [document.rs:1831](helix-view/src/document.rs#L1831)
  - `merge_finish` skips the `:w` checkpoint at [typed.rs:398](helix-term/src/commands/typed.rs#L398).
  - After finishing from OURS the disk and index are right but `[+]` persists, and `:q` in the last pane reports "1 unsaved buffer remaining". — [RUN E2-E7]
- Switch-side validity requires the cursor inside the replacement and an unchanged `doc.version()`. — [commands/diff.rs:438-446](helix-term/src/commands/diff.rs#L438)
- It fails with "No conflict at cursor" after:
  - Undo followed by redo. — [RUN A9]
  - Any edit. — [RUN I7]
  - `Space-m n`, even when it reports "No conflicts remaining". — [RUN JM1-JM2], [commands/diff.rs:584](helix-term/src/commands/diff.rs#L584)
  - Switching from an empty side, because the cursor lands one past the insertion and `contains_cursor` treats the end as exclusive. — [RUN ES1-ES3], [merge_view.rs:25-31](helix-view/src/merge_view.rs#L25)
- After finishing, switch still works and dirties the buffer. The index keeps the old version until a re-finish. — [RUN Eb2, Eb3]

#### Data-loss analysis
| Path | Result | Recoverable? | Evidence |
|---|---|---|---|
| Accept with hand edits inside the block | Non-chosen side's edits discarded, no warning | Yes, `u` | [RUN I1-I3] |
| False `>>>>>>>` or `<<<<<<<` in content | Lines dropped, marker left, stageable | `u` before saving; after that via git only | [RUN R4, R5] |
| Finish, undo, `:w` | Markers on disk, stale resolution in index, merge view refuses | Accept commands still work without panes; `git` for the rest | [RUN UF1-UF4], [RUN PM3] |
| Leaving with dirty RESULT via `:o` or `Space g` | Unsaved buffer stays, hidden, in the buffer list | Yes | [RUN O1, Y1] |
| `:reload` with a dirty RESULT | Buffer replaced from disk, `[+]` cleared, no prompt. The probe's disk text matched the buffer, so loss itself was not demonstrated. | Not tested | [RUN S3] |
| Mid-merge external change | Save refused | Yes | [RUN S2] |
| `:q!` twice | Discards buffer changes, disk markers intact | By design | [RUN N, N3] |

### Inferences
- Because the file on disk retains markers until the user saves, and git retains stages 1-3 until `git add`, a session in which the user only accepts and quits is fully recoverable. After `Space-m f`, conflict stages are gone, and recovery would need `git checkout -m <file>`. That command was not tested here.
- Adding `append_changes_to_history` for RESULT in `merge_accept_impl` would fix both the stuck `[+]` and side-pane undo grouping.
- Allowing `u` in a side pane to act on RESULT would match the principle already used by accept, jump and finish.

### Gaps
- Redo across the session boundary (undo after leaving and re-entering) was not tested.
- `git checkout -m` recovery was not exercised.

## How does the design intent, as seen in commits and docs, compare with what shipped, and what looks unfinished?

### Takeaway
The founding "buffer is the only state" idea shipped intact, with one deliberate concession for switch-side. Much of the surrounding UX was declared or implied and then not finished: sync scroll is a dead field, the layout is inverted from its description, merge side colours and revision labels do not reach the screen, and recent robustness work on diffs stopped short of merge. The docs cover the happy path only, and overstate the guard on `Space-m f`.

### Cited Findings

| Intent and source | Shipped? | Evidence |
|---|---|---|
| "Buffer is the only state", b11087624 | Yes. `LastResolution` is the one deliberate addition (378847550). | [merge_view.rs:3](helix-view/src/merge_view.rs#L3) |
| "undo works for free", b11087624 | From RESULT yes, from side panes no | [RUN A4-A6, Db2] |
| "OURS \| THEIRS top, RESULT editable bottom", b11087624 and code comments | **Inverted**: RESULT on top | [editor.rs:3611](helix-view/src/editor.rs#L3611), [RUN t2] |
| `sync_scroll` flag on `MergeViewState`, `s` in `Space-m` | **Dead**: set, never read. `Space-m s` errors in merge view. | [editor.rs:1291](helix-view/src/editor.rs#L1291), [commands/diff.rs:686](helix-term/src/commands/diff.rs#L686), [RUN t2] |
| "char-diff wired between OURS and THEIRS" | Yes | [RUN AB] |
| "merge_finish: save and `git add` when no conflicts remain" | Partly: "no conflicts" means no paired regions | [RUN F2, G3] |
| 53836d002 "side-based gutter colors for split diff/merge views" | Diff only. Merge panes are not in `diff.views`, so both show the same colour. | [gutter.rs:110](helix-view/src/gutter.rs#L110), [RUN AB] |
| `filename @ ref` naming | Field set. Not rendered in status line, bufferline or buffer picker. | [document.rs:2009](helix-view/src/document.rs#L2009), [RUN t2, BL, O0] |
| e9d5662da "Keep merge accept/jump/finish on RESULT" | Yes | [commands/diff.rs:413](helix-term/src/commands/diff.rs#L413) |
| 534b47de8 auto-open merge view for conflicts in `Space-g` | Yes at the default HEAD base. Non-HEAD ranges hide it. | [RUN t2, T2] |
| de51c5ef2 "preserve comparisons on open failure" | Diff only. `open_merge_view` closes sessions before validating. | [editor.rs:3630-3635](helix-view/src/editor.rs#L3630), [RUN IF1, MS1], [diff-and-merge.md:63](book/src/diff-and-merge.md#L63) |
| 98c8b07dd "preserve diff sessions across buffer navigation" | Diff only. Leaving a merge view closes it permanently. | [editor.rs:1929](helix-view/src/editor.rs#L1929), [editor.rs:3547](helix-view/src/editor.rs#L3547), [RUN O2] |
| f153b900e keep a view when closing a split or merge pane | Yes for `:q`. Side effect is the two-run `:qa!`. | [diff_merge.rs:1380](helix-term/tests/test/diff_merge.rs#L1380), [RUN N3] |
| "resolve conflicts without leaving the editor" (book) | Only up to staging | [diff-and-merge.md:3](book/src/diff-and-merge.md#L3), [RUN Q4] |
| "If conflict markers are still present, `Space-m f` reports an error" (book) | **False** for broken pairs | [diff-and-merge.md:94](book/src/diff-and-merge.md#L94), [RUN F2] |

- Commit history shows repeated hardening of this path rather than new UX:
  - 4a34e637a: stage race.
  - e9d5662da: targeting.
  - f153b900e: closing.
  - abb396cd3: focus.
  - 9988007b4: `:bc` panic.
  - After 378847550 (2026-04-18), merge-tagged commits are refactors, fixes, and diff-led features that touch merge only incidentally (53836d002, 34cf2cdeb). — `git log master..HEAD --grep=merge -i`
- The Merge resolution section of the book is 12 lines and omits:
  - Pane layout.
  - Pane meaning.
  - Undo.
  - Switch-side, which is undocumented anywhere in the book.
  - `Space-m q` in context.
  - What to do after staging.
  - Marker-less conflicts.
  - Rebase semantics. — [diff-and-merge.md:82-95](book/src/diff-and-merge.md#L82)
- Unlike the docs for diffs, nothing states that returning to a merged file does not restore the session. — [diff-and-merge.md:56-59](book/src/diff-and-merge.md#L56)

### Inferences
- The branch name and history suggest a deliberate rewrite to reduce complexity: from a hunk-tracking model on `origin/dev-3-way-diff` to a stateless one. That simplification succeeded. The polish work then went almost entirely to the diff half of the feature, leaving merge at roughly its first-working state.
- The `sync_scroll` field, the `s` key's presence in the commit's keymap list, and the "split diff/merge" commit title together suggest merge parity was planned but dropped, not rejected.
- Pinning merge behaviours with tests for labels, sync and `[+]` state would stop further drift, since the current test scenario passes while user-visible defects exist.

### Gaps
- The author's intent is inferred only from commit messages and code comments. No design document for the merge view was found in the repo: `docs/` mentions merge only in `releases.md`, and `reports/` is empty.
- No comparison against other merge tools was attempted, because it was out of the assigned scope and would need external sources.
