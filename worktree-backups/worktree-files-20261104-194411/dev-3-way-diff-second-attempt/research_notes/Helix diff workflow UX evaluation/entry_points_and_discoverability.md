# Entry points and discoverability of the Helix diff and merge workflows

Scope: branch `dev-3-way-diff-second-attempt` (HEAD `de51c5ef2`) against local `master`. Links are relative to the worktree root. `commands/diff.rs` = `helix-term/src/commands/diff.rs`; `ui/diff.rs` = `helix-term/src/ui/diff.rs`; `view/editor.rs` = `helix-view/src/editor.rs`; `typed.rs` and `typed_diff.rs` are in `helix-term/src/commands/`; `default.rs` = `helix-term/src/keymap/default.rs`; `git.rs` = `helix-vcs/src/git.rs`.

**Evidence labels.** `[live]` means observed by driving the real `target/debug/hx` in tmux against throwaway repos in a scratch directory (a repo with commits and tag `v1`, a mid-merge repo, a clean repo, a `git mv` repo, a non-git directory), using an isolated config and cache. Strings under `[live]` were captured from the screen, not read from source. `[code]` means derived from reading source only and not exercised. Caveat on the binary: `hx --version` prints `e1be05fe`, which is 7 commits behind HEAD, but this is a stale version string and not an old build. The binary's mtime is later than every source file (0 sources newer), and it contains `suspend_diff_view`, added by `182cae713`, a commit after `e1be05fe`. `helix-loader/build.rs:64,80` only re-runs on HEAD or ref file changes, which can miss commits made in a worktree (the worktree explanation is an inference).

## Overall UX judgement: is entry and discovery good, and what is most severe?

### Takeaway
The happy path is genuinely good: `Space-g`, then Enter, gets a file into a diff or a conflict into a merge view with no new habit, and the `Space-m` infobox makes the in-view actions findable. The weakness is everything around that path: bad input reports success and then shows a misleading list, the file-level identity of what you are looking at is invisible, the default view hides the old text, and `Space-m f` stages files with no context guard. The five High findings (F1 to F5 below) should be fixed before this surface is considered finished.

### Cited Findings

