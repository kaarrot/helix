# Helix diff and merge workflows: hands-on observations

Everything below was observed by driving a debug `hx` in tmux 3.2a on WSL2, branch `dev-3-way-diff-second-attempt`, HEAD `de51c5ef2`, git 2.34.1, default theme, empty config. Unless a bullet says otherwise, the terminal was 140x40.

**Rating scale (UX, out of 5):** 5 smooth, 4 works with minor friction, 3 usable but confusing, 2 misleading or easy to get wrong, 1 broken or dangerous.

**Citations.** `[capture NAME](path)` links to a saved `tmux capture-pane` file. `(live: session X)` means the quote was copied from `capture-pane` output during the run and was not saved as a file. 37 captures were saved; the rest of the evidence is the verbatim quotes here. Repository paths such as `helix-vcs/src/git.rs:583` are relative to the worktree root. Fixture scripts: [drv.sh](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/drv.sh), [mk_repo1.sh](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/mk_repo1.sh), [mk_merge.sh](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/mk_merge.sh).

**Setup caveats that affect how to read these results.**

- `hx --version` prints `helix 25.07.1 (e1be05fe)`, not HEAD. `e1be05fe` is an ancestor of HEAD, 7 commits back. The binary was built at 14:48 from sources dated 11:57, so the code under test is current and only the embedded hash is stale.
- Language servers were disabled for rust, python and markdown after rust-analyzer produced fixture noise (`Failed to discover workspace`). This was an artifact of a fixture with no `Cargo.toml`, not a product finding.
- `HOME` pointed at a scratchpad directory so snapshot paths fold to the realistic `~/.cache/helix/git-rev/...`. An earlier run with a long cache path exaggerated statusline overflow; those observations were discarded.
- `runtime/grammars` has one entry in this worktree. No finding here concerns syntax highlighting.
- My first session ran before the cache redirect and wrote one fixture snapshot to the real `~/.cache/helix/git-rev/`. I removed that single directory after confirming its timestamp and content; no other run wrote there.
- I read `book/src/diff-and-merge.md` and the keymap before driving the editor, so the hint judgements in question 8 are reconstructed, not blind.
- Discarded runs: one session where `hx` never started, so keys went nowhere; one run where my harness overwrote a variable so "accept" keys sent `n`; one `zdiff3` run that could not create a conflict. All were rerun or marked not tested. One fixture was contaminated by a stray `:w` and was rebuilt.

## 1. Changed-file review: picker, split view, hunks, sync, search, leaving

### Takeaway
Rating 3/5. The core loop is good: the picker, side-by-side alignment, hunk hopping and scroll sync (5/5) all work and stay fast. The friction is around it: opening a file looks like nothing happened, there is no next-file key, renames appear as whole-file add plus whole-file delete, and the read-only pane accepts typing and silently drops it.

### Cited Findings

**Picker**
- `Space-g` lists 11 entries for this fixture, with `~ modified`, `- deleted`, `+ untracked` markers and a `Loading changes for: HEAD` status line. — [capture s1__02-picker](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/s1__02-picker.txt)
- Row order changes between openings. First open is in arrival order (`deleted docs/gone.txt`, `modified docs/moved_clean.txt`, `deleted moved_clean.txt`...), with `src/main.py` and `blob.bin` swapping places across runs. The second open is grouped modified, deleted, untracked. — [capture s1__17-picker-again](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/s1__17-picker-again.txt); (live: session s1, restarted)
- Renames are not detected. `git status` shows `R  renamed_old.py -> renamed_new.py`, but the picker shows `~ modified  renamed_new.py` and `- deleted   renamed_old.py`. Opening `renamed_new.py` marks every line as added against an empty left pane (`~ ▔`), even though only one line changed. — (live: session s1); [capture s1__02-picker](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/s1__02-picker.txt)
- A file with both staged and unstaged changes (`MM`) gets one row. `]g` steps through both kinds of hunk with nothing to tell them apart (`10 ▍LINE 010 STAGED`, `50 ▍LINE 050 UNSTAGED`). — (live: session s1)
- Previews show the plain file, not a diff. Deleted files show `<File not found>` and binary files `<Binary file>`. — [capture s1__03-picker-deleted-preview](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/s1__03-picker-deleted-preview.txt)
- Opening a deleted entry gives old content on the left and `[scratch]  [readonly]` on the right, a clear enough presentation. — (live: session s1)
- Opening `blob.bin` despite the `<Binary file>` preview shows a garbled text diff tagged `windows-1252`, with no warning. — (live: session s1)
- The status line `Loading changes for: HEAD` stays on screen after the picker closes, until something replaces it. — [capture s1__04-open-main](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/s1__04-open-main.txt)

**Entering a diff**
- Selecting a modified file opens the single-pane form, which is visually the same as normal editing: same statusline, gutter bars only. The screens before and after are identical. — [capture s1__00-opened](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/s1__00-opened.txt); [capture s1__04-open-main](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/s1__04-open-main.txt)
- `Space-m v` gives `Split view on` with padded alignment and a `▔` marker on the far side of an insertion. — [capture s1__07-split-on](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/s1__07-split-on.txt)

