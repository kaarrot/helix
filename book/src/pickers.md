## Using pickers

Helix has a variety of pickers, which are interactive windows used to select various kinds of items. These include a file picker, global search picker, and more. Most pickers are accessed via keybindings in [space mode](./keymap.md#space-mode). Pickers have their own [keymap](./keymap.md#picker) for navigation.

The global search picker (`Space` `/`) searches the whole workspace but orders matches by proximity to the current file: the current file first, then its directory and subdirectories, then the rest of the workspace, without repeating locations that were already searched.

### Filtering Picker Results

Most pickers perform fuzzy matching using [fzf syntax](https://github.com/junegunn/fzf?tab=readme-ov-file#search-syntax). Two exceptions are the global search picker, which uses regex, and the workspace symbol picker, which passes search terms to the language server. Note that OR operations (`|`) are not currently supported.

If a picker shows multiple columns, you may apply the filter to a specific column by prefixing the column name with `%`. Column names can be shortened to any prefix, so `%p`, `%pa` or `%pat` all mean the same as `%path`. For example, a query of `helix %p .toml !lang` in the global search picker searches for the term "helix" within files with paths ending in ".toml" but not including "lang".

You can insert the contents of a [register](./registers.md) using `Ctrl-r` followed by a register name. For example, one could insert the currently selected text using `Ctrl-r`-`.`, or the directory of the current file using `Ctrl-r`-`%` followed by `Ctrl-w` to remove the last path section. The global search picker will use the contents of the [search register](./registers.md#default-registers) if you press `Enter` without typing a filter. For example, pressing `*`-`Space-/`-`Enter` will start a global search for the currently selected text.

### Changed-file diff picker

`Space-g` opens a picker backed by git changes. By default it compares `HEAD`
to the working tree and lists modified, untracked, renamed, deleted, and
conflicted files.

Selecting a normal entry opens that file in diff view. Selecting a conflicted
entry opens the 3-way merge view instead.

You can change the picker's scope before opening it:

- `:diff-commit REF` compares `REF` to the working tree.
- `:diff-commit REF!` shows only the changes introduced by `REF`.
- `:diff-commit REF1..REF2` compares two refs directly.
- `Space-m c` reads two or more selected git log lines and sets the range to
  `OLDER..NEWER`.
- `Space-m C` reads one selected git log line and sets the range to
  `COMMIT^..COMMIT`.

For a one-off picker of files changed between a ref and the current branch, use
`:diff-files REF`.
