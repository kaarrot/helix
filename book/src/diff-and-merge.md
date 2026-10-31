# Diff and merge

Helix can review git changes in-place, compare arbitrary revisions, and resolve
conflicts without leaving the editor.

## Single-buffer diffing

Use `:diff-base [REF]` to compare the current buffer against a specific git ref.
With no argument it resets the buffer back to `HEAD`.

- `:diff-base HEAD^` compares the current file to the previous commit.
- Git ancestry expressions such as `HEAD~3`, numbered parents, branches, tags,
  and commit hashes are supported. Inside a Git comparison, `:diff-base` changes
  that comparison's base.
- `:toggle-char-diff` toggles character-level highlighting inside changed lines.
- `:diff-reset` clears any custom diff range and restores the default `HEAD`
  comparison.

## Buffer diffing

Use `:diff-buffer [BUFFER]` to compare the current buffer against another open
buffer. `BUFFER` can be a buffer id, a path, or a display name. With no argument
it opens a picker.

This works for scratch buffers and other buffers that have not been written to
disk. Buffer diffs open side-by-side by default; use `Space-m v` to toggle back
to a single-pane diff view.

## Repo-wide review

Use `Space-g` to open the changed-file picker for the current repository. By
default it shows `HEAD` versus the working tree.

- `:diff-commit REF` changes `Space-g` to compare `REF` with the working tree.
- `:diff-commit REF!` limits `Space-g` to the changes introduced by one commit.
- `:diff-commit REF^!` is the Git spelling of the same single-commit comparison.
  Merge commits are compared to their first parent; root commits use an empty base.
- `:diff-commit REF1..REF2` compares two refs directly.
- `:diff-commit REF1...REF2` compares their merge base with `REF2`.
- `Space-m c` reads selected git log lines. The bottom line is the base and is
  not included: one line becomes `COMMIT` versus the working tree; two or more
  lines become `OLDER..NEWER`. This matches typed `:diff-commit`. On `HEAD`
  only uncommitted changes remain, possibly none, and the status line says so.
- `Space-m C` reads one selected git log line and sets the picker range to
  `COMMIT^..COMMIT`.
- `:diff-files REF` opens a one-off picker comparing `REF` with the working tree,
  without changing the `Space-g` range.

Revision endpoints are validated before replacing the current picker scope.
Invalid refs and failed listings report errors instead of an empty result.

Selecting a normal entry opens diff view for that file. Selecting a conflicted
entry opens merge view instead.

## Split diff controls

Once a diff view is open:

- `]g` jumps to the next diff hunk.
- `[g` jumps to the previous diff hunk.
- `Space-m v` toggles side-by-side split diff view.
- `Space-m s` toggles synchronized scrolling between split panes. When enabled (the default), mouse wheel, `j`/`k`/arrows, page scrolling, and search keep both viewports aligned. Hunk jumps (`]g`/`[g`) still move both cursors as well.
- `Space-m q` closes the current diff or merge view.

Returning to a diff's buffer with `Space-b`, `ga`, buffer cycling, or jump history
restores the comparison and its split setting without losing your cursor position.
If both split panes navigate elsewhere, the diff returns when both panes show
their original buffers again. `:diff-reset` also forgets suspended comparisons.

You can compare the same buffer against different revisions in separate views.
Each comparison keeps its own highlighting, hunk navigation, and scrolling;
edits update every comparison. If opening a comparison or toggling its split
fails, the current diff remains available.

## Git revision buffers

A range-diff (`:diff-commit REF!`, `:diff-commit REF1..REF2`, `Space-m C`)
opens read-only snapshots named `filename @ ref`, not the file on disk.
Those snapshots are distinct from the working-tree buffer:

- `:o` of the same path opens the on-disk file and leaves the snapshot in the
  buffer list (`Space-b`).
- The same file at two commits is two buffers (`foo.py @ abc123` and
  `foo.py @ def456`). Opening the same path and ref again reuses that snapshot.
- Language servers attach to the snapshot through a unique cache URI so they
  do not collide with the working-tree file. Workspace discovery still uses
  the real project root, so hover and completions can work; goto-definition
  still resolves through the current checkout, not the historical tree.
- `Space-m q` closes the current diff session and its revision buffers.

## Merge resolution