```text
  117                                                                 │  117
                                                                      │  118 ▍# new comment above func_30
                                                                      │  119 ▍# and a second line
  118 ▔def func_30():                                                 │  120  def func_30():
```

- The left pane is labelled with an internal path, `~/.cache/helix/git-rev/5514ae60eedd4174/main.py`, and its `[readonly]` tag is clipped to `[r`. Nothing says "HEAD". — [capture s1__07-split-on](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/s1__07-split-on.txt); (live: session s1)

**Moving around**
- `]g` lands on each hunk, including deletions, in both panes. `[g` works backward in the same way. At the last or first hunk both stop silently, with no wrap or message. — [capture s1__11-split-hunk-del](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/s1__11-split-hunk-del.txt); [capture s1__12-split-hunk-last](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/s1__12-split-hunk-last.txt); (live: session v1)
- Scroll sync held for `C-d`, `C-f`, `j` past the viewport edge, `ge`, search, and the mouse wheel, including the correct offset past a hunk (`127 ... │ 129 ...`). — (live: sessions s1, a2, v1)
- `Space-m s` reports `Sync scroll disabled` and panes then diverge (left at 1, right at 34). Re-enabling reports `Sync scroll enabled` and pulls the left pane back into line. — (live: session s1)
- `/func_40` search keeps both panes aligned. — (live: session s1)
- The left pane's cursor does not follow normal movement: its statusline stayed at `232:1` while the right moved to `1:1`. Only hunk jumps move it. — (live: session s1)
- Scrolling from the focused left pane moves both, but switching focus back with `C-w l` snaps the view (top line 237 back to 227). — (live: session s1)
- Character-level diff is on by default: `3333` gets a background in `return 33333`. `:toggle-char-diff` reports `Character-level diff highlighting disabled` and the background goes away. — [capture s1__15-chardiff.ansi](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/s1__15-chardiff.ansi); [capture s1__16-chardiff-off.ansi](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/s1__16-chardiff-off.ansi)
- Live edits in the right pane update the diff immediately, and undo removes them. — [capture s1__14-right-edit](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/s1__14-right-edit.txt)

**Read-only surprise**
- Pressing `i` in the left snapshot pane enters `INS` mode with no message. Typed `XX` vanishes and `d` also does nothing. The snapshot stays intact, but no warning appears. — (live: session s1)

**Stepping and leaving**
- There is no next-file key. The observed path is `Space-g`, arrow, `Enter`, each time. `Space-'` reopens the picker with the old filter still in it. — (live: session s1)
- The split setting carries between files, and `Space-g` back to an earlier file restores its split. — (live: session s1)
- `Space-m q` returns to normal single-pane editing. A second press reports `Not in a diff or merge view`. — [capture s1__19-closed-diff](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/s1__19-closed-diff.txt); (live: session s1)
- Visiting nine files left 18 buffers: each file, its snapshot, and three `[scratch]` buffers from deleted entries. They were still listed after `:diff-reset`, which the book says forgets suspended comparisons. — (live: session s1); `book/src/diff-and-merge.md:58-59`

**Scale (debug build)**
- A 200,000-line file with 50 hunks: picker populated in about 0.5s, `Split view on` in about 0.1s, and three navigation keys took about 0.3s beyond my harness delays. A repository with 1,800 changed files reached `1800/1800` in about 0.5s. Timing used 0.1s polling, so treat these as rough. — (live: sessions b1, b2)

### Inferences
- Because single-pane mode looks like ordinary editing, a user who presses `Enter` in the picker has no signal that a diff opened and may conclude that it did nothing. A default of split view, as buffer diffs already use, or any statusline indicator, would likely remove that uncertainty.
- Reviewing a rename-heavy change is noisy enough that a user would likely fall back to `git diff -M`.
- Buffer-list growth may matter in long reviews, since snapshots and scratch buffers outnumber real files about 2:1.

### Gaps
- Mouse click to change focus was not tested. Only the wheel was, via injected SGR sequences.
- Inline deleted-line display in single-pane mode was not seen and appears absent, but I did not check every option.

## 2. Diffing against refs and commits: :diff-base, :diff-commit, :diff-files, :diff-reset

### Takeaway
Rating 2/5. `^` forms, tags, branches and hashes work, but every `~N` form fails, which is the most common way to write a relative commit. `:diff-base` at least reports an error. `:diff-commit` and `:diff-files` accept any string and then show a misleading or empty picker with no error at all.

### Cited Findings

**`~N` revisions fail, and the command's own help promises them**
- With a five-commit history where `git rev-parse HEAD~3` works, results of `:diff-base` were:

| Argument | Result |
| --- | --- |
| `HEAD^`, `HEAD^^`, `v1`, `feature`, `main`, short hash, full hash | `Diff base set to: ...` |
| `HEAD~1`, `HEAD~2`, `HEAD~3`, `main~1` | `failed to set diff base: 'HEAD~1' is not a valid reference or commit hash` |
| `HEAD@{1}`, `v1^0` | same error |

  — (live: session s2)