**What is good**
- Reusing `Space-g` means no new habit. Conflicts route to merge view and everything else to diff view automatically — [commands/diff.rs:186-204](helix-term/src/commands/diff.rs#L186).
- The picker pre-selects the current file, so `Space-g Enter` is effectively "diff this file" — [commands/diff.rs:208-212](helix-term/src/commands/diff.rs#L208), `[live]` cursor landed on `u.txt` when `u.txt` was the open buffer.
- Conflicts sort first and untracked files last — [commands/diff.rs:73-81](helix-term/src/commands/diff.rs#L73).
- The `Space` infobox shows `m  Merge/Diff`, and `Space-m` lists all 12 actions in plain language — `[live]`, [default.rs:239-252](helix-term/src/keymap/default.rs#L239).
- `:diff` completion lists every `diff-*` command, and the prompt shows the doc and aliases inline — `[live]`, [typed.rs:4069-4078](helix-term/src/commands/typed.rs#L4069).
- Mnemonics are strong: `o`/`t`/`b` for ours, theirs, both; `n`/`p` for next, previous; `q` for close; `f` for finish — [default.rs:240-251](helix-term/src/keymap/default.rs#L240).
- In-merge feedback is good: `Conflict 1/2`, wrap-around, `Switched to incoming version` for changing your mind, and `1 conflict(s) still unresolved` — `[live]`, [commands/diff.rs:434-476](helix-term/src/commands/diff.rs#L434).
- `Space-m c` on HEAD emits a model message: `Diff set: 31acd5a (HEAD) vs working tree: uncommitted changes only, space m C shows the commit` — `[live]`, [commands/diff.rs:957-968](helix-term/src/commands/diff.rs#L957).
- `book/src/diff-and-merge.md` exists as one linked page, and `keymap.md`'s 12-key table matches `default.rs` exactly — [book/src/keymap.md:328-349](book/src/keymap.md#L328).

**What is wrong, ranked** (High = wrong outcome or data-affecting; Medium = confusing, recoverable; Low = polish)

| ID | Severity | Finding | Evidence |
|---|---|---|---|
| F1 | High | Bad refs and non-repos fail silently. `:diff-commit nosuchref` reports success, then `Space-g` shows only untracked files with no error. Outside a repo, `Space-g` shows `0/0`. This is a regression: master surfaced an error | `[live]`; errors dropped by `let _ =` at commands/diff.rs:39,49,247,265 |
| F2 | High | Common ref syntax fails or silently means something else. `HEAD~N` is documented but rejected; `main...HEAD` yields `0/0`; git's `HEAD^!` selects the wrong commit | `[live]`; git.rs:570-614; view/editor.rs:1207-1233 |
| F3 | High | `Space-m f` is not gated to merges. In any buffer it saves and runs `git add`, including on an untracked file | `[live]` `git status` went from `?? u.txt` to `A  u.txt`; commands/diff.rs:413-420,630-671 |
| F4 | High | You cannot tell what a pane shows. The documented `filename @ ref` name is rendered nowhere. Base, snapshot, OURS and THEIRS panes show a cache path, with no ref or side label | `[live]`; statusline.rs:450; commands.rs:3213; ui/editor.rs:645 |
| F5 | High | The default single-pane "diff" shows no removed or old text, and its text layout is identical to a plain buffer. The only setting that fixes it is undocumented | `[live]`; book/src/editor.md has no `[editor.diff]` section |
| F6 | Medium | Entering a view is silent. There is no hint, no conflict count, and no cursor move to the first conflict, and a stale `Loading changes for: HEAD` stays on screen | `[live]`; commands/diff.rs:333-334 |
| F7 | Medium | `Space-g` regressed picker conventions: `Ctrl-s`, `Ctrl-v` and `Alt-Enter` are ignored because the action argument is `_action` | `[live]`; commands/diff.rs:186 vs master's `cx.editor.open(path_to_open, action)` |
| F8 | Medium | Split diff and merge view close all other panes and never restore them, undocumented | `[live]` three panes became one; view/editor.rs:3010-3030,3682-3689 |
| F9 | Medium | The active range is global, sticky, and invisible; `Space-m v` is similarly sticky and beats config | `[live]`; statusline shows neither by default |
| F10 | Medium | `Space-m c`/`C` depend on a git log buffer the editor cannot produce and the book never explains | `[live]`; no log picker exists (`grep`) |
| F11 | Medium | `Space-m` hides its own prerequisites: the two ways in (`Space-g`, `:merge`) are not listed, and 11 of 12 entries need a context | `[live]` infobox |
| F12 | Medium | `:diff-files` is misdescribed and takes a narrower grammar than `:diff-commit` | `[live]` |

### Inferences
- The pattern behind F1, F2 and the Q4 false successes is "state set first, validated never". `:diff-commit` never asks git anything, even though `resolve_commit_id` exists and `Space-m c` already uses it [commands/diff.rs:961-963](helix-term/src/commands/diff.rs#L961). Validating at set time would fix most of the High items with one call.
- F4 and F5 compound: in the default configuration a reviewer sees a screen that looks like a normal buffer, with no statement of what it is compared against.
- A helpful way to frame the design: `Space-m` is an "inside the view" menu, but it is presented as if it were the workflow's front door.

### Gaps
- No usability testing with real users. All severity ratings are expert judgement.
- No external benchmarking against vim-fugitive, lazygit, Zed or VS Code. The conventions compared are Helix's own, from this repo.

## What is the complete inventory of user-facing entry points?

### Takeaway
The branch adds 8 typable commands, 12 static commands all bound under a new non-sticky `Space-m` menu, 1 repurposed key (`Space-g`), 2 config sections, 1 new theme key and 1 changed statusline element. That gives 13 default keys (`Space-g` plus 12 under `Space-m`). Six commands have no default key: `:merge`, `:diff-buffer`, `:diff-files`, `:diff-commit`, `:diff-base` and `:toggle-char-diff`.

### Cited Findings

**Typable commands** ([typed.rs](helix-term/src/commands/typed.rs))

| Command | Alias | Args | What it actually does | Preconditions | Citation |
|---|---|---|---|---|---|
| `:diff-base [REF]` | `:db` | 0-1 | Changes the current buffer's gutter base. Does **not** open a diff view. No arg resets that buffer to HEAD | File in a repo and present at REF | typed.rs:3475; typed_diff.rs:5-43 |
| `:diff-commit RANGE` | `:dc` | exactly 1 | Sets the global range `Space-g` uses. Opens nothing, validates nothing. Forms: `REF`, `REF!`, `REF1..REF2` | None enforced | typed.rs:3952; typed_diff.rs:205-226 |
| `:diff-files REF` | `:df` | exactly 1 | Opens a one-off picker of REF vs working tree. Does not change the sticky range. Raw ref only | A repo in the cwd | typed.rs:3974; typed_diff.rs:287-363 |
| `:diff-reset` | `:dr` | 0 | Closes the current diff, clears the range and suspended diffs, resets every buffer's base, turns char-diff off everywhere. Same as `Space-m r` | None | typed.rs:3963; typed_diff.rs:245-285 |
| `:diff-buffer [BUFFER]` | `:dbuf` | 0-1 | Opens a side-by-side diff of two open buffers; picker when no arg. Accepts id, relative or absolute path, display name. Other buffer is the base | Another open buffer | typed.rs:3941; typed_diff.rs:82-112,179-203 |
| `:toggle-char-diff` | `:tcd` | 0 | Toggles intraline highlighting for this buffer or view | Useful only with a diff handle | typed.rs:3930; typed_diff.rs:45-65 |
| `:merge` | `:3way` | 0 | Opens the 3-way view of the current file | Buffer has a path, conflict markers, and index stages 2 and 3 | typed.rs:3985; typed_diff.rs:228-243; view/editor.rs:3615 |
| `:copy-path [url\|absolute]` | `:cp` | 0-1 | Copies the path; `url` copies a commit permalink only in a pane showing a revision | Clipboard; for URLs a remote | typed.rs:579,638,3081 |

Pre-existing but changed: `:reset-diff-change` (`:diffget`, `:diffg`) now uses the view's diff — [typed.rs:2640](helix-term/src/commands/typed.rs#L2640).

**Keys** (all also bound in select mode — [book/src/generated/static-cmd.md](book/src/generated/static-cmd.md); doc strings at [commands.rs:557-568](helix-term/src/commands.rs#L557))

| Key | Static command | Infobox text | Works when |
|---|---|---|---|
| `Space-g` | `changed_file_picker` | Open changed file picker | Cwd in a repo. Now opens diff or merge view instead of the file; uses the sticky range |
| `Space-m o` | `merge_accept_ours` | Accept HEAD version for current conflict | Cursor on a conflict, in **any** buffer |
| `Space-m t` | `merge_accept_theirs` | Accept incoming version for current conflict | As above |
| `Space-m b` | `merge_accept_both` | Accept both versions for current conflict | As above |
| `Space-m n` / `p` | `merge_next_conflict` / `merge_prev_conflict` | Jump to next / previous merge conflict | Any buffer with markers; wraps; moves focus to RESULT |
| `Space-m f` | `merge_finish` | Save and stage the resolved file | **Any** buffer with zero markers. Leaves the view open |
| `Space-m v` | `diff_toggle_split_view` | Toggle between single-pane and side-by-side diff | Diff view only |
| `Space-m s` | `diff_toggle_sync_scroll` | Toggle synchronized scrolling in split diff | Diff view only (not merge) |
| `Space-m r` | `diff_reset` | Reset diff base to HEAD | Always; leaves merge views open |
| `Space-m c` | `diff_commit_from_selection` | Set diff range from selected git log lines, bottom line as base | Primary selection begins with a 7-40 hex token |
| `Space-m C` | `diff_show_commit_from_selection` | Show changes introduced by selected commit | Exactly one such line. Sets the range only; shows nothing |
| `Space-m q` | `close_diff_or_merge_view` | Close diff or merge view | Inside either |

Behaviour-changed existing keys: `]g`, `[g`, `]G`, `[G` and the `g` textobject now follow the view's diff, and in a diff view they collapse to a point and move the paired pane — [commands.rs:3956-3985](helix-term/src/commands.rs#L3956). Key sources: [default.rs:236](helix-term/src/keymap/default.rs#L236), [default.rs:239-252](helix-term/src/keymap/default.rs#L239).

**Config** ([view/editor.rs:378-381](helix-view/src/editor.rs#L378))
- `[editor.diff] split-view` (bool, default `false`) — [view/editor.rs:515-520](helix-view/src/editor.rs#L515). `[live]` it works from the config file and from `:set diff.split-view true`.
- `[editor.git-remote] url-template`, `line-template`, `line-range-template` (default: chosen per host) — [view/editor.rs:537-566](helix-view/src/editor.rs#L537).

**Theme keys**
- New: `ui.diff.spacer`, with a computed fallback — [ui/diff.rs:52-73](helix-term/src/ui/diff.rs#L52).
- Reused with new meaning: `diff.plus`, `diff.minus` for intraline highlights — [ui/diff.rs:135-140](helix-term/src/ui/diff.rs#L135).
- Reused: `diff.plus.gutter`, `diff.minus.gutter` now colour a whole pane by side in splits — [helix-view/src/gutter.rs:97-120](helix-view/src/gutter.rs#L97).
- Reused: `diff.delta.conflict`, `diff.delta.moved` in the picker — [commands/diff.rs:137-141](helix-term/src/commands/diff.rs#L137).

**Other surfaces**
- The statusline `version-control` element gains `branch@ref` when `:diff-base` is active — [statusline.rs:545-549](helix-term/src/ui/statusline.rs#L545).
- There is no CLI entry and no tutor content: `git diff master...HEAD` touches neither `helix-term/src/args.rs` nor `runtime/`.

### Inferences
- There is no direct "open a diff view of this file against REF" command. The closest route is `:diff-commit REF`, `Space-g`, Enter, and that fails if the file has no changes.
- Without CLI arguments, Helix cannot be used as `git mergetool` or `git difftool`, so the workflow cannot start from a terminal failure.

### Gaps
- `:3way` was checked by reading the goto-line rule [typed.rs:4019](helix-term/src/commands/typed.rs#L4019), which only fires for a whole numeric token; the alias was not run live.

## Are names, argument forms and key groupings consistent with each other and with Helix conventions?

### Takeaway
The surface looks consistent (a `diff-*` prefix, a `Space-m` group) but behaves inconsistently: commands sharing a prefix do different kinds of things, the range grammar differs by command and diverges from git in ways that silently change meaning, and three Helix conventions are broken (picker actions, `]`/`[` navigation, multi-selection).

### Cited Findings

**Names**
- `diff-*` as a noun-prefix family matches `lsp-*`, `debug-*`, `tree-sitter-*` — [typed.rs:3475-3985](helix-term/src/commands/typed.rs#L3475).
- Same prefix, four different behaviours: `:diff-base` retargets a gutter, `:diff-commit` sets hidden state, `:diff-files` opens a picker, `:diff-reset` tears down state. The names do not reveal this — [typed_diff.rs:5-363](helix-term/src/commands/typed_diff.rs#L5).
- "Show changes introduced by selected commit" (`Space-m C`) shows nothing; it only sets a range — [commands/diff.rs:998-1019](helix-term/src/commands/diff.rs#L998).
- `:toggle-char-diff` sits outside the prefix, as does `close_diff_or_merge_view` among statics; `reset-diff-change` and `diff-reset` invert word order — [typed.rs:3842,3930,3963](helix-term/src/commands/typed.rs#L3842).
- `:db`, `:dbuf` and `:dbg` (the last is `:debug-start`) do not collide but are easy to confuse — [typed.rs:3476,3566,3942](helix-term/src/commands/typed.rs#L3476).
- `:dr`/`Space-m r` is duplicated in full between `diff_reset` and `diff_reset_typed`, which at least guarantees the same behaviour today — [commands/diff.rs:899-926](helix-term/src/commands/diff.rs#L899), [typed_diff.rs:245-285](helix-term/src/commands/typed_diff.rs#L245).

**Argument forms** (all `[live]`)

| Input | Result | Judgement |
|---|---|---|
| `:diff-commit HEAD^..HEAD` | Correct: 1 file | Good |
| `:diff-commit abc1234!` | Becomes `abc1234^..abc1234`, as documented (unit test at view/editor.rs:1256) | Works; the syntax is unusual, see below |
| `:diff-commit HEAD^!` (git's own syntax for "just this commit") | Parsed as REF=`HEAD^`: `Diff range set to: HEAD^^..HEAD^`, showing the **parent's** changes | Silent wrong answer |
| `:diff-commit main...HEAD` (the standard PR-review form) | `split_once("..")` makes the target `.HEAD`; status looks right; picker `0/0` | Silent failure |
| `:diff-commit HEAD~1` | Success status, then only untracked files | Silent failure |
| `:diff-base HEAD~1`, `HEAD^2`, `@` | `not a valid reference or commit hash` | False: they are valid in git |
| `:diff-base HEAD^`, `HEAD^^`, branch, tag, hash | Work | Good |
| `:diff-files HEAD^..HEAD`, `:diff-files HEAD^!` | Lists only `+ untracked u.txt` | Grammar not shared with `:diff-commit`; silent |

- `REF!` conflicts with Helix's own meaning of a bang, which is force (`:w!`, `:q!`), and sits one character from git's `^!` — [view/editor.rs:1217-1225](helix-view/src/editor.rs#L1217).
- The ref resolver handles only a trailing `^`, then literal ref names, then hex prefixes — [git.rs:570-614](helix-vcs/src/git.rs#L570).

**Keys and modes**
- `Space-m` is not sticky, while `Space-G` and `Z` are, so each conflict costs six keystrokes (`Space m n Space m o`) — [default.rs:239,253,327](helix-term/src/keymap/default.rs#L239).
- Next/previous conflict uses `Space-m n/p`, not the `]`/`[` family that hunks use (`]g`, `[g`) — [default.rs:114,129](helix-term/src/keymap/default.rs#L114).
- `v` and `s` are vertical and horizontal split in window mode, but `s` means sync-scroll here — [book/src/keymap.md:269-270](book/src/keymap.md#L269), [default.rs:246-247](helix-term/src/keymap/default.rs#L246).
- `c`/`C` differ in semantics, not in the "scope widens" sense `f`/`F`, `s`/`S`, `d`/`D` use elsewhere in Space mode — [default.rs:226-235](helix-term/src/keymap/default.rs#L226).

**Picker and selection conventions**
- `_action` is ignored in `Space-g`, `:diff-files` and `:diff-buffer`, against the documented `Ctrl-s`, `Ctrl-v`, `Alt-Enter` — [commands/diff.rs:186](helix-term/src/commands/diff.rs#L186), [typed_diff.rs:153](helix-term/src/commands/typed_diff.rs#L153), [book/src/keymap.md:498-500](book/src/keymap.md#L498).
- `[live]` `Ctrl-v` opened in the same pane. `Alt-Enter` replaced the buffer behind a still-open picker rather than loading it in the background.
- `Space-m c`/`C` respect select-then-act, but read only the primary selection, so a two-cursor selection of two commits silently becomes a one-commit diff `[code]` — [commands/diff.rs:938,1001](helix-term/src/commands/diff.rs#L938).
- `Space-m o/t/b` use only the primary cursor, so a multi-cursor user cannot accept several conflicts together `[code]` — [commands/diff.rs:487-496](helix-term/src/commands/diff.rs#L487).

**Notation**
- Four spellings of the same key: `Space-g` (book), `space+g` ([typed_diff.rs:221](helix-term/src/commands/typed_diff.rs#L221), [typed.rs:3954](helix-term/src/commands/typed.rs#L3954)), `space m C` ([commands/diff.rs:966](helix-term/src/commands/diff.rs#L966)), `space g` (comments only).
- Three status prefixes for one concept: `Diff base set to:`, `Diff range set to:`, `Diff set:` — [typed_diff.rs:35,221](helix-term/src/commands/typed_diff.rs#L35), [commands/diff.rs:971](helix-term/src/commands/diff.rs#L971).

### Inferences
- `Space-m` as a menu is reasonable, but a sticky merge sub-mode, or `]m`/`[m`-style conflict navigation, would suit how often these keys repeat.
- Git users will type `~N`, `...` and `^!` from muscle memory. `revision` is not among the `gix` features listed at [helix-vcs/Cargo.toml:25](helix-vcs/Cargo.toml#L25), so adopting full rev-parse looks like the natural fix. At minimum, reject what cannot be resolved.
- Renaming the status family to one form, and naming `Space-m C` "Set range to selected commit", would be cheap wins.

### Gaps
- Whether `gix` rev-parse would cover every form was not tested, and whether `revision` is enabled transitively was not established. A scratch crate for this was dropped in favour of live `hx` testing.

## Can a user find these features without reading the book?

### Takeaway
Partly. A user who already knows `Space-g` can stumble into diff and merge views and then learn the actions from the `Space-m` infobox. Anything beyond that is hard to find: range selection needs prior knowledge, ref arguments have no completion, and neither the infobox nor the entry moment explains how to get started.

### Cited Findings

**Space infobox and `Space-m`**
- `[live]` `Space` shows `g    Open changed file picker` and `m    Merge/Diff`. The word "diff" is not attached to `g`, and the `g` label does not reveal that Enter now opens a diff.
- `[live]` The `Space-m` box lists 12 static entries, all always visible regardless of context, merge entries first.
- `Space-m` omits both ways in: `Space-g` and `:merge`. It also omits every `:diff-*` command and `:toggle-char-diff` — [default.rs:239-252](helix-term/src/keymap/default.rs#L239).
- Only `Space-m r` is meaningful everywhere. `v`, `s`, `q` need a view; `o`, `t`, `b`, `n`, `p` need markers; `c`, `C` need a commit selection; `f` needs an unconflicted file — [commands/diff.rs:367-1019](helix-term/src/commands/diff.rs#L367).
- Infobox rendering has no conditional or grouping support: it takes each key's doc and orders by insertion — [helix-term/src/keymap.rs:75-110](helix-term/src/keymap.rs#L75).

**`:` completion**
- `[live]` `:diff` lists `diff-base`, `diff-buffer`, `diff-commit`, `diff-reset`, `diff-files`, `reset-diff-change`, `toggle-char-diff`. `:merge` is absent, so the merge command needs a separate guess.
- `[live]` Doc text appears once the name is typed, with aliases, in a floating box.
- `:diff-commit` has the best doc in the set, listing its three forms. `:diff-files` says "specified commit" and `:diff-buffer` does not mention an argument at all — [typed.rs:3943,3954,3976](helix-term/src/commands/typed.rs#L3943).
- No positional placeholders are possible: the prompt doc prints only doc, aliases and flags — [typed.rs:4069-4140](helix-term/src/commands/typed.rs#L4069).

**Argument completion**
- None for refs: `:diff-base`, `:diff-commit`, `:diff-files` use `CommandCompleter::none()` — [typed.rs:3479,3956,3978](helix-term/src/commands/typed.rs#L3479). `[live]` Tab offers nothing.
- `:diff-buffer` completes numeric ids only, though it accepts names — [helix-term/src/ui/mod.rs:461-468](helix-term/src/ui/mod.rs#L461), `[live]` Tab offered `1`.
- `:copy-path` completes `url`, `absolute` — [helix-term/src/ui/mod.rs:472-477](helix-term/src/ui/mod.rs#L472).

**Command palette** (`Space-?`)
- `[live]` "diff" returns 14 of 425 commands with key bindings and docs, but **not** `changed_file_picker`, the primary entry point, because neither its name nor its doc says "diff" — [commands.rs:418](helix-term/src/commands.rs#L418).
- `[live]` "merge" returns 11 results, led by the unrelated `merge_selections`. `:merge` shows with no binding.
- Arg-requiring commands fail from the palette, because it runs them with empty args `[code]`. This is inherited Helix behaviour — [commands.rs:254-265](helix-term/src/commands.rs#L254).

**At the moment of entry**
- `[live]` After `Space-g Enter` the only message is the stale `Loading changes for: HEAD`. The same is true for merge view.
- `[live]` Merge view opens with the cursor at 1:1, and `Space-m o` then answers `No conflict at cursor`. The user must first discover `Space-m n`.
- `[live]` A clean repo shows `0/0` and the column headers, with no "no changes" text and still `Loading changes for: HEAD`.
- `[live]` The picker has no title or range indicator; the range appears only in that transient status.
- The picker preview always shows the working-tree file, even for a `REF1..REF2` range — [commands/diff.rs:206](helix-term/src/commands/diff.rs#L206). `[live]` Range `HEAD^^..HEAD` previewed the current `a.txt`.

**Review loop**
- There is no next-changed-file key. Reviewing N files means reopening the picker each time. `last_changed_file_selection` is written but never read — [commands/diff.rs:187](helix-term/src/commands/diff.rs#L187), [helix-view/src/diff_view.rs:18](helix-view/src/diff_view.rs#L18).

**The `Space-m c` path**
- No built-in source of git log text exists: a search found no log, commit or ref picker in `helix-term/src/commands/`. The user has to know to run something like `:insert-output git log --oneline` in a scratch buffer.

### Inferences
- A small status on entry, for example `Diff: a.txt vs HEAD (Space-m for actions)` and `Merge: 2 conflicts (Space-m n to start)`, plus jumping to the first conflict, would likely remove the largest learnability gap at very low cost.
- Adding the word "diff" to `changed_file_picker`'s doc string would fix both the infobox and the palette.
- Ref completion would both aid discovery and prevent F2's typos, though it does not by itself cover `~N`.

### Gaps
- Palette execution of arg-requiring commands was not run live.
- Whether `Space-'` reliably restores the changed-file picker with its position was not tested, though it could partly mitigate the review-loop gap.

## Are error and status messages actionable?

### Takeaway
Quality is split sharply. Messages raised directly by merge and in-view commands are mostly good. Messages on the setup path (ranges, picker, repo detection) are absent or falsely positive, which is the worst place for it, since that is where a new user makes mistakes. Entry-level failures often appear as no message at all.

### Cited Findings

All strings below are `[live]` captures unless marked. Prefixes like `'merge':` come from the typable command runner.

**Not a git repo**

| Trigger | Actual output | Judgement |
|---|---|---|
| `Space-g` | Picker `0/0`; `Loading changes for: HEAD` | **Bad.** Silent, and a regression: master reported through [helix-vcs/src/lib.rs:81-84](helix-vcs/src/lib.rs#L81), whose caller was removed |
| `:diff-files HEAD` | Picker `0/0`, no message | **Bad** |
| `:diff-commit HEAD` | `Diff range set to: HEAD — use space+g to browse files` | **Bad.** False success |
| `:diff-base HEAD`, `:diff-base` | `failed to set diff base: failed to open git repo` | Acceptable; "not inside a git repository" would be clearer |
| `:merge`, file has markers | `'merge': Failed to fetch merge versions from git index` | **Bad.** Cause hidden |
| `:merge`, no markers | `'merge': No conflict markers found in file` | Good |
| `Space-m f` | `File saved but git add failed: exit status: 128` | Partial: honest, but a raw exit code |

**No changes**

| Trigger | Actual output | Judgement |
|---|---|---|
| `Space-g`, clean repo | Picker `0/0`; stale `Loading changes for: HEAD` | Poor: indistinguishable from the failures above |
| `Space-m c` on HEAD | `Diff set: 31acd5a (HEAD) vs working tree: uncommitted changes only, space m C shows the commit` | **Excellent** |
| `:merge`, resolved file | `'merge': No conflict markers found in file` | Good |
| `Space-m n` with none | `No conflicts remaining` | Good in a merge; odd in a buffer that never had any |

**Bad ref**

| Trigger | Actual output | Judgement |
|---|---|---|
| `:diff-base nosuchref` | `failed to set diff base: 'nosuchref' is not a valid reference or commit hash` | Good |
| `:diff-base deadbeef` | `failed to set diff base: no commit found with hash 'deadbeef'` | Good |
| `:diff-base HEAD~1` | `failed to set diff base: 'HEAD~1' is not a valid reference or commit hash` | **Bad.** Untrue, and the command's own doc promises it |
| `:diff-commit nosuchref` | `Diff range set to: nosuchref — use space+g to browse files` | **Bad.** False success |
| ...then `Space-g` | Untracked files only; `Loading changes for: nosuchref` | **Bad.** Misleading list, no error |
| ...then Enter on an entry | `Failed to open diff view: Failed to fetch 'nosuchref': 'nosuchref' is not a valid reference or commit hash` | First honest error, two steps late |
| `:diff-files nosuchref` | Untracked files only, no message | **Bad** |
| `:diff-commit ..HEAD` | `'diff-commit': failed to parse diff range: ..HEAD` | Partial: drops the inner reason "both sides must be non-empty" ([view/editor.rs:1209](helix-view/src/editor.rs#L1209)) |
| `:diff-commit`, no arg | `'diff-commit': expected exactly 1 argument, got 0` | Partial: generic. The friendlier `missing git reference argument` at [typed_diff.rs:214](helix-term/src/commands/typed_diff.rs#L214) is unreachable |

**File not tracked**

| Trigger | Actual output | Judgement |
|---|---|---|
| `:diff-base HEAD^` on untracked `u.txt` | `failed to set diff base: file is untracked` | Good |
| `:diff-base` on a **tracked** file absent at REF | `failed to set diff base: file is untracked` | **Misleading**; it is tracked, just not at that ref |
| `:diff-base` (reset) on untracked file | `failed to set diff base: file is untracked` | Partial: a reset that "fails" |

**Nothing to toggle, wrong context**

| Trigger | Actual output | Judgement |
|---|---|---|
| `Space-m v`, `s`, plain buffer | `Not in a diff view` | Good |
| `Space-m v`, `s`, in merge view | `Not in a diff view` | Partial: true but surprising, since no explanation that merge differs |
| `Space-m q`, plain buffer | `Not in a diff or merge view` | Good |
| `Space-m o`, `t`, `b`, no conflict | `No conflict at cursor` (info level) | Partial: right after entering a merge it should point to `Space-m n` |
| `Space-m f`, conflicts remain | `1 conflict(s) still unresolved` | Good; could name `Space-m n` |
| `Space-m f`, plain buffer | `File saved and staged (git add)` | **Bad.** Should be refused |
| `Space-m f`, scratch | `Failed to save: Can't save with no path set!` | Good |
| `Space-m r`, in merge view | `Diff reset to HEAD` (merge stays) | Poor: claims work on something unrelated |
| `Space-m c`, prose, default selection, `commit <hash>`, `* <hash>` | `Invalid commit hash in selection` | Partial: omits the expected format |
| `Space-m C`, several lines | `Select exactly one commit line` | Good |
| `Space-m c`, whitespace-only selection | `No selection` `[code]` | Misleading label |
| `:diff-base HEAD^`, scratch | `Diff base set to: HEAD^` | **Bad.** False success ([document.rs:2084-2091](helix-view/src/document.rs#L2084) returns Ok when there is no path) |
| `:diff-base`, scratch | `Diff base reset to HEAD` | **Bad.** False success |
| `:toggle-char-diff`, scratch | `Character-level diff highlighting enabled` | Poor: nothing to highlight |
| `:diff-buffer`, one buffer | `No other buffer to diff` | Good |
| `:diff-buffer <own id or name>` | `'diff-buffer': No matching buffer: 1` | Partial: `Cannot diff a buffer against itself` exists but self is filtered out first ([typed_diff.rs:90](helix-term/src/commands/typed_diff.rs#L90)) |
| `:merge`, scratch | `'merge': Current buffer has no file path` | Good |
| `:merge`, markers but no merge in progress | `'merge': Failed to fetch merge versions from git index` | **Bad.** The helpful `OURS (stage 2) not in index — is there an active merge?` ([git.rs:246-248](helix-vcs/src/git.rs#L246)) is discarded |
| `:copy-path bogus` | `'copy-path': Unknown argument 'bogus', expected 'url'` | Partial: omits `absolute` |
| `:copy-path url`, working-tree pane | `Copied path to clipboard: ...` | Partial: asked for a URL, silently got a path |
| `:copy-path`, scratch | `'copy-path': No file path available (scratch buffer)` | Good |

**Success-path strings**
- `Split view on`, `Split view off`, `Sync scroll enabled`, `Sync scroll disabled`, `Conflict 1/2`, `Accepted HEAD version`: all good — [commands/diff.rs](helix-term/src/commands/diff.rs).
- `Switched to both version`: grammar slip from sharing a label — [commands/diff.rs:390-396,472-473](helix-term/src/commands/diff.rs#L390).
- `Diffing buffer 2 against current buffer`: uses an id rather than a name — [typed_diff.rs:198-200](helix-term/src/commands/typed_diff.rs#L198).
- `Diff set: showing changes in 3ee09bc` and `Diff set: 3ee09bc vs working tree`: unlike `:diff-commit`, neither says to press `Space-g`, even though nothing has been shown — [commands/diff.rs:971,1018](helix-term/src/commands/diff.rs#L971).

**Causes**
- Errors are discarded with `let _ =` and `if let Ok(change)` in both listing paths — [commands/diff.rs:39-44,49,247-258,265](helix-term/src/commands/diff.rs#L39).
- `.map_err(|e| anyhow!("{e}"))` keeps only the outermost context, losing the useful inner cause — [typed_diff.rs:242](helix-term/src/commands/typed_diff.rs#L242).

### Inferences
- Priority order for fixes: report listing errors in the picker or status; validate refs at set time; guard `Space-m f`; print the full error chain with `{e:#}`; clear or replace `Loading changes for:` on completion.
- Because the bad-ref picker still lists untracked files, it looks like a working result. That is worse than an empty list.

### Gaps
- Message level (info vs warning vs error) was confirmed in code but not visually compared. `[live]` captures are colourless.
- Windows-specific failure strings were not tested.

## Do the book and generated docs accurately and fully describe behaviour?

### Takeaway
The key tables are accurate and well-structured, and static docs are fully in sync. But the prose has several claims that are simply false in the running editor, the single most important config option is absent, and the merge view's layout is not described anywhere.

### Cited Findings

**Concrete mismatches**

| # | Severity | Doc says | Reality |
|---|---|---|---|
| D1 | High | `:diff-base` accepts "branch, tag, SHA, or HEAD~N" ([typed.rs:3477](helix-term/src/commands/typed.rs#L3477), [typable-cmd.md](book/src/generated/typable-cmd.md)) | `HEAD~N` is rejected `[live]`. Only trailing `^` works |
| D2 | High | Snapshots are "named `filename @ ref`", e.g. `foo.py @ abc123` ([diff-and-merge.md:69,74](book/src/diff-and-merge.md#L69), [pickers.md:21](book/src/pickers.md#L21)) | `[live]` Never shown: `Space-b` shows `…e/helix/git-rev/<hash>/a.txt`, the statusline a cache path, the bufferline bare `a.txt`. All 9 `display_name()` call sites are error or log text, the `%` register, or `:diff-buffer` name matching ([definition](helix-view/src/document.rs#L2239)) |
| D3 | High | Merge section reads as merge-only ([diff-and-merge.md:82-95](book/src/diff-and-merge.md#L82)) | `Space-m f` stages any file; `o`, `t`, `b`, `n`, `p` work on any buffer with markers `[live]` |
| D4 | Medium | `:diff-files` shows files "between `REF` and the current branch" ([diff-and-merge.md:40-41](book/src/diff-and-merge.md#L40), [pickers.md:36-37](book/src/pickers.md#L36), [typed.rs:3976](helix-term/src/commands/typed.rs#L3976)) | It is REF vs **working tree**, including uncommitted and untracked: `[live]` `:diff-files v1` listed `u.txt`. The source comment admits this ([typed_diff.rs:308-312](helix-term/src/commands/typed_diff.rs#L308)) |
| D5 | Medium | `:diff-reset` "clears any custom diff range and restores the default `HEAD` comparison" ([diff-and-merge.md:13-14](book/src/diff-and-merge.md#L13)); "Reset the diff base to HEAD (default behavior)" ([typed.rs:3965](helix-term/src/commands/typed.rs#L3965)) | Also closes the current diff view and disables char-diff on all buffers `[live]`, [typed_diff.rs:254-273](helix-term/src/commands/typed_diff.rs#L254). Does not close merge views or clear the split override |
| D6 | Medium | [pickers.md:16](book/src/pickers.md#L16) says the default listing includes "renamed" | Rename tracking is disabled on purpose ([git.rs:697-707](helix-vcs/src/git.rs#L697)). `[live]` `git mv` shows `~ modified new.txt` and `- deleted old.txt`, and Enter shows the whole file as added |
| D7 | Medium | The picker key table ([keymap.md:498-500](book/src/keymap.md#L498)) presents `Alt-Enter`, `Ctrl-s`, `Ctrl-v` as working in all pickers | Ignored in all three new pickers `[live]` |
| D8 | Low | "If opening a comparison or toggling its split fails, the current diff remains available" ([diff-and-merge.md:63-64](book/src/diff-and-merge.md#L63)) | `:merge` closes the current diff before validating ([view/editor.rs:3630-3635](helix-view/src/editor.rs#L3630)), so a failed `:merge` loses it `[code]` |
| D9 | Low | Code comments say OURS and THEIRS on top, RESULT below ([view/editor.rs:3611-3614](helix-view/src/editor.rs#L3615), `MergeViewState` doc) | `[live]` RESULT is on **top**, full width; OURS and THEIRS are below. Not user-facing, but it will mislead whoever writes the missing docs |
| D10 | Low | [typable-cmd.md](book/src/generated/typable-cmd.md) lists `:markdown-preview`, `:md-preview`, `:mdp` | No such command exists: the only repo hit is that file. Added by commit `34cf2cdeb` |
| D11 | Low | `Space-g` is "Open the changed-file diff picker" ([keymap.md:297](book/src/keymap.md#L297)) | Infobox says "Open changed file picker" ([commands.rs:418](helix-term/src/commands.rs#L418)) |

**Gaps in coverage**
- **`[editor.diff] split-view` is not documented anywhere**: no hits for `split-view` or `editor.diff` in `book/src/*.md`, while [editor.md:309](book/src/editor.md#L309) documents the more niche `[editor.git-remote]`.
- Merge view layout, which pane is which, and that RESULT is the only editable one: not in the book.
- That entering split or merge closes other panes: not in the book.
- That `Space-m v` persists for the session, overrides config, and survives `:diff-reset`: not in the book.
- That the default single-pane view does not show removed lines: not in the book, although [diff-and-merge.md:23-24](book/src/diff-and-merge.md#L23) calls it a "single-pane diff view".
- `Space-m f` leaves the view open, needing `Space-m q`. Close is documented only under "Split diff controls" ([diff-and-merge.md:54](book/src/diff-and-merge.md#L54)), not in the Merge section.
- Where "selected git log lines" come from, and the required format: `--oneline`-style with the hash as first token. `[live]` Default `git log` (`commit <hash>`) and `--graph` (`* <hash>`) lines both fail.
- Pre-selection of the current file in `Space-g`, the sticky nature of the range, and the fact that it is cwd-based.
- `:reset-diff-change` (`:diffget`), the hunk-revert command, is not mentioned on the page.
- `:copy-path url` and `[editor.git-remote]` are not linked from the diff page, and the silent working-tree fallback is undocumented ([typed.rs:656-676](helix-term/src/commands/typed.rs#L656)).
- `ui.diff.spacer` is absent from [themes.md](book/src/themes.md), which lists the `diff` scopes at lines 266-274.
- The `version-control` element's `@ref` suffix is not described at [editor.md:153](book/src/editor.md#L153).
- The Space mode table has no `m` row, though `G` and `w` do; the section exists below it — [keymap.md:293-317](book/src/keymap.md#L293).

**Verified accurate**
- Keymap table: 12 of 12 keys match `default.rs`.
- Generated static docs: a scripted comparison of [commands.rs](helix-term/src/commands.rs) with `static-cmd.md` found 325 of 325 commands in sync, with one pre-existing escaping difference unrelated to this work.
- `:diff-buffer` argument forms and picker ([diff-and-merge.md:18-20](book/src/diff-and-merge.md#L18)).
- `Space-m c`/`C` semantics, including the HEAD warning ([diff-and-merge.md:34-39](book/src/diff-and-merge.md#L34)) `[live]`.
- Conflict markers blocking `Space-m f` ([diff-and-merge.md:94-95](book/src/diff-and-merge.md#L94)) `[live]`.
- Conflicted entries opening merge view `[live]`.

### Inferences
- D2 suggests the display name was implemented on `Document` but never connected to the three places users see buffer names. Wiring `display_name()` into the statusline, buffer picker and bufferline would fix a High UX issue and make the docs true at once.
- D10 would be removed by regenerating the docs, and if CI compares generated docs, it would currently fail.

### Gaps
- `cargo xtask docgen` was not run, because the task is read-only for the repo. The typable comparison used a script whose flags on `pipe` and `line-ending` were parser artefacts (multi-line definitions), so those two are not findings.
- `> renamed` may still be reachable in tree-to-tree ranges ([git.rs:751,795](helix-vcs/src/git.rs#L751)). This was not tested.
- Whether upstream CI enforces docgen on this branch was not checked.

## Which config options exist, are defaults sensible, and what cannot be configured or rebound?

### Takeaway
Configuration is minimal, which is good, but the one default that shapes first impressions is the wrong way round: `split-view = false` hides the old text. All 12 commands can be rebound, but the behaviours people are most likely to want changed (keeping `Space-g` as file-open, not closing panes, a sticky merge menu) cannot be.

### Cited Findings

**Options and defaults**

| Option | Default | Assessment |
|---|---|---|
| `editor.diff.split-view` | `false` — [view/editor.rs:522-526](helix-view/src/editor.rs#L522) | **Questionable.** Undocumented, and see below |
| `editor.git-remote.url-template` | per host | Sensible: GitHub, GitLab, Bitbucket, Gitea, Azure recognised ([editor.md:309-336](book/src/editor.md#L309)) |
| `editor.git-remote.line-template` | per host | Sensible |
| `editor.git-remote.line-range-template` | per host | Sensible |

**Why `split-view = false` is a weak default**
- `[live]` In single-pane view, a changed line shows only the new text (`line twenty`) and a deleted line shows only a `▔` marker on the next line. Old content is unreachable without `Space-m v`.
- `[live]` The text layout matched a plain buffer of the same file after `:diff-reset`. Colour-only intraline highlighting is the remaining difference `[code]`, [helix-view/src/document.rs set_view_diff](helix-view/src/document.rs).
- For a range diff, where the pane shows the newer revision, the old side is similarly hidden `[live]`.
- `:diff-buffer` ignores this default and always splits — [typed_diff.rs:156,196](helix-term/src/commands/typed_diff.rs#L156). `[live]` it opened side by side. The book documents this ([diff-and-merge.md:23](book/src/diff-and-merge.md#L23)), but the inconsistency remains.

**Settings precedence**
- Precedence is: explicit argument, then session override, then config — [view/editor.rs:3084-3085](helix-view/src/editor.rs#L3084).
- `Space-m v` writes the session override with no way to clear it — [commands/diff.rs:866](helix-term/src/commands/diff.rs#L866). `[live]` with `split-view = true` in config, toggling off once made later diffs single-pane, even after `:diff-reset`. Restarting is the only reset.

**Visibility of state**
- `version-control` is not in the default statusline, so `branch@ref` is off by default — [view/editor.rs:628-648](helix-view/src/editor.rs#L628).
- That suffix reflects only `:diff-base`, never the `:diff-commit` range — [statusline.rs:545-549](helix-term/src/ui/statusline.rs#L545).

**Hard-coded, no option**
- Sync scroll default on — [helix-view/src/diff_view.rs:104](helix-view/src/diff_view.rs#L104).
- Intraline highlighting default on in diff views, and all its thresholds — [ui/diff.rs:13-22](helix-term/src/ui/diff.rs#L13).
- Merge layout, and diff split direction (always vertical) — [view/editor.rs:3039-3100](helix-view/src/editor.rs#L3039).
- Closing other panes on split and merge — [view/editor.rs:3010-3030](helix-view/src/editor.rs#L3010).
- `Space-g` opening a diff instead of a file: no opt-out. Existing users' muscle memory changes silently — [commands/diff.rs:186-204](helix-term/src/commands/diff.rs#L186).
- Inclusion of untracked files in the picker.

**Rebinding**
- All 12 static commands are normal `MappableCommand`s and can be rebound under `[keys.normal.space.m]` or elsewhere, and the typable commands can be bound as strings — [commands.rs:557-568](helix-term/src/commands.rs#L557).
- `sticky` is available only through the built-in macro, not user config — [helix-term/src/keymap/macros.rs:116](helix-term/src/keymap/macros.rs#L116); no deserialisation in [keymap.rs](helix-term/src/keymap.rs). So users cannot make `Space-m` sticky.
- Keymaps have no context conditions, so `o`/`t`/`b` cannot become single keys only inside a merge view.
- `:merge` has no static twin and no default key. It can be bound as a typable string.

**Theming**
- `ui.diff.spacer`: 0 of 195 bundled themes define it, so all rely on the computed fallback, or plain `Color::Gray` on non-RGB themes — [ui/diff.rs:52-73](helix-term/src/ui/diff.rs#L52).
- There is no dedicated scope for intraline highlights, merge panes, or conflict regions. Intraline reuses `diff.plus` and `diff.minus` and silently does nothing if the exact scope is missing — [ui/diff.rs:140-142](helix-term/src/ui/diff.rs#L140).
- A rough `grep` (any line naming those scopes and containing `bg`) found 15 lines across 195 bundled themes, so intraline highlighting is probably a foreground-colour change in most themes. This is an estimate, not a per-theme audit.

### Inferences
- Recommended defaults: make `split-view = true`, or render removed lines inline in the single-pane view. Keep a config option either way, and document it first.
- Clearing `split_view_override` in `diff_reset`, or making `Space-m v` per-diff, would remove the "config seems ignored" trap.
- A setting to preserve layout, or restoring the prior layout on close, would resolve F8 more durably than documentation.
- A config option is less useful for `Space-g` than an opening convention. For example, `Ctrl-v` and `Ctrl-s` could open the diff in a split, restoring the broken convention, and a separate action could open the plain file.

### Gaps
- The intraline visual result in single-pane view was not confirmed in colour: the colour-capture comparison was run against an unchanged file, so it produced no data.
- The practical appearance of the `ui.diff.spacer` fallback across themes was not assessed.
- `:set diff.split-view` after a `Space-m v` toggle was not separately run, though precedence is clear from code.
