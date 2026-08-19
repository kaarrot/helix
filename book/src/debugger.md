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
  the selection, or with the word under the cursor when nothing is selected.
  Since the sticky menu swallows motions, the mouse is the way to point at
  something without leaving the menu — drag over an expression, or click a word,
  then press `p`.
- **It remembers.** `Up` and `Down` walk previously evaluated expressions, and
  the history outlives the editor: it is kept in the `=` register and mirrored to
  `dap-eval-history` in Helix's cache directory, holding the most recent 500
  entries.
- **The whole result is yours to keep.** A status line only ever shows one line,
  so the complete value is copied to the system clipboard after every
  evaluation. Double-clicking the result on the status line copies it again.
- **It runs statements, not just expressions.** `count = 0`, `items.append(x)`
  or an import all work, and they change the frame you are stopped in — execution
  continues with the new values. A statement evaluates to nothing, so the status
  line reports `evaluated` rather than a value.

`:debug-eval <expression>` does the same from the command line.

Inside any prompt, `Ctrl-r` followed by a register name inserts that register:
`"` for the last yank, `*` for the primary selection (what a mouse drag just
selected), `+` for the system clipboard.

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
```

When a result comes back elided, Helix evaluates it once more through that
template, which returns the value as a plain string and so escapes the limit.
Note this evaluates your expression **a second time**, so an expression with
side effects performs them twice; it only happens for values that were actually
shortened.

Assignment works because Helix evaluates in the protocol's `repl` context, the
only one for which debugpy will execute a statement instead of merely evaluating
an expression. Assigning to a local, to a function argument, or mutating a local
container all take effect in the running frame.

[dap]: https://microsoft.github.io/debug-adapter-protocol/
[debugpy]: https://github.com/microsoft/debugpy