- The `:diff-base` help popup reads `Set diff base to a specific commit (branch, tag, SHA, or HEAD~N). No arguments resets to HEAD.` — (live: session q2)
- The rejecting path tries `find_reference`, then a hex prefix, which fits the observed behaviour. — `helix-vcs/src/git.rs:583-594`

**Bad input to the range commands is accepted silently**
- `:diff-commit nosuchref` prints `Diff range set to: nosuchref — use space+g to browse files`. `Space-g` then shows one row, `+ untracked notes_untracked.md`, with no error. — [capture s2__20-picker-badrange](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/s2__20-picker-badrange.txt)
- `:diff-commit HEAD..nosuchref`, `nope..HEAD` and `nosuchref!` are accepted the same way. `HEAD..nosuchref` gives an empty picker. — (live: session s2)
- Valid `~N` ranges behave identically to garbage. `:diff-commit HEAD~2` gives only the untracked file, while `HEAD~2!` and `HEAD~3..HEAD~1` give an empty picker, although `git diff --name-only HEAD~3..HEAD~1` lists `README.md` and `src/main.py`. — [capture s2__21-range-picker-bang](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/s2__21-range-picker-bang.txt); (live: session s2)
- `:diff-commit main...feature` (three dots) gives an empty picker with no error. — (live: session s2)
- A bad range persists: `Space-g` stays wrong until `:diff-reset`. — (live: session s2)
- `:diff-files nosuchref` opens a picker with only the untracked file and prints nothing. — (live: session s2)
- Argument errors are clear: `'diff-commit': expected exactly 1 argument, got 0` and `'diff-files': expected exactly 1 argument, got 0`. — (live: session s2)
- `Enter` on an empty picker does nothing. In my first run this meant the following `i X` and `:w` edited the real `src/main.py` while I believed I was in a read-only snapshot. — (live: session s2)

**Working forms**
- `:diff-commit HEAD^!` reports `Diff range set to: HEAD^^..HEAD^`, and the picker correctly shows only `README.md`. — (live: session s2)
- `:diff-commit HEAD^^^..HEAD^` lists `README.md` and `src/main.py`. `:diff-commit v1..feature` works. — (live: session s2)
- `:diff-commit <hash>` compares against the working tree and opens the working file. — (live: session s2)
- A file that exists only on the newer side, `feature.txt` in `v1..feature`, is labelled `~ modified`, not added. — (live: session s2)
- `:diff-files feature` also lists working-tree changes and the untracked file, so it is wider than the book's "between `REF` and the current branch". It does not change later `Space-g` results. — (live: session s2); `book/src/diff-and-merge.md:40-41`

**Snapshots**
- Range diffs open read-only snapshots displayed as `~/.cache/helix/git-rev/31179cc5bd9b0015/README.md  [readonly]`, in both panes and the buffer list. The book says they are named `filename @ ref`. Neither pane shows which commit it is. — (live: session s2); `book/src/diff-and-merge.md:69`

```text
       ~/.cache/helix/git-rev/ef6d4cabdc67272f/README.md   1 sel  4:1   NOR   ~/.cache/helix/git-rev/31179cc5bd9b0015/README.md  1 sel  4:1
```

- `i` in a snapshot enters `INS` silently and drops input. `:w` reports `'write': Cannot save a git revision document`, which is clear. — (live: session s2)
- `:open src/util.rs` from a range diff opens the real file and leaves both snapshots in the buffer list. `Space-m q` then reports `Not in a diff or merge view`, so the documented way to clean them up no longer applies. — (live: session s2)

**`:diff-base` is not a "diff view"**
- After `:diff-base HEAD^` the gutter updates and `]g` works, but `Space-m v` reports `Not in a diff view`, so there is no way to get side-by-side from this command. — (live: session s2)
- **False success in split view.** Run inside an open split, `:diff-base HEAD^` reports `Diff base set to: HEAD^`, but nothing changes. The left snapshot path stays the same, and `return 4000` appears on both sides with no hunk mark, although `HEAD^` has `return 40` there. — (live: session v1)

```text
  160      return 4000                                                │  162      return 4000
```

**Non-repository directory**
- `:diff-base HEAD` and `:diff-base` report `failed to set diff base: failed to open git repo`, which is clear. — (live: session s3)
- `Space-g` opens an empty picker with no explanation. `:diff-commit HEAD^` still reports `Diff range set to: HEAD^ — use space+g to browse files`. `:diff-reset` reports `Diff reset to HEAD`. — (live: session s3)
- From outside a repository, with an absolute path to a file inside one, gutter diff and `:diff-base HEAD^` work, but `Space-g` is empty with no message. The picker appears to follow the working directory rather than the file. — (live: session d2)
- `Space-m f` in a non-repo lets the child process write directly onto the TUI, corrupting the top row until redraw. Status: `File saved but git add failed: exit status: 128`. — [capture s3__30-diffbuf](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/s3__30-diffbuf.txt)

```text
    1  aatal: not a git repository (or any of the parent directories):│.g  1  alpha
```

