## Debugger

Helix speaks the [Debug Adapter Protocol][dap], so debugging works the same way
for every language that has an adapter. Support is still experimental. The
adapter is configured per language in `languages.toml` (see
[Debugger configuration](./languages.md#debugger-configuration)); C, C++, C#,
Go, JavaScript, Odin, Python, Rust and Zig ship with one, and each of them still
expects you to install the adapter itself. The Python section at the end of this
page walks through one setup end to end.

Everything below is reached from the debug menu, `<space>G`.

> 💡 The debug menu is **sticky**: once you press `<space>G` it stays active
> until you press `Esc`. While it is up, keys are looked up in the menu alone —
> ordinary motions do nothing, and `j`, `b` or `c` run debug commands rather
> than moving, selecting or changing. Press `Esc` to get back to normal editing;
> the session keeps running.

### Starting a session

`<space>G l` lists the templates configured for the current language and starts
the one you pick. Templates may ask for parameters — a port, a process id, a
file — one prompt at a time, each opening pre-filled with the template's default
so you can press `<ret>` to accept it or type over it.

The same thing from the command line:

| Command                                         | Description                                            |
| ---                                             | ---                                                    |
| `:debug-start <template> [params…]`             | Start a template, passing its parameters as arguments  |
| `:debug-remote <host:port> <template> [params…]`| Connect to an adapter already listening at an address  |

Template names containing spaces need quoting, e.g. `:debug-start "Attach to PID" 12345`.

### Breakpoints

| Key      | Description                                        | Command                 |
| ---      | ---                                                | ---                     |
| `b`      | Toggle a breakpoint on the current line            | `dap_toggle_breakpoint` |
| `Ctrl-c` | Give the breakpoint a condition                    | `dap_edit_condition`    |
| `Ctrl-l` | Turn it into a log point                           | `dap_edit_log`          |

A conditional breakpoint stops only when its expression is true. A log point
does not stop at all — it prints its message and carries on.

Breakpoints can be set before a session starts; they are sent to the adapter
when it attaches.

### Controlling execution

| Key | Description                                           | Command          |
| --- | ---                                                   | ---              |
| `c` | Continue                                              | `dap_continue`   |
| `h` | Pause                                                 | `dap_pause`      |
| `i` | Step in                                               | `dap_step_in`    |
| `o` | Step out                                              | `dap_step_out`   |
| `n` | Step over                                             | `dap_next`       |
| `j` | Jump execution to the current line, without running what lies between | `dap_goto_line` |
| `r` | Restart the session                                   | `dap_restart`    |
| `t` | Terminate the session                                 | `dap_terminate`  |

With several threads or a deep stack, `s t` switches thread and `s f` switches
stack frame; the variables and evaluation below always apply to the frame you
have selected.

### Inspecting and changing state

`<space>G v` lists the variables of the current frame in a picker, grouped by
scope. Looking does not require Enter. Enter opens a `value:` prompt, pre-filled
with the current value, and assigns it through the adapter (`setExpression` when
the variable has an evaluate name, otherwise `setVariable`). The picker then
reopens with the new value. This only edits names that already exist in the
frame; creating a new name, or running any other statement, is what eval is for.

`<space>G p` opens the `eval:` prompt, which evaluates an expression in the
current frame. It is the most useful key in the mode, and it does a little more
than it looks:

- **It starts from what you are pointing at.** The prompt opens pre-filled with
  the selection, when there is one. Since the sticky menu swallows motions, the
  mouse is the way to point at something without leaving the menu — drag over an
  expression, then press `p`. With no selection the prompt opens empty, ready
  for `Up`.
- **It remembers.** `Up` and `Down` walk previously evaluated expressions, and
  the history outlives the editor: it is kept in the `=` register and mirrored to
  `dap-eval-history` in Helix's cache directory, holding the most recent 500
  entries.
- **The whole result is yours to keep.** A status line only ever shows one line,
  so the complete value is copied to the system clipboard after every
  evaluation. Double-clicking the result on the status line copies it again and
  also writes it to the [debug console](#debug-console), putting the console on
  screen if it is not showing — a long list is only useful once you can scroll
  and search it. While the console is open, every evaluation lands there anyway.
- **It runs statements, not just expressions.** `count = 0`, `items.append(x)`
  or an import all work, and they change the frame you are stopped in — execution
  continues with the new values. An assignment evaluates to nothing, so its
  target is read back and reported instead: `count = 0` shows `0`. Any other
  statement leaves the status line reporting `evaluated`.

`:debug-eval <expression>` does the same from the command line.

Inside any prompt, `Ctrl-r` followed by a register name inserts that register:
`"` for the last yank, `*` for the primary selection (what a mouse drag just
selected), `+` for the system clipboard.

### Debug console

The debug console works like a terminal pdb session: a buffer whose earlier
output stays above for scrolling, searching and yanking, and whose last line,
after the `(hx)` prompt, takes the next command. Stepping, jumping and
inspecting all happen from there, while the source view beside it follows the
current frame and stops at the breakpoints set in the gutter.

`Ctrl-d` in the debug menu opens it in a split under the source, ready to type.
The same key from inside the console closes it again and puts you back in the
debug menu; the transcript is kept for next time. From the command line,
`:debug-console` opens it and `:debug-console-toggle` opens or closes it the way
the key does. `:` works from the debug menu, so neither needs you to leave it. A
toggle rebound to a plain character, e.g. `[keys.normal.space.G] x = "dap_console_toggle"`, types
that character on the input line and closes the console only from normal mode.

Using the console does not leave debug mode: while it has focus, the debug menu
is set aside so keys type and move as usual, and once focus leaves the console
the menu is back as you left it — for `s t` to switch thread, for instance. The
debug menu can also be opened from inside the console with `<space>G`; `Esc`
closes it again.

The console is an ordinary buffer with Helix's modes. Only opening it with the
toggle or `:debug-console` puts you in insert mode. Reaching it any other way,
with `Ctrl-w j` or a mouse click, or pressing `Esc` while typing, leaves you in
normal mode, where keys are editor commands: `n` searches, `x` selects a line.
Press `i` or `a` to type; the cursor moves to the end of the `(hx)` line first.

| Key, in the console                         | Does                                                          |
| ---                                         | ---                                                           |
| `<ret>`                                     | Run the input line. Output appears below it, then a new prompt |
| `<ret>` on an empty line                    | Run the console's previous command again — `n` then `<ret> <ret> <ret>` |
| `Up`, `Down` (also `Ctrl-p`, `Ctrl-n`)      | Walk the history, shared with the eval prompt                 |
| `Tab`                                       | Complete from the debugger in the selected frame — `p obj.` then `Tab` lists `obj`'s members. Nothing pops up on its own |
| `Esc`                                       | Normal mode inside the console, to move around the output     |
| `Esc` in normal mode                        | Back to the source, in the debug menu; the console stays open |
| `Ctrl-d`                                    | Close the console, back to the source in the debug menu. It takes the place of half-page down and delete-forward here |

Typing always happens on the input line: entering insert mode anywhere in the
transcript, or typing after scrolling up, moves the cursor to the end of the
input first. Pasting does too, and reads as typing: each line break in the
pasted text runs the line so far, as it would in a terminal, and what follows
the last one stays on the input line. Output that arrives while you type — what
the program prints, or where it stopped — goes above the input line and leaves
what you were typing alone.

An empty line only repeats what the console itself last ran, never an entry of
the history, which is shared with the eval prompt and kept between sessions.

The console is never counted as unsaved, so it does not hold up `:q`, and it
survives having another file opened in its window; `Ctrl-d` brings it back with
its transcript.

The console understands pdb's commands, plus `i` and `o` for step in and out as
in the debug menu:

| Command                                  | Does                                                       |
| ---                                      | ---                                                        |
| `n`, `next`                              | Step over                                                  |
| `s`, `step`, `i`                         | Step in                                                    |
| `r`, `return`, `o`                       | Step out                                                   |
| `c`, `cont`, `continue`                  | Continue                                                   |
| `j N`, `jump N`                          | Jump execution to line `N` of the current frame's file     |
| `u [N]`, `up` / `d [N]`, `down`          | Select the frame `N` levels out (callers) or in            |
| `f N`, `frame N`                         | Select frame `N`, as numbered by `w`                       |
| `w`, `where`, `bt`                       | Show the stack, the selected frame marked `>`              |
| `b`                                      | List breakpoints                                           |
| `b N`, `b file:N`                        | Toggle a breakpoint on line `N`, saying where the debugger put it |
| `?`, `??`                                | The selected frame's local variables; `??` every scope     |
| `expr?`, `expr??`                        | The members of a value, one level; `??` adds special and function members |
| `p expr`, `pp expr`                      | Evaluate; `pp` lays the value out over several lines       |
| `q`, `quit`, `exit`, `quit()`, `exit()`  | End the session by disconnecting: a launched program is stopped, an attached one keeps running |
| `!stmt`                                  | Evaluate even what looks like a command, e.g. `!n` for a variable `n` |
| anything else                            | Evaluate or execute it in the selected frame               |

A command word followed by anything else is Python, not a command: `n = 3`
assigns and `c.x` reads an attribute. `b` and `b file:N` work before a session
starts, like the gutter; the debugger may then move a breakpoint to a line it
can stop on, and `b N` reports the line it landed on.

Everything runs in the selected frame, so `u` then `?` shows the caller's
variables. Values are printed whole — the same retry as the eval prompt lifts
debugpy's elision — and so is a failed evaluation's traceback. Reading and
changing state:

- `x`, `obj.attr`, `xs[3:10]` read a value; `x = 5`, `obj.attr = 5`,
  `xs[0] += 1` change the running frame and print what was stored;
  `items.append(x)` or `obj.reset()` run with real side effects.
- A new variable, `tmp = f(x)`, is kept with the frame, so later console input
  sees it. The running function does not: it only knows the names it was
  compiled with. At module level it becomes a real global. To hand a value to
  the program, assign to an existing name, an attribute, an item or
  `globals()['name']`.
- A member listing shows each value as the debugger formats it, which can elide
  a long collection. Evaluate the member itself, `obj.tags`, to see all of it.

With debugpy the console also shows what the program prints while it is open,
since the Python templates set `redirectOutput`. Output is gathered and written
once per frame, so a chatty program does not slow the editor down. Without a
console, the program's output is not shown on the status line, where it would
bury the debugger's own messages; it still goes to the program's terminal.

### Exceptions

`e` asks the adapter to break on exceptions, `E` turns that off again. Which
exception categories exist is up to the adapter.

## Python

Python is the one language with debug templates built in. They use
[debugpy][debugpy], which you need to install yourself:

```sh
pip install debugpy
```

Two templates ship in `languages.toml`:

**Attach to PID** attaches to a Python process that is already running, by PID
or by process name. Helix starts `python3 -m debugpy.adapter` and the adapter
injects itself into the target, which on Linux needs ptrace permission —
`kernel.yama.ptrace_scope = 0` or `CAP_SYS_PTRACE`. Giving a name rather than a
number looks it up, and refuses ambiguous matches with a list of candidates.

**Attach to port** connects to a debugpy server that your program started
itself. Instrument it with:

```python
import debugpy
debugpy.listen(("127.0.0.1", 5678))
debugpy.wait_for_client()
```

then pick the template and accept the offered port, `5678`, or type another one.
`wait_for_client()` returns as soon as Helix attaches.

### debugpy behavior worth knowing

Values you see come from debugpy's formatting, not Helix's, and it shortens
them. Long strings and — more visibly — collections come back with an ellipsis
in the middle, showing only their first entries. No part of the protocol turns
that off for collections, so the Python configuration carries a quirk:

```toml
[language.debugger.quirks]
full-value-expression = "repr({})"
pretty-value-expression = "..."
repl-statements = true
```

When a result comes back elided, Helix evaluates it once more through that
template, which returns the value as a plain string and so escapes the limit.
Note this evaluates your expression **a second time**, so an expression with
side effects performs them twice; it only happens for values that were actually
shortened.

`pretty-value-expression` is what `pp` in the debug console evaluates instead of
the expression itself: the bundled one lays values out with `pprint`, and numpy
arrays with `numpy.array2string` without numpy's own elision of large arrays.
Other numpy objects, such as a random generator or a dtype, go to `pprint`.

`repl-statements` says the adapter runs statements in the `repl` context, as
debugpy does. For adapters that cannot, a failed plain assignment such as
`x = 5` is retried as the protocol's `setExpression`; with this quirk it is not,
since the failure may come after the right-hand side ran, and a retry would run
it — and its side effects — again.

If you override `[language.debugger]` in your own `languages.toml`, **repeat the
quirk there**. Configuration merging stops at that table, so your block replaces
the bundled one wholesale and drops any key you leave out — including `quirks`.
The symptom is an ellipsis that survives into the clipboard and the result
buffer, and the status line says so when it happens.

Assignment works because Helix evaluates in the protocol's `repl` context, the
only one for which debugpy will execute a statement instead of merely evaluating
an expression. Assigning to a local, to a function argument, or mutating a local
container all take effect in the running frame.

[dap]: https://microsoft.github.io/debug-adapter-protocol/
[debugpy]: https://github.com/microsoft/debugpy
