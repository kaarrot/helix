## Surround

Helix includes built-in functionality similar to [vim-surround](https://github.com/tpope/vim-surround).
The keymappings have been inspired from [vim-sandwich](https://github.com/machakann/vim-sandwich):

![Surround demo](https://user-images.githubusercontent.com/23398472/122865801-97073180-d344-11eb-8142-8f43809982c6.gif)

| Key Sequence                              | Action                                  |
| ----------------------------------------- | --------------------------------------- |
| `ms<delimiter><ret>` (after selecting text) | Add delimiters around selection         |
| `mr<char_to_replace><new_char>`            | Replace the closest surround characters |
| `md<char_to_delete>`                       | Delete the closest surround characters  |

You can use counts to act on outer pairs.

`ms` opens an editable delimiter prompt. Type a delimiter and press Enter to
surround every selection in one undo step. Backspace edits the delimiter;
Escape or Enter with empty input cancels without changing the selection.

For example, `ms"""<ret>` adds triple double quotes, and `ms'''<ret>` adds
triple single quotes. Repeating a bracket keeps its matching pair:
`ms((<ret>` or `ms))<ret>` produces `((text))`. Other delimiter strings are
inserted literally on both sides, so `ms**<ret>` produces `**text**`.

Use `ms\n<ret>` to surround with newlines, using the document's line ending
(LF or CRLF). `\\` inserts a literal backslash; other backslash sequences are
left unchanged. For example, `ms\\n<ret>` surrounds with the literal text `\n`.

Single-character delimiters also require Enter: use `ms"<ret>` for `"text"`.
Existing macros that add surrounds need this confirmation key too.

Surround can also act on multiple selections. For example, to change every occurrence of `(use)` to `[use]`:

1. `%` to select the whole file
2. `s` to split the selections on a search term
3. Input `use` and hit Enter
4. `mr([` to replace the parentheses with square brackets

Replacing and deleting surrounds still operate on single-character pairs.

