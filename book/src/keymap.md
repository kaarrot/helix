## Keymap

- [Normal mode](#normal-mode)
  - [Movement](#movement)
  - [Changes](#changes)
    - [Shell](#shell)
  - [Selection manipulation](#selection-manipulation)
  - [Search](#search)
  - [Minor modes](#minor-modes)
    - [View mode](#view-mode)
    - [Goto mode](#goto-mode)
    - [Match mode](#match-mode)
    - [Window mode](#window-mode)
    - [Space mode](#space-mode)
      - [Merge / Diff mode](#merge--diff-mode)
      - [Popup](#popup)
      - [Completion Menu](#completion-menu)
      - [Signature-help Popup](#signature-help-popup)
    - [Unimpaired](#unimpaired)
- [Insert mode](#insert-mode)
- [Select / extend mode](#select--extend-mode)
- [Picker](#picker)
- [Prompt](#prompt)

> 💡 Mappings marked (**LSP**) require an active language server for the file.

> 💡 Mappings marked (**TS**) require a tree-sitter grammar for the file type.

> ⚠️ Some terminals' default key mappings conflict with Helix's. If any of the mappings described on this page do not work as expected, check your terminal's mappings to ensure they do not conflict. See the [wiki](https://github.com/helix-editor/helix/wiki/Terminal-Support) for known conflicts.

## Normal mode

Normal mode is the default mode when you launch helix. You can return to it from other modes by pressing the `Escape` key.

### Movement

> NOTE: Unlike Vim, `f`, `F`, `t` and `T` are not confined to the current line.

> Hereafter, `<n>` represents an integer by typing a sequence of digits.

| Key                   | Description                                        | Command                     |
| -----                 | -----------                                        | -------                     |
| `h`, `Left`           | Move left                                          | `move_char_left`            |
| `j`, `Down`           | Move down                                          | `move_visual_line_down`     |
| `k`, `Up`             | Move up                                            | `move_visual_line_up`       |
| `l`, `Right`          | Move right                                         | `move_char_right`           |
| `w`                   | Move next word start                               | `move_next_word_start`      |
| `b`                   | Move previous word start                           | `move_prev_word_start`      |
| `e`                   | Move next word end                                 | `move_next_word_end`        |
| `W`                   | Move next WORD start                               | `move_next_long_word_start` |
| `B`                   | Move previous WORD start                           | `move_prev_long_word_start` |
| `E`                   | Move next WORD end                                 | `move_next_long_word_end`   |
| `t`                   | Find till next char                                | `find_till_char`            |
| `f`                   | Find next char                                     | `find_next_char`            |
| `T`                   | Find till previous char                            | `till_prev_char`            |
| `F`                   | Find previous char                                 | `find_prev_char`            |
| `<n>G`, `<n>gg`       | Go to line number `<n>`                            | `goto_line`                 |
| `Alt-.`               | Repeat last motion (`f`, `t`, `m`, `[` or `]`)     | `repeat_last_motion`        |
| `Home`                | Move to the start of the line                      | `goto_line_start`           |
| `End`                 | Move to the end of the line                        | `goto_line_end`             |
| `Ctrl-b`, `PageUp`    | Move page up                                       | `page_up`                   |
| `Ctrl-f`, `PageDown`  | Move page down                                     | `page_down`                 |
| `Ctrl-u`              | Move cursor and page half page up                  | `page_cursor_half_up`       |
| `Ctrl-d`              | Move cursor and page half page down                | `page_cursor_half_down`     |
| `Ctrl-i`              | Jump forward on the jumplist                       | `jump_forward`              |
| `Ctrl-o`              | Jump backward on the jumplist                      | `jump_backward`             |
| `Ctrl-s` / `Ctrl-Shift-s` | Send the pending review comment at the cursor, else save the selection to the jumplist | `review_send_or_save_selection` |

### Changes

| Key         | Description                                                          | Command                   |
| -----       | -----------                                                          | -------                   |
| `r`         | Replace with a character                                             | `replace`                 |
| `R`         | Replace with yanked text                                             | `replace_with_yanked`     |
| `~`         | Switch case of the selected text                                     | `switch_case`             |
| `` ` ``     | Set the selected text to lower case                                  | `switch_to_lowercase`     |
| `` Alt-` `` | Set the selected text to upper case                                  | `switch_to_uppercase`     |
| `i`         | Insert before selection                                              | `insert_mode`             |
| `a`         | Insert after selection (append)                                      | `append_mode`             |
| `I`         | Insert at the start of the line                                      | `insert_at_line_start`    |
| `A`         | Insert at the end of the line                                        | `insert_at_line_end`      |
| `o`         | Open new line below selection                                        | `open_below`              |
| `O`         | Open new line above selection                                        | `open_above`              |
| `.`         | Repeat last insert                                                   | N/A                       |
| `u`         | Undo change                                                          | `undo`                    |
| `U`         | Redo change                                                          | `redo`                    |
| `Alt-u`     | Move backward in history                                             | `earlier`                 |
| `Alt-U`     | Move forward in history                                              | `later`                   |
| `y`         | Copy from the focused review box to the clipboard, else yank selection | `review_copy_or_yank`   |
| `p`         | Paste after selection                                                | `paste_after`             |
| `P`         | Paste before selection                                               | `paste_before`            |
| `"` `<reg>` | Select a register to yank to or paste from                           | `select_register`         |
| `>`         | Indent selection                                                     | `indent`                  |
| `<`         | Unindent selection                                                   | `unindent`                |
| `=`         | Format selection (**LSP**)                                           | `format_selections`       |
| `d`         | Delete selection                                                     | `delete_selection`        |
| `Alt-d`     | Delete selection, without yanking                                    | `delete_selection_noyank` |
| `c`         | Continue the unsent review draft at the cursor, else reply, else change selection | `review_comment_or_change` |
| `d`         | Delete the focused review entry, else delete selection               | `review_delete_or_change` |
| `C-left` / `C-right` | Show the previous / next entry in the thread at the cursor   | `review_prev_message` / `review_next_message` |
| `C-up` / `C-down` | Move the cursor up / down inside the focused review box         | `review_scroll_up` / `review_scroll_down` |
| `Ctrl-Shift-c` | Copy from the focused review box, else yank to the clipboard    | `review_copy_or_yank_to_clipboard` |
| `Alt-c`     | Change selection (delete and enter insert mode, without yanking)     | `change_selection_noyank` |
| `Ctrl-a`    | Increment object (number) under cursor                               | `increment`               |
| `Ctrl-x`    | Decrement object (number) under cursor                               | `decrement`               |
| `Q`         | Start/stop macro recording to the selected register (experimental)   | `record_macro`            |
| `q`         | Play back a recorded macro from the selected register (experimental) | `replay_macro`            |

#### Shell

| Key     | Description                                                                      | Command               |
| ------  | -----------                                                                      | -------               |
| <code>&#124;</code>     | Pipe each selection through shell command, replacing with output                 | `shell_pipe`          |
| <code>Alt-&#124;</code> | Pipe each selection into shell command, ignoring output                          | `shell_pipe_to`       |
| `!`     | Run shell command, inserting output before each selection                        | `shell_insert_output` |
| `Alt-!` | Run shell command, appending output after each selection                         | `shell_append_output` |
| `$`     | Pipe each selection into shell command, keep selections where command returned 0 | `shell_keep_pipe`     |


### Selection manipulation

| Key                      | Description                                                       | Command                              |
| -----                    | -----------                                                       | -------                              |
| `s`                      | Select all regex matches inside selections                        | `select_regex`                       |
| `S`                      | Split selection into sub selections on regex matches              | `split_selection`                    |
| `Alt-s`                  | Split selection on newlines                                       | `split_selection_on_newline`         |
| `Alt-minus`              | Merge selections                                                  | `merge_selections`                   |
| `Alt-_`                  | Merge consecutive selections                                      | `merge_consecutive_selections`       |
| `&`                      | Align selection in columns                                        | `align_selections`                   |
| `_`                      | Trim whitespace from the selection                                | `trim_selections`                    |
| `;`                      | Collapse selection onto a single cursor                           | `collapse_selection`                 |
| `Alt-;`                  | Flip selection cursor and anchor                                  | `flip_selections`                    |
| `Alt-:`                  | Ensures the selection is in forward direction                     | `ensure_selections_forward`          |
| `,`                      | Keep only the primary selection                                   | `keep_primary_selection`             |
| `Alt-,`                  | Remove the primary selection                                      | `remove_primary_selection`           |
| `C`                      | Copy selection onto the next line (Add cursor below)              | `copy_selection_on_next_line`        |
| `Alt-C`                  | Copy selection onto the previous line (Add cursor above)          | `copy_selection_on_prev_line`        |
| `(`                      | Rotate main selection backward                                    | `rotate_selections_backward`         |
| `)`                      | Rotate main selection forward                                     | `rotate_selections_forward`          |
| `Alt-(`                  | Rotate selection contents backward                                | `rotate_selection_contents_backward` |
| `Alt-)`                  | Rotate selection contents forward                                 | `rotate_selection_contents_forward`  |
| `%`                      | Select entire file                                                | `select_all`                         |
| `x`                      | Select current line, if already selected, extend to next line     | `extend_line_below`                  |
| `X`                      | Extend selection to line bounds (line-wise selection)             | `extend_to_line_bounds`              |
| `Alt-x`                  | Shrink selection to line bounds (line-wise selection)             | `shrink_to_line_bounds`              |
| `J`                      | Join lines inside selection                                       | `join_selections`                    |
| `Alt-J`                  | Join lines inside selection and select the inserted space         | `join_selections_space`              |
| `K`                      | Keep selections matching the regex                                | `keep_selections`                    |
| `Alt-K`                  | Remove selections matching the regex                              | `remove_selections`                  |
| `Ctrl-c`                 | Comment/uncomment the selections                                  | `toggle_comments`                    |
| `Alt-o`, `Alt-up`        | Expand selection to parent syntax node (**TS**)                   | `expand_selection`                   |
| `Alt-i`, `Alt-down`      | Shrink syntax tree object selection (**TS**)                      | `shrink_selection`                   |
| `Alt-p`, `Alt-left`      | Select previous sibling node in syntax tree (**TS**)              | `select_prev_sibling`                |
| `Alt-n`, `Alt-right`     | Select next sibling node in syntax tree (**TS**)                  | `select_next_sibling`                |
| `Alt-a`                  | Select all sibling nodes in syntax tree (**TS**)                  | `select_all_siblings`                |
| `Alt-I`, `Alt-Shift-down`| Select all children nodes in syntax tree (**TS**)                 | `select_all_children`                |
| `Alt-e`                  | Move to end of parent node in syntax tree (**TS**)                | `move_parent_node_end`               |
| `Alt-b`                  | Move to start of parent node in syntax tree (**TS**)              | `move_parent_node_start`             |

### Search

Search commands all operate on the `/` register by default. To use a different register, use `"<char>`.

| Key   | Description                                 | Command              |
| ----- | -----------                                 | -------              |
| `/`   | Search for regex pattern                    | `search`             |
| `?`   | Search for previous pattern                 | `rsearch`            |
| `n`   | Select next search match                    | `search_next`        |
| `N`   | Select previous search match                | `search_prev`        |
| `*`   | Use current selection as the search pattern, automatically wrapping with `\b` on word boundaries | `search_selection_detect_word_boundaries` |
| `Alt-*` | Use current selection as the search pattern | `search_selection` |

### Minor modes

These sub-modes are accessible from normal mode and typically switch back to normal mode after a command.

| Key      | Description                                        | Command        |
| -----    | -----------                                        | -------        |
| `v`      | Enter [select (extend) mode](#select--extend-mode) | `select_mode`  |
| `g`      | Enter [goto mode](#goto-mode)                      | N/A            |
| `m`      | Enter [match mode](#match-mode)                    | N/A            |
| `:`      | Enter command mode                                 | `command_mode` |
| `z`      | Enter [view mode](#view-mode)                      | N/A            |
| `Z`      | Enter sticky [view mode](#view-mode)               | N/A            |
| `Ctrl-w` | Enter [window mode](#window-mode)                  | N/A            |
| `Space`  | Enter [space mode](#space-mode)                    | N/A            |

These modes (except command mode) can be configured by
[remapping keys](https://docs.helix-editor.com/remapping.html#minor-modes).

#### View mode

Accessed by typing `z` in [normal mode](#normal-mode).

View mode is intended for scrolling and manipulating the view without changing
the selection. The "sticky" variant of this mode (accessed by typing `Z` in
normal mode) is persistent and can be exited using the escape key. This is
useful when you're simply looking over text and not actively editing it.


| Key                  | Description                                               | Command                 |
| -----                | -----------                                               | -------                 |
| `z`, `c`             | Vertically center the line                                | `align_view_center`     |
| `t`                  | Align the line to the top of the screen                   | `align_view_top`        |
| `b`                  | Align the line to the bottom of the screen                | `align_view_bottom`     |
| `m`                  | Align the line to the middle of the screen (horizontally) | `align_view_middle`     |
| `j`, `down`          | Scroll the view downwards                                 | `scroll_down`           |
| `k`, `up`            | Scroll the view upwards                                   | `scroll_up`             |
| `Ctrl-f`, `PageDown` | Move page down                                            | `page_down`             |
| `Ctrl-b`, `PageUp`   | Move page up                                              | `page_up`               |
| `Ctrl-u`             | Move cursor and page half page up                         | `page_cursor_half_up`   |
| `Ctrl-d`             | Move cursor and page half page down                       | `page_cursor_half_down` |

#### Goto mode

Accessed by typing `g` in [normal mode](#normal-mode).

Jumps to various locations.

| Key   | Description                                      | Command                    |
| ----- | -----------                                      | -------                    |
| `<n>g`| Go to line number `<n>`                          | `goto_file_start`          |
| `g`   | Go to the start of the file                      | `goto_file_start`          |
| <code>&lt;n&gt;&#124;</code>  | Go to column number `<n>`      | `goto_column`              |
| <code>&#124;</code>     | Go to the start of line        | `goto_column`              |
| `e`   | Go to the end of the file                        | `goto_last_line`           |
| `f`   | Go to the path under a focused review box's cursor, else to files in the selections | `review_goto_file_or_goto_file` |
| `h`   | Go to the start of the line                      | `goto_line_start`          |
| `l`   | Go to the end of the line                        | `goto_line_end`            |
| `s`   | Go to first non-whitespace character of the line | `goto_first_nonwhitespace` |
| `t`   | Go to the top of the screen                      | `goto_window_top`          |
| `c`   | Go to the middle of the screen                   | `goto_window_center`       |
| `b`   | Go to the bottom of the screen                   | `goto_window_bottom`       |
| `d`   | Go to definition (**LSP**)                       | `goto_definition`          |
| `y`   | Go to type definition (**LSP**)                  | `goto_type_definition`     |
| `r`   | Go to references (**LSP**)                       | `goto_reference`           |
| `i`   | Go to implementation (**LSP**)                   | `goto_implementation`      |
| `a`   | Go to the last accessed/alternate file           | `goto_last_accessed_file`  |
| `m`   | Go to the last modified/alternate file           | `goto_last_modified_file`  |
| `n`   | Go to next buffer                                | `goto_next_buffer`         |
| `p`   | Go to previous buffer                            | `goto_previous_buffer`     |
| `.`   | Go to last modification in current file          | `goto_last_modification`   |
| `j`   | Move down textual (instead of visual) line       | `move_line_down`           |
| `k`   | Move up textual (instead of visual) line         | `move_line_up`             |
| `w`   | Show labels at each word and select the word that belongs to the entered labels | `goto_word` |

#### Match mode

Accessed by typing `m` in [normal mode](#normal-mode).

Please refer to the relevant sections for detailed explanations about [surround](./surround.md) and [textobjects](./textobjects.md).

| Key              | Description                                     | Command                    |
| -----            | -----------                                     | -------                    |
| `m`              | Goto matching bracket (**TS**)                  | `match_brackets`           |
| `s` `<char>`     | Surround current selection with `<char>`        | `surround_add`             |
| `r` `<from><to>` | Replace surround character `<from>` with `<to>` | `surround_replace`         |
| `d` `<char>`     | Delete surround character `<char>`              | `surround_delete`          |
| `a` `<object>`   | Select around textobject                        | `select_textobject_around` |
| `i` `<object>`   | Select inside textobject                        | `select_textobject_inner`  |

TODO: Mappings for selecting syntax nodes (a superset of `[`).

#### Window mode

Accessed by typing `Ctrl-w` in [normal mode](#normal-mode).

This layer is similar to Vim keybindings as Kakoune does not support windows.

| Key                    | Description                                          | Command           |
| -----                  | -------------                                        | -------           |
| `w`, `Ctrl-w`          | Switch to next window                                | `rotate_view`     |
| `v`, `Ctrl-v`          | Vertical right split                                 | `vsplit`          |
| `s`, `Ctrl-s`          | Horizontal bottom split                              | `hsplit`          |
| `f`                    | Go to files in the selections in horizontal splits   | `goto_file`       |
| `F`                    | Go to files in the selections in vertical splits     | `goto_file`       |
| `h`, `Ctrl-h`, `Left`  | Move to left split                                   | `jump_view_left`  |
| `j`, `Ctrl-j`, `Down`  | Move to split below                                  | `jump_view_down`  |
| `k`, `Ctrl-k`, `Up`    | Move to split above                                  | `jump_view_up`    |
| `l`, `Ctrl-l`, `Right` | Move to right split                                  | `jump_view_right` |
| `q`, `Ctrl-q`          | Close current window                                 | `wclose`          |
| `o`, `Ctrl-o`          | Only keep the current window, closing all the others | `wonly`           |
| `H`                    | Swap window to the left                              | `swap_view_left`  |
| `J`                    | Swap window downwards                                | `swap_view_down`  |
| `K`                    | Swap window upwards                                  | `swap_view_up`    |
| `L`                    | Swap window to the right                             | `swap_view_right` |

#### Space mode

Accessed by typing `Space` in [normal mode](#normal-mode).

This layer collects workspace pickers, clipboard helpers, and the `Space-m`
merge/diff submode.

| Key     | Description                                                             | Command                                    |
| -----   | -----------                                                             | -------                                    |
| `f`     | Open file picker at LSP workspace root                                  | `file_picker`                              |
| `F`     | Open file picker at current working directory                           | `file_picker_in_current_directory`         |
| `b`     | Open buffer picker                                                      | `buffer_picker`                            |
| `j`     | Open jumplist picker                                                    | `jumplist_picker`                          |
| `g`     | Open the changed-file diff picker                                       | `changed_file_picker`                      |
| `G`     | Debug (experimental)                                                    | N/A                                        |
| `k`     | Show documentation for item under cursor in a [popup](#popup) (**LSP**) | `hover`                                    |
| `s`     | Open document symbol picker (**LSP**)                                   | `symbol_picker`                            |
| `S`     | Open workspace symbol picker (**LSP**)                                  | `workspace_symbol_picker`                  |
| `d`     | Open document diagnostics picker (**LSP**)                              | `diagnostics_picker`                       |
| `D`     | Open workspace diagnostics picker (**LSP**)                             | `workspace_diagnostics_picker`             |
| `r`     | Rename symbol (**LSP**)                                                 | `rename_symbol`                            |
| `a`     | Apply code action (**LSP**)                                             | `code_action`                              |
| `h`     | Select symbol references (**LSP**)                                      | `select_references_to_symbol_under_cursor` |
| `'`     | Open last fuzzy picker                                                  | `last_picker`                              |
| `w`     | Enter [window mode](#window-mode)                                       | N/A                                        |
| `c`     | Comment/uncomment selections                                            | `toggle_comments`                          |
| `C`     | Block comment/uncomment selections                                      | `toggle_block_comments`                    |
| `Alt-c` | Line comment/uncomment selections                                       | `toggle_line_comments`                     |
| `p`     | Paste system clipboard after selections                                 | `paste_clipboard_after`                    |
| `P`     | Paste system clipboard before selections                                | `paste_clipboard_before`                   |
| `y`     | Yank selections to clipboard                                            | `yank_to_clipboard`                        |
| `Y`     | Yank main selection to clipboard                                        | `yank_main_selection_to_clipboard`         |
| `R`     | Replace selections by clipboard contents                                | `replace_selections_with_clipboard`        |
| `/`     | Global search in workspace folder                                       | `global_search`                            |
| `?`     | Open command palette                                                    | `command_palette`                          |

> 💡 Global search displays results in a fuzzy picker, use `Space + '` to bring it back up after opening a file.

`Space-g` opens a git-backed picker for changed files. By default it compares
`HEAD` to the working tree. Use `:diff-commit`, `Space-m c`, or `Space-m C` to
change that picker to another commit range before opening it. Selecting a
regular entry opens a diff view for that file, while selecting a conflicted
entry opens the 3-way merge view.

##### Merge / Diff mode

Accessed by typing `Space-m` in [space mode](#space-mode).

These mappings control diff review, commit-range selection, and merge
resolution. The commands that open diff or merge views are documented in
[Diff and merge](./diff-and-merge.md).

| Key | Description | Command |
| --- | --- | --- |
| `c` | Set the changed-file picker range from selected git log lines, with the bottom line as the excluded base (`OLDER..NEWER`, or `COMMIT` vs working tree) | `diff_commit_from_selection` |
| `C` | Set the changed-file picker range to show the selected commit only (`COMMIT^..COMMIT`) | `diff_show_commit_from_selection` |
| `r` | Reset diff state back to `HEAD` vs working tree | `diff_reset` |
| `v` | Toggle side-by-side split diff view | `diff_toggle_split_view` |
| `s` | Toggle synchronized scrolling in split diff view | `diff_toggle_sync_scroll` |
| `q` | Close the current diff or merge view | `close_diff_or_merge_view` |
| `o` | Accept the `HEAD` version for the current conflict | `merge_accept_ours` |
| `t` | Accept the incoming version for the current conflict | `merge_accept_theirs` |
| `b` | Accept both sides for the current conflict | `merge_accept_both` |
| `n` | Jump to the next unresolved conflict | `merge_next_conflict` |
| `p` | Jump to the previous unresolved conflict | `merge_prev_conflict` |
| `f` | Save and stage the resolved file (`git add`) | `merge_finish` |
| `R` | Review submenu, see below | |

###### Review

Accessed by typing `Space-m-R`. [Review comments](./review-comments.md) explains
how they work and walks through a typical review. Review comments are anchored to a file and a
line, so they work in any buffer — a diff view is simply where they are most
useful, not a requirement.

Pressing `c` — in the submenu, or on its own on a line that carries a thread — adds a reply rather than
starting a second thread, so a thread grows into a conversation. `c` only does
this when the cursor is on a thread; elsewhere it changes the selection as
usual, at the cost of not being able to change text on a commented line. If the
thread still has an unsent draft, `c` opens that draft again with the text in
place and the cursor at the end. `Esc` closes the box and leaves the saved
draft as it was.

Moving down onto a line that carries a thread stops on its box first: the
cursor stays put, the box is drawn as focused, and the next press carries on.
Going up lands on the line with its box focused, and the press after that
continues. A box cannot hold the cursor itself — it is drawn into virtual rows
rather than document text — so it is given a stop of its own in the motion. A
count (`10j`) travels straight through without stopping.

`C-left` / `C-right` step through that thread's history, and only once its box
is focused: walking a conversation is an action on the box, not on being near
it.

A box never takes more than half the window, so the code it is about stays on
screen. A reply too tall to fit shows the visible range in its header
(`12-40 of 201`). `C-up` / `C-down` walk a cursor down the reply and the box
follows it, scrolling only when the cursor would otherwise leave. Scrolling the
editor *through* a box is not possible — the cursor cannot be inside virtual
rows, so the view would be pulled straight back to the cursor's line — which is
why a box has a cursor of its own instead.

**Drag across a reply with the mouse** to select part of it, then `y` (or
`Ctrl-Shift-C`) to copy it to the **system clipboard**, ready to paste back into
a comment. Clicking a box only points at it; it takes a drag to select, and the
click never moves the text cursor into the code underneath. `C-up` / `C-down`
adjust a selection once there is one. With nothing selected `y` copies the whole
entry — and then it copies the text as it was written rather than as it was
wrapped to fit the pane. Away from a box, `y` is an ordinary yank.

Selecting in a box is by line rather than by character, since the box draws its
own cursor and selection where the editor's cannot go. Trim after pasting, in
the comment you are writing. `Ctrl-Shift-C` reaches Helix only in terminals that
forward it; most keep it for their own copy, which is why `y` is the one to
reach for.

`d` removes the entry being looked at, leaving the rest of the conversation.
Unlike `c` it requires the box to have been stopped on rather than just having
the cursor on its line, since it throws something away. Deleting the last entry
removes the thread with it.

Replying while looking at an older entry continues the conversation from that
point and **discards the entries after it** — the point of going back is to take
it a different way. The status line says how many were dropped. That comment's
conversation still remembers them, so the next message tells it they no
longer stand. Inside the box, Enter inserts a newline, `Ctrl-S`
saves a draft and closes the box, and `Ctrl-Shift-S` saves and sends it straight away.
`c` on that line opens the saved draft again. A thread shows one
entry at a time, with a `3/5` counter in its header, so its height stays bounded
by a single message however long the conversation grows.

`S` sends every unsent comment. The agent is started on the first send. Each
comment has its own conversation, so a reply cannot land on another comment and
one comment does not see the others. A follow-up in the same window resumes
that conversation.

The agent runs with **full access to the worktree**: it can edit files and run
commands without asking, because a comment on a line usually implies a change to
it. A process the editor spawned has no terminal to prompt in, so there is no
approval step. Its edits land in the diff you are reviewing, where they are
visible, and the conversation is kept — but treat it as you would any agent with
write access to your checkout.

The child is **Claude Code** (`claude`) until you pick otherwise with
`:review-session grok`. `:review-session claude` switches back. Either way one
turn is one process, and the UUID stored on that comment decides which
conversation it is. The first turn passes `--session-id`. Every later turn in
the same window passes `--resume` with that same UUID. Claude is `claude -p`
with the prompt on stdin. Grok is `grok --prompt-file`. Turns on different
comments run at the same time. A follow-up sent while its comment's reply is
still arriving stays a draft and goes out once that reply lands, so two
processes never resume one conversation together. Saving or deleting that
draft before then cancels the send. The
UUID is shown at the right of the box's header and of the reply input, so
running `claude --resume <uuid>` (or `/resume <uuid>`) from the worktree opens
that one comment's conversation interactively. Helix does not know about that
session, so do not reply from Helix while it is open there. A
conversation name and an agent can be given together, in either order:
`:review-session spike grok`.

`:q` refuses while a reply is still being written, the same way it refuses
with unsaved buffers. `:q!` stops the agent, together with any command it is
running, and keeps what had arrived of the reply, marked as stopped.

Conversations are saved as they change and come back when you reopen Helix in
the same worktree, including comments you drafted but never sent. They are
shown as soon as the first file in that worktree opens, and track your edits
from then on; nothing is written back until you comment. If another running
Helix owns the worktree's conversation, this one shows its own `worktree#2`
conversation instead. Ones nothing has touched for a year are deleted
automatically.

Each worktree has one conversation, whatever is checked out. A comment is about
the revision it was left on: a file opened from disk, or a diff's working-tree
pane, is the working tree, and a snapshot such as `foo.rs @ abc1234` is that
commit, kept by its full hash. It is shown wherever that revision is shown, in
either pane of any diff. Comments on the working tree stay on the file through
a commit, a stash or a checkout, and follow its text on `:reload`; nothing is
copied onto a new commit.
`:review-session` shows the current conversation and agent.
`:review-session [name]` switches to a conversation of that name, kept apart
from the worktree's; `:review-session worktree` goes back to it.
`:review-session grok` or `:review-session claude` picks the agent without
renaming.

| Key | Description | Command |
| --- | --- | --- |
| `c` | Comment, reply, or continue editing the unsent draft on this line | `review_add` |
| `S` | Send any comments saved but not yet sent | `review_send_all` |
| `t` | Collapse or expand the thread at the cursor | `review_toggle_collapse` |
| `h` | Hide or show every review box | `review_toggle_visible` |

An agent's reply is drawn with the markdown preview renderer: headings, lists,
tables and code are laid out instead of shown as source. That text is not
editable. `d` still deletes the reply on screen, and `c` starts a new reply
rather than opening what the agent wrote.

Everything else happens on the box itself: `c` and `d` act on it, `C-left` /
`C-right` walk its history, `C-up` / `C-down` read a long reply, the mouse
selects from it and `y` copies that. `]c` / `[c` move between review
comments in the current diff, and are code comments in any other buffer.
`]C` / `[C` move between every review comment in the session, in path order,
and open or switch to the buffer that holds the next one.
`Ctrl-Shift-S` in the box sends straight away. After saving with `Ctrl-S`, `c`
on the line opens that draft for editing again. `Ctrl-S` or `Ctrl-Shift-S` in
normal mode send the draft on this line; `S` in the review submenu still sends
every unsent comment.

`h` hides the boxes for the keys as well as for the eye: `c`, `d` and the motion
stops fall back to what they normally do, so a box you cannot see cannot be
replied to or deleted by mistake. Asking to comment shows them again, since that
is what asking implies. The conversations themselves are untouched either way.

##### Popup

Displays documentation for item under cursor. Remapping currently not supported.

| Key      | Description |
| ----     | ----------- |
| `Ctrl-u` | Scroll up   |
| `Ctrl-d` | Scroll down |

##### Completion Menu

Displays documentation for the selected completion item. Remapping currently not supported.

| Key                         | Description                      |
| ----                        | -----------                      |
| `Shift-Tab`, `Ctrl-p`, `Up` | Previous entry                   |
| `Tab`, `Ctrl-n`, `Down`     | Next entry                       |
| `Enter`                     | Close menu and accept completion |
| `Ctrl-c`                    | Close menu and reject completion |

Any other keypresses result in the completion being accepted.

##### Signature-help Popup

Displays the signature of the selected completion item. Remapping currently not supported.

| Key     | Description        |
| ----    | -----------        |
| `Alt-p` | Previous signature |
| `Alt-n` | Next signature     |

#### Unimpaired

These mappings are in the style of [vim-unimpaired](https://github.com/tpope/vim-unimpaired).

| Key      | Description                                  | Command                 |
| -----    | -----------                                  | -------                 |
| `]d`     | Go to next diagnostic (**LSP**)              | `goto_next_diag`        |
| `[d`     | Go to previous diagnostic (**LSP**)          | `goto_prev_diag`        |
| `]D`     | Go to last diagnostic in document (**LSP**)  | `goto_last_diag`        |
| `[D`     | Go to first diagnostic in document (**LSP**) | `goto_first_diag`       |
| `]f`     | Go to next function (**TS**)                 | `goto_next_function`    |
| `[f`     | Go to previous function (**TS**)             | `goto_prev_function`    |
| `]t`     | Go to next type definition (**TS**)          | `goto_next_class`       |
| `[t`     | Go to previous type definition (**TS**)      | `goto_prev_class`       |
| `]a`     | Go to next argument/parameter (**TS**)       | `goto_next_parameter`   |
| `[a`     | Go to previous argument/parameter (**TS**)   | `goto_prev_parameter`   |
| `]c`     | Go to next review comment in the current diff, else next code comment (**TS**) | `goto_next_comment_or_review` |
| `[c`     | Go to previous review comment in the current diff, else previous code comment (**TS**) | `goto_prev_comment_or_review` |
| `]C`     | Go to the next review comment in any buffer | `goto_next_review_comment` |
| `[C`     | Go to the previous review comment in any buffer | `goto_prev_review_comment` |
| `]T`     | Go to next test (**TS**)                     | `goto_next_test`        |
| `[T`     | Go to previous test (**TS**)                 | `goto_prev_test`        |
| `]p`     | Go to next paragraph                         | `goto_next_paragraph`   |
| `[p`     | Go to previous paragraph                     | `goto_prev_paragraph`   |
| `]g`     | Go to next change                            | `goto_next_change`      |
| `[g`     | Go to previous change                        | `goto_prev_change`      |
| `]G`     | Go to last change                            | `goto_last_change`      |
| `[G`     | Go to first change                           | `goto_first_change`     |
| `[x`     | Go to next (X)HTML element                   | `goto_next_xml_element` |
| `]x`     | Go to previous (X)HTML element               | `goto_prev_xml_element` |
| `]Space` | Add newline below                            | `add_newline_below`     |
| `[Space` | Add newline above                            | `add_newline_above`     |

## Insert mode

Accessed by typing `i` in [normal mode](#normal-mode).

Insert mode bindings are minimal by default. Helix is designed to
be a modal editor, and this is reflected in the user experience and internal
mechanics. Changes to the text are only saved for undos when
escaping from insert mode to normal mode.

> 💡 New users are strongly encouraged to learn the modal editing paradigm
> to get the smoothest experience.

| Key                                         | Description                 | Command                  |
| -----                                       | -----------                 | -------                  |
| `Escape`                                    | Switch to normal mode       | `normal_mode`            |
| `Ctrl-s`                                    | Commit undo checkpoint      | `commit_undo_checkpoint` |
| `Ctrl-x`                                    | Autocomplete                | `completion`             |
| `Ctrl-r`                                    | Insert a register content   | `insert_register`        |
| `Ctrl-w`, `Alt-Backspace`                   | Delete previous word        | `delete_word_backward`   |
| `Alt-d`, `Alt-Delete`                       | Delete next word            | `delete_word_forward`    |
| `Ctrl-u`                                    | Delete to start of line     | `kill_to_line_start`     |
| `Ctrl-k`                                    | Delete to end of line       | `kill_to_line_end`       |
| `Ctrl-h`, `Backspace`, `Shift-Backspace`    | Delete previous char        | `delete_char_backward`   |
| `Ctrl-d`, `Delete`                          | Delete next char            | `delete_char_forward`    |
| `Ctrl-j`, `Enter`                           | Insert new line             | `insert_newline`         |

These keys are not recommended, but are included for new users less familiar
with modal editors.

| Key                                         | Description                 | Command                  |
| -----                                       | -----------                 | -------                  |
| `Up`                                        | Move to previous line       | `move_line_up`           |
| `Down`                                      | Move to next line           | `move_line_down`         |
| `Left`                                      | Backward a char             | `move_char_left`         |
| `Right`                                     | Forward a char              | `move_char_right`        |
| `PageUp`                                    | Move one page up            | `page_up`                |
| `PageDown`                                  | Move one page down          | `page_down`              |
| `Home`                                      | Move to line start          | `goto_line_start`        |
| `End`                                       | Move to line end            | `goto_line_end_newline`  |

As you become more comfortable with modal editing, you may want to disable some
insert mode bindings. You can do this by editing your `config.toml` file.

```toml
[keys.insert]
up = "no_op"
down = "no_op"
left = "no_op"
right = "no_op"
pageup = "no_op"
pagedown = "no_op"
home = "no_op"
end = "no_op"
```

## Select / extend mode

Accessed by typing `v` in [normal mode](#normal-mode).

Select mode echoes Normal mode, but changes any movements to extend
selections rather than replace them. Goto motions are also changed to
extend, so that `vgl`, for example, extends the selection to the end of
the line.

Search is also affected. By default, `n` and `N` will remove the current
selection and select the next instance of the search term. Toggling this
mode before pressing `n` or `N` makes it possible to keep the current
selection. Toggling it on and off during your iterative searching allows
you to selectively add search terms to your selections.

## Picker

Keys to use within picker. Remapping currently not supported.
See the documentation page on [pickers](./pickers.md) for more info.
[Prompt](#prompt) keybinds also work in pickers, except where they conflict with picker keybinds.

| Key                          | Description                                                |
| -----                        | -------------                                              |
| `Shift-Tab`, `Up`, `Ctrl-p`  | Previous entry                                             |
| `Tab`, `Down`, `Ctrl-n`      | Next entry                                                 |
| `PageUp`, `Ctrl-u`           | Page up                                                    |
| `PageDown`, `Ctrl-d`         | Page down                                                  |
| `Home`                       | Go to first entry                                          |
| `End`                        | Go to last entry                                           |
| `Enter`                      | Open selected                                              |
| `Alt-Enter`                  | Open selected in the background without closing the picker |
| `Ctrl-s`                     | Open horizontally                                          |
| `Ctrl-v`                     | Open vertically                                            |
| `Ctrl-t`                     | Toggle preview                                             |
| `Escape`, `Ctrl-c`           | Close picker                                               |

## Prompt

Keys to use within prompt, Remapping currently not supported.

| Key                                         | Description                                                             |
| -----                                       | -------------                                                           |
| `Escape`, `Ctrl-c`                          | Close prompt                                                            |
| `Alt-b`, `Ctrl-Left`                        | Backward a word                                                         |
| `Ctrl-b`, `Left`                            | Backward a char                                                         |
| `Alt-f`, `Ctrl-Right`                       | Forward a word                                                          |
| `Ctrl-f`, `Right`                           | Forward a char                                                          |
| `Ctrl-e`, `End`                             | Move prompt end                                                         |
| `Ctrl-a`, `Home`                            | Move prompt start                                                       |
| `Ctrl-w`, `Alt-Backspace`, `Ctrl-Backspace` | Delete previous word                                                    |
| `Alt-d`, `Alt-Delete`, `Ctrl-Delete`        | Delete next word                                                        |
| `Ctrl-u`                                    | Delete to start of line                                                 |
| `Ctrl-k`                                    | Delete to end of line                                                   |
| `Backspace`, `Ctrl-h`, `Shift-Backspace`    | Delete previous char                                                    |
| `Delete`, `Ctrl-d`                          | Delete next char                                                        |
| `Ctrl-s`                                    | Insert a word under doc cursor, may be changed to Ctrl-r Ctrl-w later   |
| `Ctrl-p`, `Up`                              | Select previous history                                                 |
| `Ctrl-n`, `Down`                            | Select next history                                                     |
| `Ctrl-r`                                    | Insert the content of the register selected by following input char     |
| `Tab`                                       | Select next completion item                                             |
| `BackTab`                                   | Select previous completion item                                         |
| `Enter`                                     | Open selected                                                           |