**Other locations**
- Running from a subdirectory works. Files outside the working directory appear as long absolute paths with a leading `…`. — (live: session d1)
- A linked git worktree works normally. — (live: session d3 worktree run)

### Inferences
- The combination of `~N` failure and silent acceptance is the worst part of this scenario. A user who types `:diff-commit HEAD~3..HEAD` sees a success message and then an empty picker, and would reasonably conclude that those commits changed nothing.
- `:diff-base` and `:diff-commit` differ in validation, so the same typo gives an error from one and a false success from the other. `:diff-base` inside a split adds a third variant: a valid ref that reports success and has no effect.

### Gaps
- Whether `~N` fails in the same way for every ref source was covered only for `HEAD` and `main`. Code-level confirmation belongs to the code readers.
- Comparing the same buffer against two revisions in separate views (`book/src/diff-and-merge.md:61-62`) was tried once and the result was inconclusive. I did not establish it either way.

## 3. Range from selected git log lines (Space-m c, Space-m C)

### Takeaway
Rating 4/5 once a log is in a buffer, with the clearest messages of any feature tested. Discoverability is 2/5: Helix has no log view, so the user has to think of `:insert-output git log --oneline` in a scratch buffer, and only bare `--oneline` lines parse.

### Cited Findings
- Setup used `:new`, then `:insert-output git log --oneline`, then `x` to select lines. — (live: session s4)
- One line (c3) selects `95a9cfd` versus the working tree, and the picker lists changed files. — (live: session s4)
- Three lines report `Diff set: 95a9cfd..ed5fa5c`, and the picker lists `README.md`, `src/main.py`, `src/util.rs`, correctly leaving out the bottom line's own changes. — (live: session s4)
- On `HEAD`: `Diff set: ed5fa5c (HEAD) vs working tree: uncommitted changes only, space m C shows the commit`. This is the best hint in the feature set. — (live: session s4)
- `Space-m C` on one line reports `Diff set: showing changes in f8fa577`, and on two lines `Select exactly one commit line`. — (live: session s4)
- A cursor on a line without `x` selects one character and gives `Invalid commit hash in selection`, which does not suggest what to do. — (live: session s4)
- An empty line selected with `x` reports `No selection`, which is inaccurate. — (live: session s4)
- `git log --graph --decorate --oneline` lines such as `* 64410cd (feature) f1: feature work` are rejected with `Invalid commit hash in selection`. — (live: session s4)
- Full `git log` output (`commit ed5fa5c863...`) is rejected with the same message. — (live: session s4)
- Outside a repository, `Space-m c` and `Space-m C` report `Invalid commit hash in selection`. — (live: session s3)

### Inferences
- Since `--graph` and full log output are common formats, accepting them or naming the expected format in the error would likely make this much easier to adopt.

### Gaps
- Selecting lines inside a `:sh git log` popup was not tested, since it does not appear to be a selectable buffer.
- A log produced by a separate file on disk was not tested; behaviour should match but was not observed.

## 4. Buffer-to-buffer diff

### Takeaway
Rating 4/5. `:diff-buffer` is simple and works outside a repository, against unsaved scratch buffers, and with both sides editable. Its only weakness is the id-based message.

### Cited Findings
- `:diff-buffer a.txt` opens side by side at once, with status `Diffing buffer 1 against current buffer`. The message gives a number rather than a name. — [capture s3__30-diffbuf](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/s3__30-diffbuf.txt)
- Both panes are editable, and edits in either update the diff live. — (live: session s3)
- `Space-m v` collapses to single-pane with `Split view off`. `Space-m q` closes it, and an unsaved edit stays in its buffer. — (live: session s3)
- With no argument, a picker lists the other buffers, with `+` flagging modified ones. — (live: session s3)
- Scratch versus file works, including live updates on the scratch side. — (live: session s3)
- `:diff-buffer nosuchbuffer` reports `'diff-buffer': No matching buffer: nosuchbuffer`. — (live: session s2)
- `:diff-buffer README.md` inside an existing split replaces the comparison. — (live: session q1)
- Diffing a buffer against itself is refused, with a slightly misleading message: `'diff-buffer': No matching buffer: src/main.py`. — (live: session v1)

### Inferences
- Buffer diffs open split while repo diffs open single-pane, an inconsistency users are likely to notice.

### Gaps
- Very large buffer-to-buffer diffs were not tested.

## 5. Real merge conflict: 3-way view, resolving, finishing

### Takeaway
Rating 3/5. Entering the view and accepting sides is the best interaction on the branch (5/5), and only `Space-m f` touches the index. Three things hold it back: the bottom panes never follow the conflict and are unlabelled, delete/modify conflicts are dead ends in the picker, and `Space-m f` reports success on a file that still contains conflict markers.

### Cited Findings

**Fixture**
- `git merge feature` produced `UU c.txt`, `DU delmod.txt`, `UD moddel.txt`, `UU src/a.py` (3 conflicts), `UU src/b.txt` (2 conflicts) and an auto-merged `M  auto.txt`. — (live: session m1)

