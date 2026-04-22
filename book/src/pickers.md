## Using pickers

Helix has a variety of pickers, which are interactive windows used to select various kinds of items. These include a file picker, search pickers, and more. Most pickers are accessed via keybindings in [space mode](./keymap.md#space-mode). Pickers have their own [keymap](./keymap.md#picker) for navigation.

Search mode (`Space` `/`) provides three regex-based search pickers with different scopes: workspace search (`/`), current-directory search (`.`), and current-buffer search (`,`). The current-buffer picker works for both file-backed buffers and scratch buffers.

### Filtering Picker Results

Most pickers perform fuzzy matching using [fzf syntax](https://github.com/junegunn/fzf?tab=readme-ov-file#search-syntax). Two exceptions are the search pickers, which use regex, and the workspace symbol picker, which passes search terms to the language server. Note that OR operations (`|`) are not currently supported.

If a picker shows multiple columns, you may apply the filter to a specific column by prefixing the column name with `%`. Column names can be shortened to any prefix, so `%p`, `%pa` or `%pat` all mean the same as `%path`. For example, a query of `helix %p .toml !lang` in the workspace or current-directory search picker searches for the term "helix" within files with paths ending in ".toml" but not including "lang".

You can insert the contents of a [register](./registers.md) using `Ctrl-r` followed by a register name. For example, one could insert the currently selected text using `Ctrl-r`-`.`, or the directory of the current file using `Ctrl-r`-`%` followed by `Ctrl-w` to remove the last path section. The search pickers will use the contents of the [search register](./registers.md#default-registers) if you press `Enter` without typing a filter. For example, pressing `*`-`Space-/`-`/`-`Enter` starts a workspace search for the currently selected text, while replacing the final `/` with `.` or `,` scopes it to the current directory or current buffer.
