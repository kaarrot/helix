# Diff and merge

Helix can review git changes in-place, compare arbitrary revisions, and resolve
conflicts without leaving the editor.

## Single-buffer diffing

Use `:diff-base [REF]` to compare the current buffer against a specific git ref.
With no argument it resets the buffer back to `HEAD`.

- `:diff-base HEAD^` compares the current file to the previous commit.
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
- `:diff-commit REF1..REF2` compares two refs directly.
- `Space-m c` reads selected git log lines and sets the picker range to
  `OLDER..NEWER`.
- `Space-m C` reads one selected git log line and sets the picker range to
  `COMMIT^..COMMIT`.
- `:diff-files REF` opens a one-off picker for files changed between `REF` and
  the current branch.

Selecting a normal entry opens diff view for that file. Selecting a conflicted
entry opens merge view instead.

## Split diff controls

Once a diff view is open:

- `]g` jumps to the next diff hunk.
- `[g` jumps to the previous diff hunk.
- `Space-m v` toggles side-by-side split diff view.
- `Space-m s` toggles synchronized scrolling between split panes. When enabled (the default), mouse wheel, `j`/`k`/arrows, page scrolling, and search keep both viewports aligned. Hunk jumps (`]g`/`[g`) still move both cursors as well.
- `Space-m q` closes the current diff or merge view.

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