**Entering and identifying panes**
- Conflicted files are marked `x conflict` and the preview shows raw markers. On first open the auto-merged row sorts above the conflicts; on later opens conflicts come first. — [capture m1__01-picker](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/m1__01-picker.txt)
- Layout: the working file fills the top (19 rows), with two snapshot panes below (20 rows). The cursor starts at `1:1` and no conflict count is shown, so if the first conflict is off screen, nothing visible says there is one. — [capture m1__02-merge-open](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/m1__02-merge-open.txt)
- Bottom panes are identified only by hash paths with no ours, theirs, HEAD or branch name, and `[readonly]` is clipped. — [capture m1__02-merge-open](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/m1__02-merge-open.txt)

```text
       ~/.cache/helix/git-rev/8a8b32e55211e760/a.py  [read 1 sel  1:1 │       ~/.cache/helix/git-rev/40ae9df074eb4a4d/a.py  [rea 1 sel  1:1
```

**Bottom panes do not follow**
- With the top pane at line 146 (conflict 3), both bottom panes stayed at lines 1-19. — [capture m1__04-conflict3](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/m1__04-conflict3.txt)
- `]g` and `C-d` in the top pane left them unchanged. Scrolling one bottom pane did not move the other. — (live: session m1)
- `Space-m s` and `Space-m v` report `Not in a diff view` inside merge view, although the infobox still lists them. — (live: session m1)

**Navigating and resolving**
- `Space-m n` and `p` report `Conflict 1/3` and wrap. After resolving, the total drops (`Conflict 1/2`). — [capture m1__03-next-conflict-1](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/m1__03-next-conflict-1.txt); (live: session m1)
- `Space-m o`, `t`, `b` report `Accepted HEAD version`, `Accepted incoming version`, `Accepted both version`, and the text is correct. Both keeps ours then theirs with markers removed. — (live: session m1)
- Accepting does not advance, so each conflict costs `Space m n` then `Space m o`. — (live: session m1)
- Merge keys pressed from a bottom pane act on the top document and return focus there. `Space-m f` and `Space-m q` also work from a bottom pane. — (live: sessions m1, f1)
- With the cursor elsewhere: `No conflict at cursor`. When none are left: `No conflicts remaining`. — (live: session m1)
- `u` restores the conflict with markers, and `U` redoes. — (live: session m1)
- Switching sides works: after `Space-m t`, pressing `Space-m o` on the resolved text reports `Switched to HEAD version`. — (live: session m1)
- `diff3` markers (`|||||||`) work: all three accepts remove them, and the finished file had zero markers. — (live: session d3)
- `zdiff3` could not be tested (see Gaps).

**Finishing, and whether markers survive**
- `Space-m f` with a whole block left reports `1 conflict(s) still unresolved`. It saves nothing and the status stays `UU`. — (live: session m1)
- After resolving: `File saved and staged (git add)`, and `UU` becomes `M `. — (live: session m1)
- **Markers can survive.** On a real conflict in `c.txt`, I deleted only the `>>>>>>>` line, then pressed `Space-m f`. It reported `File saved and staged (git add)` and the index holds `<<<<<<< HEAD` and `=======`. The book says this case produces an error. — (live: session m2); `book/src/diff-and-merge.md:94-95`

```text
File saved and staged (git add)
M  c.txt
10:<<<<<<< HEAD
12:=======
```

- A lone typed `<<<<<<< fake marker` or `=======` line also passes. — [capture m1__05-after-finish](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/m1__05-after-finish.txt); (live: session m1)
- `:w` writes markers without complaint, which is normal for a plain write. — (live: session m1)
- Deleting a whole conflict block by hand works; the count updates to `No conflicts remaining`. — (live: session m3)

**Index handling**
- `git status` stayed `UU` through `Space-m o`, `t`, `b`, undo, `:w` and `Space-m q`. Only `Space-m f` changed it. — (live: sessions m1, m5)
- `Space-m f` stages any file, with or without a merge: on an ordinary unstaged file it reports `File saved and staged (git add)`. On `src/both.txt` (`MM`) it swept unstaged hunks into the index (`M `). — (live: sessions s4, d1)
- On snapshots and deleted-file panes it safely reports `Failed to save: Cannot save a git revision document`. — (live: session z1)

**Moving between files**
- After a successful finish the merge view stays open with no pointer to remaining files. — [capture m1__05-after-finish](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/m1__05-after-finish.txt)
- `Space-g` from inside merge view works and swaps the session to the next file. Finished files move to `~ modified`. — [capture m1__06-switch-to-b](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/m1__06-switch-to-b.txt)
- `Space-m q` closes merge view and removes its snapshots, leaving `UU` if unresolved. — (live: session m1)
- Finishing the merge had to be done in a shell. Nothing suggests `git commit`. After committing, the picker is empty. — (live: session m1)

### Inferences
- The bottom panes are decorative for any conflict past the first screen. A user has to look at markers in the top pane, which is what they would do without a 3-way view, or scroll two panes by hand with no labels to say which is which.
- `Space-m f` is the highest-stakes key here and has the weakest check. A false success puts conflict markers one `git commit` away from history. Requiring no marker-shaped lines at all, not only complete blocks, would match what the book already promises.
- Binding save-and-stage to a key that also works outside a merge means a slip stages partial work with no confirmation. Scoping it to merge sessions would reduce that risk.