Open merge view for the current conflicted file with `:merge`, or by selecting a
conflicted entry from `Space-g`.

- `Space-m o` accepts the `HEAD` side for the current conflict.
- `Space-m t` accepts the incoming side.
- `Space-m b` accepts both sides.
- `Space-m n` and `Space-m p` move between unresolved conflicts.
- `Space-m f` saves the file and stages it with `git add` when no conflict
  markers remain.

If conflict markers are still present, `Space-m f` reports an error instead of
staging the file.

## Orientation and navigation

Every active comparison names its endpoints in the statusline. Split panes show
`BASE` and the working tree or target revision; revision names include the commit
identity used by that snapshot. Merge panes show `RESULT`, the two index sides,
and the number of remaining marker blocks. Snapshot names also appear in the
buffer picker and bufferline. Attempts to enter insert mode in a snapshot report
that it is read-only.

Git file comparisons initially use a single pane. Opening a comparison reports
its endpoints and the split toggle. To open them side-by-side automatically:

```toml
[editor.diff]
split-view = true
```

- `]F` and `[F` open the next and previous changed file in the current range.
  They report when the first or last file is reached.
- `Space-m d` reverts the diff hunks intersecting the selections, like
  `:diffget`. Reverting is undoable and requires an editable, current comparison.
- `Space-M` opens a sticky version of the merge/diff menu. Press `Esc` to leave
  it; ordinary `Space-m` continues to perform one action at a time.

Comparisons split the focused pane and keep neighboring panes and their unsaved
buffers. Closing a comparison removes its extra panes. The working pane keeps
its jump history and alternate buffers.

With synchronization enabled, switching panes maps the active cursor to avoid
jumping back to the other pane's old location. Disable synchronization to keep
independent pane positions.

Staged and historical renames retain their original paths. An unstaged move may
initially appear as a deleted/untracked pair; opening the new file searches the
deleted candidates for its original base without hashing all untracked files.
Binary entries report that text comparison is unavailable and keep the current
comparison open.

## Changed Git state

A comparison against a moving ref, such as `HEAD` or a branch, displays `STALE`
when its snapshot no longer matches the ref. A merge also becomes stale when its
index stages or Git operation change. Historical comparisons against fixed
commit identities remain fixed.

Use `:diff-refresh` or `Space-m R` to refresh the current comparison. Ordinary
working-buffer edits are preserved. If merge inputs changed while the result has
unsaved edits, preserve those edits before refreshing; Helix refuses to replace
them. Stale merge accepts and stale hunk reverts report an error.

`:diff-reset` closes active comparisons, forgets suspended comparisons and the
split override, clears the picker range, and restores ordinary `HEAD` diffs.

## More merge controls

Merge view opens at the first conflict. The reference panes follow the result
when navigating conflicts or scrolling, using a separate mapping for each side.
`Space-m s` also controls merge scrolling.

- `Space-m B` opens the index base as an additional read-only reference pane.
  Add/add conflicts have no base.
- `Space-m a` accepts the base content for the current conflict. Diff3 markers
  supply it directly; ordinary markers use the corresponding index-base region.
- `Space-m O`, `Space-m T`, and `Space-m A` accept HEAD, incoming, or both and
  advance to the next conflict. Lowercase accepts stay at the resolution so a
  second accept can switch the choice. Each accept is undoable from the result.

During a rebase, HEAD is labeled **upstream HEAD** and the incoming side is
labeled **replayed commit**. Thus `o` takes upstream and `t` takes the commit
being replayed, which is usually the user's own commit.

Delete/modify conflicts open with the missing side labeled as deleted. For these
marker-less conflicts, accepting a side selects the entire file; choosing the
missing side prepares a deletion. `Space-m f` completes that choice. External
file changes prevent a pending deletion from removing newer disk content.
Already hand-resolved, unstaged conflicts can also be opened and finished.

Finish applies only to files that still have unresolved index stages. It stages
all changes in that file. It rejects orphaned or malformed marker lines as well
as intact conflicts, and respects the file's `conflict-marker-size` attribute.
Ambiguous marker-like content requires manual editing or staging with Git.
Ordinary `:w` can save unfinished marker text without staging it.

Finishing a file does not commit or continue a merge, rebase, or cherry-pick.
Continue the Git operation after resolving and staging its remaining files.