### Gaps
- `zdiff3` was not tested: git 2.34.1 rejects it (`fatal: unknown style 'zdiff3' given for 'merge.conflictstyle'`).
- Rename/rename, add/add, file/directory and binary conflicts were not tested.
- CRLF conflict files were not tested.
- Cursor placement on `Space-m b` for very large blocks was not examined.

## 6. Rebase conflicts, ours/theirs labels, delete/modify conflicts, other sources

### Takeaway
Rating 2/5. The `HEAD`/`incoming` wording is literally true but inverted from how a rebasing user thinks, with no hint. Multi-commit rebases hit stale buffers and stale panes, and delete/modify conflicts cannot be opened at all. Stash pop and cherry-pick conflicts work.

### Cited Findings

**Labels during rebase**
- Rebasing `mywork` onto `main` gives markers `<<<<<<< HEAD` (upstream) and `>>>>>>> 4ff7065 (my work on line 10)` (the user's commit). — [capture r1__01-rebase-merge](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/r1__01-rebase-merge.txt)
- `Space-m o` reports `Accepted HEAD version` and picks `app line 10 UPSTREAM-MAIN`. `Space-m t` reports `Accepted incoming version` and picks `app line 10 MY-BRANCH-WORK`. — (live: session r1)
- The infobox text is `Accept HEAD version for current conflict` and `Accept incoming version for current conflict`, with keys `o` and `t`. Nothing changes during a rebase. — [capture s1__06-space-m-info](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/s1__06-space-m-info.txt)

**Multi-commit rebase in the same file**
- After `Space-m f`, `git rebase --continue` stopped on a second conflict in the same file. `hx` showed no message, and the buffer still held the first commit's text. — (live: sessions r1, r3)
- `Space-g` still showed `x conflict app.txt`, but `Enter` then `Space-m n` reported `No conflicts remaining`, while the file on disk had markers. — (live: session r1)
- `:reload` recovers the document: `Conflict 1/1`. — (live: session r1)
- **Wrong content after `:reload`.** With merge view left open across `rebase --continue`, the bottom panes stayed on the previous commit. — (live: session r3)

```text
expected  ours: line 10 MY-1, line 30 UP     theirs: line 10 MY-1, line 30 MY-2
observed  ours: 10 ▍app line 10 UP           theirs: 30 ▍app line 30
```

  The theirs pane did not contain the incoming change for the conflict on screen.
- Closing with `Space-m q` and re-selecting gives fresh, correct panes, whether `:reload` came before or after closing. — (live: sessions r2, r4)

**Delete/modify**
- Both `DU delmod.txt` and `UD moddel.txt` show `x conflict` with a normal preview, but `Enter` gives this and leaves the user on the previous buffer. — [capture m1__07-delmod](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/m1__07-delmod.txt); [capture m1__08-moddel](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/m1__08-moddel.txt)

```text
Failed to open merge view: No conflict markers found in file
```

- **Hazard, hit by accident.** Because that failure leaves the previous file (`c.txt`) on screen, my next keys acted on it: `Space-m n` then `Space-m o` resolved it and `Space-m f` staged it. The status line named no file. — (live: session m1)
- Workarounds found: `:open delmod.txt` then `Space-m f` keeps the file, moving `DU` to `A `. `:sh git rm -f moddel.txt` removes it. Neither is suggested on screen. — (live: session m1)

**Other conflict sources**
- `git stash pop`: markers read `Updated upstream` and `Stashed changes`. View, accept and finish work. The stash entry is kept, as git does. — (live: session st)
- `git cherry-pick`: view works, with `>>>>>>> c670b89 (other change)`. — (live: session cp)

### Inferences
- On a rebase, `o` probably reads as "ours" meaning "my work", and it selects the opposite. Status text saying which branch or commit was taken, such as the commit subject already in the marker, would make the outcome checkable.
- Stale panes during a rebase are worse than empty panes, because they look authoritative. The accept keys appear to work from the markers in the top pane, so a user who trusts the bottom panes could choose wrongly while the keys behave as designed.
- Since rebases commonly revisit the same file, this flow is likely to be hit often, not only as a corner case.

### Gaps
- The hazard after a failed open is from one accidental run; I did not try other sequences.
- `git rebase --abort` while merge view is open was not tested. `git merge --abort` was (see question 7).
- Interactive rebase with edit stops was not tested.

## 7. Abuse: closing panes, splits, resizing, on-disk changes, git commit mid-session

### Takeaway
Rating 2/5, driven by one deterministic panic. Otherwise the code held up well: no other crash occurred in pane closes, nested splits, or resizing down to 30x10. The session weaknesses are stale data. Open diffs and merge views do not notice HEAD moving.

### Cited Findings

**Panic (exit code 101)**
- `:bc` on the top pane of a merge view crashes `hx`. It reproduced every time it was tried through the picker and `:merge`. — [capture p1__panic-bc-top-pane](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/p1__panic-bc-top-pane.txt)

```text
thread 'main' panicked at helix-view/src/editor.rs:1869:23:
called `Option::unwrap()` on a `None` value
...
  5: helix_view::editor::Editor::_refresh
  6: helix_view::editor::Editor::close_document
  ...
 16: helix_term::commands::typed::buffer_close
HX_EXITED=101
```

- Trigger matrix on the top pane:

| Command, situation | Result |
| --- | --- |
| `:bc` (clean, via picker or `:merge`) | panic |
| `:bc!` with unsaved edit | panic |
| `:buffer-close-all` | panic |
| `:write-buffer-close` | panic |
| `:bc` with unsaved edit | refused: `'buffer-close': 1 unsaved buffer remaining: ["src/a.py"]` |
| `:buffer-close-others` | alive |
| `:bc` on either bottom pane | alive, view dissolves to single pane |

  — (live: sessions p1-p3)
- The crash depends on the pane's history. `hx src/a.py`, `hx src/a.py c.txt` and `hx c.txt src/a.py` followed by `:merge`, `:bc` all crash. `hx c.txt src/a.py`, `:open src/a.py`, `:merge`, `:bc` does not. — (live: sessions p6, p7)
- In that surviving case the top pane falls back to `c.txt`, while the bottom panes keep showing `a.py` snapshots, an orphaned view. — (live: session p7)
- **Unsaved work is lost.** `hx auto.txt`, type text, `:vnew`, `:open src/a.py`, `:merge`, `:bc` crashed, and the typed text never reached disk. — (live: session p8)
- `git status` was unchanged after the crash, with all `UU` entries intact. — (live: session p1)
- `:merge` also replaced an existing vertical split: the `auto.txt` pane in that test disappeared from the layout. — (live: session p8)

**Closing panes, split diff**
- `:q` on the right pane or `C-w q` on the left collapses to single pane with the working file, no message. `Space-m v` then reports `Not in a diff view`. — (live: session a1)
- `:bc` on the right pane closes the file and leaves `[scratch]`. `:bc` on the left closes only the snapshot. — (live: session a1)
- `:bc` on either range-diff pane returns to `src/main.py` and cleans up both snapshots, a good result. — (live: session p5)
- `:wq` in a snapshot pane of a live split is refused and the session survives: `'write-quit': Cannot save a git revision document`. `:q` there collapses to the working file. `Space-m q` from the left pane also works. — (live: session v1)

**Closing panes, merge view**
- `:q` on the top pane or `C-w q` on a bottom pane dissolves the whole view to a single pane with the working file. `Space-m n/o/t` keep working from markers, and `Space-m q` then reports `Not in a diff or merge view`. — (live: session m5)
- `:open auto.txt` mid-merge and then `ga` returns to `src/a.py` in single pane. The merge view is not restored, unlike a diff, though `Space-m f` still correctly reports `3 conflict(s) still unresolved`. — (live: session m5)

**Splits and opening files**
- `C-w s` and `C-w v` inside a split diff add a third view with no crash. Closing it returns to two panes. — (live: session a1)
- `:open README.md` from the right pane collapses to single pane, and `ga` restores the split. From the left pane it keeps the working file on the right, and `ga` restores it. — (live: session a1)
- `:diff-reset` and `Space-m r` both close a live split with `Diff reset to HEAD`. — (live: sessions p5, v1)
- `:q` with unsaved edits is refused: `'quit': 1 unsaved buffer remaining: ["README.md"]`. — (live: session a3)

**Resizing**
- 80x24 and 60x20 render without panic. Hunk jumps still align at 80x24, where the picker also drops its preview. — [capture a2__resize-80x24](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/a2__resize-80x24.txt); (live: session a2)
- At 80 columns the two merge labels are identical: `~/.cache/helix/git-re 1 sel  1:1 9       ~/.cache/helix/git-r 1 sel  1:1`. — (live: session m4)
- Left-pane statusline text spills past its pane. At 80 columns the right pane reads `f NOR   src/main.py    ly] 1 sel  11:1`, and at 60 columns `1 NOR   src/main 1 sel  118:1`. — (live: session a2)
- That overflow is not diff-specific: a plain `:vsplit` with a long left path shows the same stray characters (`g       README.md    xt`). It is more likely here because snapshot paths are long. — (live: session ctl)
- At 60 columns the infobox clips: `c  Set diff range from selected git log lines, bottom li`. — (live: session a2)
- 30x10 renders without panic, with statuslines reduced to `  1 sel  118:1 / 1 sel  118:1`, and returning to 140x40 restores the layout. — (live: session a2)

**Soft wrap (non-default)**
- Identical wrap heights stay aligned. If a change alters wrap height, rows drift: left `6  short 6` faces right `▍↪ word word...`, and right `6  short 6` faces left `11  short 11`. — (live: sessions w1, w2)

**On-disk changes**
- No notification appears when a file changes on disk. `:w` then reports `file modified by an external process, use :w! to overwrite`, and `:w!` silently overwrites. `:reload` picks up the change and updates the diff. — (live: session a3)
- `:reset-diff-change` in the right pane reverts a hunk (`Reset 1 change`). In the read-only left pane it reports `Reset 1 change` while nothing changes. — (live: session p5)

**`git commit` in another tmux window**
- After committing everything mid-diff, the open split kept comparing against the old HEAD, with no message. Old hunks remained after `j`, `]g`, an edit, `:w` and `:reload`, while `git status` showed only my later edit. The picker was correct at the same moment. — (live: session a3)
- Re-selecting the file in `Space-g` refreshes it, with no need for `Space-m q`. The stale marks disappeared and the only hunk became `241 ▍trailing line`. — (live: session a4)

**`git merge --abort` in another window**
- With the view open there was no notice. `Space-m o` still reported `Accepted HEAD version` on the stale buffer. After `:reload`, `Space-m n` reported `No conflicts remaining`, and `Space-m f` was harmless. — (live: session m5)

**Unconfirmed**
- One `hx` launch never drew a frame: the main thread sat in `futex_wait_queue_me`, the log stayed empty, and typed keys echoed to the screen. It had been alive for over 44 seconds when I stopped it, right after a fixture rebuild and tmux server restart. Four deliberate repeats of that sequence and every later launch started normally. I cannot say whether this was Helix or the harness. — (live: session s2)
- Once, a fast-typed `:diff-buffer README.md` plus `Enter` left the prompt open. This did not recur with a slightly longer delay, so it may be harness timing. — (live: session a1, q1)

### Inferences
- `:bc` is a routine command and merge view is where someone may want out of a file, so this panic is likely to be hit by ordinary use. Its unsaved-work loss makes it the top fix on this branch.
- Stale comparisons appeared in three flows (`git commit`, `git rebase --continue`, `git merge --abort`) and may share one cause. A refresh on focus or on `:reload` would likely help all three, which suits a tool meant to run next to a terminal.

### Gaps
- Comparison with `master` was not run, so I cannot say whether the statusline overflow predates this branch.
- The crash was not tried in a plain `:diff-buffer` session with both panes at the end of their history.
- No panic was seen in resize tests, but I did not fuzz layouts.

## 8. How obvious was the next step from on-screen hints alone?

### Takeaway
Rating 3/5 at the entry points and 2/5 once inside. The `Space` menu and `Space-m` infobox advertise the features well, but both go quiet once a session starts: no mode indicator, no side labels, no conflict count on entry, and no "what's next" after finishing.

### Cited Findings
- The `Space` menu shows `g    Open changed file picker` and `m    Merge/Diff`. — [capture s1__01-space-menu](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/s1__01-space-menu.txt)
- `Space-m` shows all 12 actions with clear descriptions, the same list in every context, including `o/t/b` in a plain diff and `v/s` in merge view, where they fail. — [capture s1__06-space-m-info](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/s1__06-space-m-info.txt)
- `]g` and `[g` do not appear in `Space-m`. I knew them only from the docs. — [capture s1__06-space-m-info](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/s1__06-space-m-info.txt)
- Good hints exist. `Diff range set to: ... — use space+g to browse files` and `...space m C shows the commit` both point at the next action. `Conflict 2/3` orients the user well. — (live: sessions s2, s4, m1)
- Command help popups explain arguments as you type: `:diff-commit` documents `REF`, `REF!` and `REF1..REF2`, and `:merge` reads `Open a 3-way merge view for the current conflicted file.` — (live: session q2)
- Gaps in guidance, step by step:

| Moment | What the screen says | What the user needs |
| --- | --- | --- |
| Picker `Enter` on modified file | nothing new | that a diff is active; how to split |
| Inside split | hash path on left | which ref the left pane is |
| `i` in read-only pane | `INS` | that the pane is read-only |
| Entering merge view | stale `Loading changes for: HEAD` | number of conflicts; how to reach them |
| Merge bottom panes | hash paths | which is ours and which is theirs |
| After `Space-m f` | `File saved and staged (git add)` | remaining files; next git command |
| Delete/modify `Enter` | `No conflict markers found in file` | that it is a delete/modify conflict; how to resolve it |
| Empty picker | blank | why it is empty |

  — (live: sessions s1, m1); [capture m1__02-merge-open](/tmp/claude-1000/-home-kuba-SRC-helix--worktrees-dev-3-way-diff-second-attempt/aa9493cb-2824-42e2-849c-fba84a6ad8e9/scratchpad/ux-fixtures/screens/m1__02-merge-open.txt)
- `:insert-output git log --oneline` as the route to `Space-m c` is not hinted anywhere on screen. I used it from general knowledge of Helix. — (live: session s4)

### Inferences
- A one-line statusline element naming the session, such as `HEAD ↔ worktree` or `Conflict 0/3 · ours | theirs`, would likely answer most rows in the table above at low cost.
- The one-shot hints work because they appear at the moment of decision. The same style after `Space-m f`, naming what remains, would likely close the largest remaining gap in the merge flow.

### Gaps
- This judgement comes from one tester who read the book and keymap before starting, not the blind first look the brief asked for. It is a reconstruction of what the screen offered, not a usability study, and no first-time users were observed.
- Hint behaviour with non-default themes or a minimal statusline config was not checked.
