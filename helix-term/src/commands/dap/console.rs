//! The debug console: a buffer that reads like a terminal pdb session. Earlier
//! output stays above to scroll, search and yank; the last line takes the next
//! command -- a pdb-style step or stack command, or any expression or statement to
//! run in the selected frame.
//!
//! Only `<ret>`, history and completion on the input line are the console's own;
//! everything else is ordinary editing, hooked up in `ui::EditorView`.

use super::{
    eval_complete, evaluate, goto_line, load_eval_history, save_eval_history, select_frame,
    selected_frame_id, step, toggle_breakpoint, unquote, Step, EVAL_HISTORY_REGISTER,
};
use crate::commands::Context;

use helix_core::{self as core, Selection, Transaction};
use helix_dap as dap;
use helix_lsp::block_on;
use helix_view::handlers::dap_console::{input_line, input_line_start, input_start, PROMPT};
use helix_view::{document::Mode, Editor};

use anyhow::{anyhow, bail};
use std::fmt::Write as _;
use std::path::PathBuf;

/// What a line typed into the console asks for.
#[derive(Debug, PartialEq, Eq)]
enum ConsoleCommand<'a> {
    Move(Step),
    /// Move execution to a line (1-based) of the selected frame's file.
    Jump(usize),
    /// Select a frame this many levels further out (callers) or in.
    Up(usize),
    Down(usize),
    /// Select a frame by its number in `where`.
    Frame(usize),
    Where,
    Breakpoints,
    /// Toggle a breakpoint on a line (1-based), in `file` or the selected frame's.
    ToggleBreakpoint {
        file: Option<&'a str>,
        line: usize,
    },
    /// The selected frame's variables: its innermost scope, or every scope.
    Variables {
        all: bool,
    },
    /// One level of a value's children, optionally with the adapter's grouped
    /// special and function members.
    Members {
        expression: &'a str,
        all: bool,
    },
    Eval(&'a str),
    Pretty(&'a str),
    /// End the session: pdb's `q`, and Python's `quit()` and `exit()`, which would
    /// otherwise raise `SystemExit` in the program and never answer.
    Quit,
}

/// Reads a console line. The pdb commands are only commands when they stand
/// alone or take the argument they expect: `n` steps, but `n = 3` assigns and
/// `c.x` reads an attribute. `!` forces anything to be evaluated.
fn parse_console_command(input: &str) -> ConsoleCommand<'_> {
    use ConsoleCommand::*;

    let input = input.trim();
    if let Some(statement) = input.strip_prefix('!') {
        return Eval(statement.trim());
    }

    let (word, rest) = match input.split_once(char::is_whitespace) {
        Some((word, rest)) => (word, rest.trim()),
        None => (input, ""),
    };
    let count = |rest: &str| match rest {
        "" => Some(1),
        rest => rest.parse::<usize>().ok(),
    };

    match (word, rest) {
        ("n" | "next", "") => return Move(Step::Over),
        ("s" | "step" | "i", "") => return Move(Step::In),
        ("r" | "return" | "o", "") => return Move(Step::Out),
        ("c" | "cont" | "continue", "") => return Move(Step::Continue),
        ("w" | "where" | "bt", "") => return Where,
        ("b" | "break", "") => return Breakpoints,
        ("q" | "quit" | "exit" | "quit()" | "exit()", "") => return Quit,
        ("?", "") => return Variables { all: false },
        ("??", "") => return Variables { all: true },
        ("u" | "up", rest) if count(rest).is_some() => return Up(count(rest).unwrap()),
        ("d" | "down", rest) if count(rest).is_some() => return Down(count(rest).unwrap()),
        ("f" | "frame", rest) if rest.parse::<usize>().is_ok() => {
            return Frame(rest.parse().unwrap())
        }
        ("j" | "jump", rest) if rest.parse::<usize>().is_ok() => {
            return Jump(rest.parse().unwrap())
        }
        ("b" | "break", rest) => {
            let (file, line) = match rest.rsplit_once(':') {
                Some((file, line)) => (Some(file.trim()), line),
                None => (None, rest),
            };
            if let Ok(line) = line.trim().parse() {
                return ToggleBreakpoint { file, line };
            }
        }
        ("p", rest) if !rest.is_empty() => return Eval(rest),
        ("pp", rest) if !rest.is_empty() => return Pretty(rest),
        _ => (),
    }

    // A trailing `?` is never valid Python, so it cannot shadow an expression.
    if let Some(expression) = input.strip_suffix("??") {
        if !expression.trim().is_empty() {
            return Members {
                expression: expression.trim(),
                all: true,
            };
        }
    }
    if let Some(expression) = input.strip_suffix('?') {
        if !expression.trim().is_empty() {
            return Members {
                expression: expression.trim(),
                all: false,
            };
        }
    }

    Eval(input)
}

/// `Ctrl-d` from the debug menu: show the console, focused and ready to type on
/// the input line. From inside the console: hide it and return to the source.
pub fn dap_console_toggle(cx: &mut Context) {
    toggle(cx.editor);
}

/// `:debug-console-toggle`, and the body of `dap_console_toggle`.
pub(crate) fn toggle(editor: &mut Editor) {
    if editor.is_dap_console_focused() {
        editor.dap_console_hide();
    } else {
        open(editor);
    }
}

/// `:debug-console`: show the console, focused and ready to type on the input
/// line, whether or not it is already on screen.
pub(crate) fn open(editor: &mut Editor) {
    let created = editor.dap_console_doc().is_none();
    editor.dap_console_show(true);
    load_eval_history(editor);
    if created && editor.debug_adapters.get_active_client().is_none() {
        editor
            .dap_console_print("*** No debug session: start one from the debug menu (<space>G l)");
    }

    let (view, doc) = current!(editor);
    doc.set_selection(view.id, Selection::point(doc.text().len_chars()));
    editor.mode = Mode::Insert;
}

/// Normal-mode `Esc` in the console: back to the source of the current frame,
/// leaving the console on screen.
pub(crate) fn leave(editor: &mut Editor) {
    if let Some(view_id) = editor.dap_frame_view() {
        editor.focus(view_id);
    }
}

/// Keeps typing on the input line: a cursor elsewhere -- up in the transcript, or
/// inside the prompt -- is moved to the end of the input first.
pub(crate) fn snap_to_input(editor: &mut Editor) {
    let (view, doc) = current!(editor);
    let text = doc.text().slice(..);
    let cursor = doc.selection(view.id).primary().cursor(text);
    if cursor < input_start(text) {
        doc.set_selection(view.id, Selection::point(text.len_chars()));
    }
}

/// Whether the cursor is on the input line, where `Up`, `Down` and `Tab` belong to
/// the console rather than to editing.
pub(crate) fn on_input_line(editor: &Editor) -> bool {
    let (view, doc) = current_ref!(editor);
    let text = doc.text().slice(..);
    doc.selection(view.id).primary().cursor(text) >= input_line_start(text)
}

/// Replaces what is typed after the prompt with `input`, cursor at its end.
fn set_input(editor: &mut Editor, input: &str) {
    let (view, doc) = current!(editor);
    let text = doc.text();
    let start = input_start(text.slice(..));
    let end = text.len_chars();
    let transaction = Transaction::change(text, [(start, end, Some(input.into()))].into_iter())
        .with_selection(Selection::point(start + input.chars().count()));
    doc.apply(&transaction, view.id);
}

/// `Up`/`Down` on the input line: walk the evaluation history, shared with the
/// eval prompt. Walking past the newest entry clears the line again.
pub(crate) fn history(editor: &mut Editor, older: bool) {
    let values: Vec<String> = editor
        .registers
        .read(EVAL_HISTORY_REGISTER, editor)
        .map(|values| values.map(|value| value.into_owned()).collect())
        .unwrap_or_default();
    if values.is_empty() {
        return;
    }

    let pos = match (editor.dap_console.history_pos, older) {
        (None, true) => Some(0),
        (Some(pos), true) => Some((pos + 1).min(values.len() - 1)),
        (None, false) => return,
        (Some(0), false) => None,
        (Some(pos), false) => Some(pos - 1),
    };
    editor.dap_console.history_pos = pos;
    set_input(editor, pos.map_or("", |pos| values[pos].as_str()));
}

/// `Tab` on the input line: what the adapter offers to complete the input at the
/// cursor -- `obj.` lists `obj`'s members -- as completion-menu items.
pub(crate) fn completions(editor: &Editor) -> anyhow::Result<Vec<core::CompletionItem>> {
    let debugger = editor
        .debug_adapters
        .get_active_client()
        .ok_or_else(|| anyhow!("Debugger is not running"))?;
    if !debugger.supports_completions() {
        bail!("The debug adapter does not offer completions");
    }
    let frame_id = selected_frame_id(debugger).ok();

    let (view, doc) = current_ref!(editor);
    let text = doc.text().slice(..);
    let start = input_start(text);
    let cursor = doc.selection(view.id).primary().cursor(text).max(start);
    // The protocol counts columns in UTF-16 code units.
    let input = text.slice(start..);
    let typed_units = text.slice(start..cursor).len_utf16_cu();
    let char_at = |units: usize| start + input.utf16_cu_to_char(units.min(input.len_utf16_cu()));

    let targets = block_on(debugger.completions(input.to_string(), typed_units + 1, frame_id))
        .map_err(|err| anyhow!("{err}"))?;

    let mut items: Vec<_> = targets
        .into_iter()
        .map(|target| {
            // `start` counts from 0 whatever the client asked columns to start at:
            // that is how VS Code reads it, and so how debugpy sends it. Without a
            // start, `length` counts back from the cursor over what is replaced.
            let length = target.length.unwrap_or(0);
            let (from, to) = match target.start {
                Some(target_start) => (char_at(target_start), char_at(target_start + length)),
                None => (char_at(typed_units.saturating_sub(length)), cursor),
            };
            let insert = target.text.unwrap_or_else(|| target.label.clone());
            let transaction =
                Transaction::change(doc.text(), [(from, to, Some(insert.into()))].into_iter());
            core::CompletionItem {
                transaction,
                label: target.label.into(),
                kind: target.ty.unwrap_or_default().into(),
                documentation: target.detail,
                provider: core::completion::CompletionProvider::Debugger,
            }
        })
        .collect();
    // Data members before private ones, and dunders last: debugpy lists them
    // alphabetically, which puts `__class__` and friends first.
    items.sort_by_key(|item| (item.label.starts_with("__"), item.label.starts_with('_')));
    Ok(items)
}

/// `<ret>` on the console: run what is on the input line, as pdb would. An empty
/// line runs the previous command again.
pub(crate) fn submit(cx: &mut Context) {
    let typed = {
        let doc = doc!(cx.editor);
        input_line(doc.text().slice(..))
    };

    // An empty line runs again what was last run here -- never an entry of the
    // shared history, which may come from the eval prompt or an earlier day.
    let input = match typed.is_empty() {
        false => {
            let newest = cx.editor.registers.first(EVAL_HISTORY_REGISTER, cx.editor);
            if newest.as_deref() != Some(typed.as_str()) {
                if let Err(err) = cx
                    .editor
                    .registers
                    .push(EVAL_HISTORY_REGISTER, typed.clone())
                {
                    cx.editor.set_error(err.to_string());
                }
                save_eval_history(cx.editor);
            }
            typed
        }
        true => match cx.editor.dap_console.last_command.clone() {
            Some(last) => last,
            None => return,
        },
    };
    cx.editor.dap_console.history_pos = None;
    cx.editor.dap_console.last_command = Some(input.clone());

    // The input line becomes the transcript's record of the command, and a fresh
    // prompt takes its place. Output goes in between.
    {
        let (view, doc) = current!(cx.editor);
        let text = doc.text();
        let start = input_line_start(text.slice(..));
        let replacement = format!("{PROMPT}{input}\n{PROMPT}");
        let cursor = start + replacement.chars().count();
        let transaction = Transaction::change(
            text,
            [(start, text.len_chars(), Some(replacement.into()))].into_iter(),
        )
        .with_selection(Selection::point(cursor));
        doc.apply(&transaction, view.id);
    }

    run(cx, &input);
}

/// Text pasted into the console goes onto the input line, wherever the cursor was,
/// and reads as if typed: every line break runs the line so far, as in a terminal.
/// What follows the last break stays on the input line.
pub(crate) fn paste(cx: &mut Context, text: &str) {
    let text = text.replace("\r\n", "\n");
    let mut lines = text.split('\n').peekable();
    while let Some(line) = lines.next() {
        insert_at_input(cx.editor, line);
        if lines.peek().is_some() {
            submit(cx);
        }
    }
}

/// Inserts `text` on the input line: at the cursor when typing there, else at its
/// end.
fn insert_at_input(editor: &mut Editor, text: &str) {
    if text.is_empty() {
        return;
    }
    let insert_mode = editor.mode() == helix_view::document::Mode::Insert;
    let (view, doc) = current!(editor);
    let doc_text = doc.text().slice(..);
    let cursor = doc.selection(view.id).primary().cursor(doc_text);
    let at = match insert_mode && cursor >= input_start(doc_text) {
        true => cursor,
        false => doc_text.len_chars(),
    };
    let transaction = Transaction::change(doc.text(), [(at, at, Some(text.into()))].into_iter())
        .with_selection(Selection::point(at + text.chars().count()));
    doc.apply(&transaction, view.id);
}

/// Runs a console line and writes what it has to say to the transcript.
fn run(cx: &mut Context, input: &str) {
    use ConsoleCommand::*;

    let command = parse_console_command(input);
    // Breakpoints can be set before a session, as in the gutter.
    let needs_session = !matches!(command, Breakpoints | ToggleBreakpoint { .. });
    if needs_session && cx.editor.debug_adapters.get_active_client().is_none() {
        cx.editor
            .dap_console_print("*** No debug session: start one from the debug menu (<space>G l)");
        return;
    }

    let editor = &mut *cx.editor;
    let result: anyhow::Result<Option<String>> = match command {
        Move(kind) => step(editor, cx.jobs, kind).map(|_| None),
        Jump(line) => line
            .checked_sub(1)
            .ok_or_else(|| anyhow!("Lines are numbered from 1"))
            .and_then(|line| {
                // The debugger's own path, which is what it compares against.
                let path = frame_path(editor)?;
                goto_line(editor, cx.jobs, path, line)
            })
            .map(|_| None),
        Up(levels) => move_frame(editor, FrameMove::Out(levels)),
        Down(levels) => move_frame(editor, FrameMove::In(levels)),
        Frame(index) => select_frame(editor, index).map(|_| {
            editor.dap_console_print_location();
            None
        }),
        Where => stack(editor).map(Some),
        Breakpoints => Ok(Some(list_breakpoints(editor))),
        ToggleBreakpoint { file, line } => set_breakpoint(editor, file, line).map(Some),
        Variables { all } => frame_variables(editor, all).map(Some),
        Members { expression, all } => members(editor, expression, all).map(Some),
        Eval(expression) => evaluate(editor, expression),
        Pretty(expression) => pretty(editor, expression).map(Some),
        Quit => quit(editor).map(Some),
    };

    let output = match result {
        Ok(Some(output)) => output,
        Ok(None) => return,
        Err(err) => format!("*** {err}"),
    };
    editor.dap_console_print(&output);
}

/// `q`: disconnect, ending the session. What becomes of the program is the
/// adapter's call, as the protocol has it: one it launched is stopped, one it
/// attached to is left running.
fn quit(editor: &mut Editor) -> anyhow::Result<String> {
    let debugger = editor
        .debug_adapters
        .get_active_client_mut()
        .ok_or_else(|| anyhow!("Debugger is not running"))?;
    let id = debugger.id();
    let request = debugger.disconnect(Some(dap::requests::DisconnectArguments {
        restart: Some(false),
        terminate_debuggee: None,
        suspend_debuggee: None,
    }));
    match block_on(request) {
        // An adapter that went away with the program has nothing left to say.
        Ok(_) | Err(dap::Error::StreamClosed) => (),
        Err(err) => bail!("{err}"),
    }

    editor.debug_adapters.remove_client(id);
    editor.debug_adapters.unset_active_client();
    for breakpoint in editor.breakpoints.values_mut().flatten() {
        breakpoint.verified = false;
    }
    Ok("Disconnected: the debug session is over".to_string())
}

/// The selected frame's source file.
fn frame_path(editor: &Editor) -> anyhow::Result<PathBuf> {
    editor
        .current_stack_frame()
        .and_then(|frame| frame.source.as_ref()?.path.clone())
        .ok_or_else(|| anyhow!("The selected frame has no source file"))
}

/// How far `u` or `d` asks to move from the selected frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameMove {
    /// Towards the callers, `u`.
    Out(usize),
    /// Towards the innermost frame, `d`.
    In(usize),
}

/// The frame a move lands on, in a stack `depth` frames deep. Frame 0 is the
/// innermost; moves past either end are refused, as pdb does.
fn frame_after(current: usize, depth: usize, by: FrameMove) -> anyhow::Result<usize> {
    match by {
        FrameMove::Out(levels) => current
            .checked_add(levels)
            .filter(|&target| target < depth)
            .ok_or_else(|| anyhow!("Oldest frame")),
        FrameMove::In(levels) => current
            .checked_sub(levels)
            .ok_or_else(|| anyhow!("Newest frame")),
    }
}

/// `u`/`d`: select a frame further out or in, and say where it is.
fn move_frame(editor: &mut Editor, by: FrameMove) -> anyhow::Result<Option<String>> {
    let debugger = editor
        .debug_adapters
        .get_active_client()
        .ok_or_else(|| anyhow!("Debugger is not running"))?;
    let thread_id = debugger
        .thread_id
        .ok_or_else(|| anyhow!("No thread is currently stopped"))?;
    let depth = debugger
        .stack_frames
        .get(&thread_id)
        .map_or(0, |frames| frames.len());
    let current = debugger.active_frame.unwrap_or(0);

    let target = frame_after(current, depth, by)?;
    select_frame(editor, target)?;
    editor.dap_console_print_location();
    Ok(None)
}

/// `w`: the stopped thread's stack, innermost first, the selected frame marked.
fn stack(editor: &Editor) -> anyhow::Result<String> {
    let debugger = editor
        .debug_adapters
        .get_active_client()
        .ok_or_else(|| anyhow!("Debugger is not running"))?;
    let thread_id = debugger
        .thread_id
        .ok_or_else(|| anyhow!("No thread is currently stopped"))?;
    let frames = debugger
        .stack_frames
        .get(&thread_id)
        .ok_or_else(|| anyhow!("The stopped thread has no stack frames"))?;
    let selected = debugger.active_frame.unwrap_or(0);

    let mut out = String::new();
    for (i, frame) in frames.iter().enumerate() {
        let marker = if i == selected { '>' } else { ' ' };
        let path = frame
            .source
            .as_ref()
            .and_then(|source| source.path.as_ref())
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<unknown>".to_string());
        let _ = writeln!(
            out,
            "{marker} #{i:<2} {}  {}:{}",
            frame.name, path, frame.line
        );
    }
    Ok(out)
}

fn list_breakpoints(editor: &Editor) -> String {
    let mut paths: Vec<_> = editor
        .breakpoints
        .iter()
        .filter(|(_, breakpoints)| !breakpoints.is_empty())
        .collect();
    if paths.is_empty() {
        return "No breakpoints".to_string();
    }
    paths.sort_by(|a, b| a.0.cmp(b.0));

    let mut out = String::new();
    for (path, breakpoints) in paths {
        let mut breakpoints: Vec<_> = breakpoints.iter().collect();
        breakpoints.sort_by_key(|breakpoint| breakpoint.line);
        for breakpoint in breakpoints {
            let _ = write!(out, "{}:{}", path.display(), breakpoint.line + 1);
            if let Some(condition) = &breakpoint.condition {
                let _ = write!(out, " if {condition}");
            }
            if let Some(message) = &breakpoint.log_message {
                let _ = write!(out, " log {message}");
            }
            if !breakpoint.verified {
                out.push_str(" (unverified)");
            }
            out.push('\n');
        }
    }
    out
}

/// `b [file:]line`: toggle a breakpoint, in the selected frame's file unless a
/// file is named. Says where the debugger put it, which may be a nearby line it
/// can stop on.
fn set_breakpoint(editor: &mut Editor, file: Option<&str>, line: usize) -> anyhow::Result<String> {
    // Breakpoints are kept by the path of the buffer, which the gutter uses too;
    // the debugger's own path may differ, e.g. through a symlink.
    let path = match file {
        Some(file) => helix_stdx::path::canonicalize(file),
        None => helix_stdx::path::canonicalize(frame_path(editor)?),
    };
    if !path.is_file() {
        bail!("No such file: {}", path.display());
    }
    let line = line
        .checked_sub(1)
        .ok_or_else(|| anyhow!("Lines are numbered from 1"))?;

    let count = |editor: &Editor| editor.breakpoints.get(&path).map_or(0, Vec::len);
    let before = count(editor);
    toggle_breakpoint(editor, path.clone(), line)?;

    if count(editor) < before {
        return Ok(format!(
            "Breakpoint cleared at {}:{}",
            path.display(),
            line + 1
        ));
    }
    // A new breakpoint goes last, and the debugger's answer may have moved it.
    let set = editor
        .breakpoints
        .get(&path)
        .and_then(|breakpoints| breakpoints.last())
        .map_or(line, |breakpoint| breakpoint.line);
    // Moved onto a line that already had one: asking for line 10 again, after the
    // debugger put the first breakpoint on 11, would otherwise stack a second.
    let on_line = editor.breakpoints.get(&path).map_or(0, |breakpoints| {
        breakpoints.iter().filter(|b| b.line == set).count()
    });
    if on_line > 1 {
        toggle_breakpoint(editor, path.clone(), set)?;
        return Ok(format!(
            "Breakpoint already at {}:{}",
            path.display(),
            set + 1
        ));
    }
    let mut report = format!("Breakpoint set at {}:{}", path.display(), set + 1);
    if set != line {
        let _ = write!(report, " (asked for line {})", line + 1);
    }
    Ok(report)
}

/// debugpy gathers dunder, function and class members of an object into child
/// nodes of their own, such as "special variables".
fn is_group(var: &dap::Variable) -> bool {
    var.evaluate_name.is_none() && var.variables_reference > 0 && var.name.ends_with(" variables")
}

fn variable_row(var: &dap::Variable, indent: &str) -> String {
    match var.ty.as_deref().filter(|ty| !ty.is_empty()) {
        Some(ty) => format!("{indent}{}: {ty} = {}\n", var.name, var.value),
        None => format!("{indent}{} = {}\n", var.name, var.value),
    }
}

/// One row per variable. Groups are left out, or expanded one level when `all`.
fn variable_rows(debugger: &dap::Client, variables: Vec<dap::Variable>, all: bool) -> String {
    let mut out = String::new();
    for var in variables {
        if !is_group(&var) {
            out.push_str(&variable_row(&var, "  "));
        } else if all {
            let _ = writeln!(out, "  {}:", var.name);
            for child in block_on(debugger.variables(var.variables_reference)).unwrap_or_default() {
                out.push_str(&variable_row(&child, "    "));
            }
        }
    }
    out
}

/// `?`/`??`: the selected frame's variables.
fn frame_variables(editor: &Editor, all: bool) -> anyhow::Result<String> {
    let debugger = editor
        .debug_adapters
        .get_active_client()
        .ok_or_else(|| anyhow!("Debugger is not running"))?;
    let frame_id = selected_frame_id(debugger)?;
    let scopes = block_on(debugger.scopes(frame_id)).map_err(|err| anyhow!("{err}"))?;

    let mut out = String::new();
    for scope in scopes.into_iter().take(if all { usize::MAX } else { 1 }) {
        let variables = block_on(debugger.variables(scope.variables_reference))
            .map_err(|err| anyhow!("{err}"))?;
        let _ = writeln!(out, "{}:", scope.name);
        out.push_str(&variable_rows(debugger, variables, all));
    }
    Ok(out)
}

/// `expr?`/`expr??`: one level of what `expression` evaluates to.
fn members(editor: &Editor, expression: &str, all: bool) -> anyhow::Result<String> {
    let debugger = editor
        .debug_adapters
        .get_active_client()
        .ok_or_else(|| anyhow!("Debugger is not running"))?;
    let frame_id = selected_frame_id(debugger)?;
    let response = block_on(debugger.eval(expression.to_owned(), Some(frame_id)))
        .map_err(|err| anyhow!("{err}"))?;

    if response.variables_reference == 0 {
        return Ok(format!("{expression} has no members: {}", response.result));
    }

    let variables = block_on(debugger.variables(response.variables_reference))
        .map_err(|err| anyhow!("{err}"))?;
    let header = match response.ty.as_deref().filter(|ty| !ty.is_empty()) {
        Some(ty) => format!("{expression}: {ty}\n"),
        None => format!("{expression}:\n"),
    };
    Ok(header + variable_rows(debugger, variables, all).as_str())
}

/// `pp expr`: the value laid out over several lines, through the adapter's
/// `pretty-value-expression` quirk. Adapters without one print it as usual.
fn pretty(editor: &Editor, expression: &str) -> anyhow::Result<String> {
    let debugger = editor
        .debug_adapters
        .get_active_client()
        .ok_or_else(|| anyhow!("Debugger is not running"))?;
    let frame_id = selected_frame_id(debugger)?;

    let value = match debugger.quirks.pretty_value_expression.as_deref() {
        Some(template) => {
            let response =
                block_on(debugger.eval_full(template.replace("{}", expression), Some(frame_id)))
                    .map_err(|err| anyhow!("{err}"))?;
            unquote(&response.result).into_owned()
        }
        None => eval_complete(debugger, expression, frame_id).map_err(|err| anyhow!("{err}"))?,
    };
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::ConsoleCommand::*;
    use super::*;

    #[test]
    fn pdb_commands_stand_alone() {
        assert_eq!(parse_console_command("n"), Move(Step::Over));
        assert_eq!(parse_console_command(" next "), Move(Step::Over));
        assert_eq!(parse_console_command("i"), Move(Step::In));
        assert_eq!(parse_console_command("s"), Move(Step::In));
        assert_eq!(parse_console_command("o"), Move(Step::Out));
        assert_eq!(parse_console_command("r"), Move(Step::Out));
        assert_eq!(parse_console_command("c"), Move(Step::Continue));
        assert_eq!(parse_console_command("w"), Where);
        assert_eq!(parse_console_command("q"), Quit);
        assert_eq!(parse_console_command("quit()"), Quit);
        assert_eq!(parse_console_command(" exit() "), Quit);
        assert_eq!(parse_console_command("q = 3"), Eval("q = 3"));
        assert_eq!(parse_console_command("!quit()"), Eval("quit()"));
        assert_eq!(parse_console_command("u"), Up(1));
        assert_eq!(parse_console_command("u 2"), Up(2));
        assert_eq!(parse_console_command("d 3"), Down(3));
        assert_eq!(parse_console_command("f 3"), Frame(3));
        assert_eq!(parse_console_command("j 42"), Jump(42));
        assert_eq!(parse_console_command("b"), Breakpoints);
        assert_eq!(
            parse_console_command("b 10"),
            ToggleBreakpoint {
                file: None,
                line: 10
            }
        );
        assert_eq!(
            parse_console_command("b src/app.py:10"),
            ToggleBreakpoint {
                file: Some("src/app.py"),
                line: 10
            }
        );
    }

    #[test]
    fn anything_else_is_evaluated() {
        // A command word with more after it is Python, not a command.
        assert_eq!(parse_console_command("n = 3"), Eval("n = 3"));
        assert_eq!(parse_console_command("c.x"), Eval("c.x"));
        assert_eq!(parse_console_command("u x"), Eval("u x"));
        assert_eq!(parse_console_command("j"), Eval("j"));
        assert_eq!(parse_console_command("f"), Eval("f"));
        assert_eq!(parse_console_command("p"), Eval("p"));
        // `!` forces evaluation of what would be a command.
        assert_eq!(parse_console_command("!n"), Eval("n"));
        assert_eq!(parse_console_command("! c"), Eval("c"));
        // `p`/`pp` evaluate what follows.
        assert_eq!(parse_console_command("p xs[0]"), Eval("xs[0]"));
        assert_eq!(parse_console_command("pp xs"), Pretty("xs"));
        assert_eq!(
            parse_console_command("items.append(x)"),
            Eval("items.append(x)")
        );
        assert_eq!(parse_console_command(""), Eval(""));
    }

    #[test]
    fn frame_moves_stop_at_either_end() {
        use super::FrameMove::{In, Out};

        assert_eq!(frame_after(0, 3, Out(1)).unwrap(), 1);
        assert_eq!(frame_after(1, 3, Out(1)).unwrap(), 2);
        assert_eq!(frame_after(2, 3, In(2)).unwrap(), 0);
        assert_eq!(
            frame_after(2, 3, Out(1)).unwrap_err().to_string(),
            "Oldest frame"
        );
        assert_eq!(
            frame_after(0, 3, In(1)).unwrap_err().to_string(),
            "Newest frame"
        );
        // Counts too large for any stack are refused, not wrapped around.
        assert!(frame_after(1, 3, Out(usize::MAX)).is_err());
        assert!(frame_after(1, 3, In(usize::MAX)).is_err());
    }

    #[test]
    fn question_marks_ask_for_members() {
        assert_eq!(parse_console_command("?"), Variables { all: false });
        assert_eq!(parse_console_command("??"), Variables { all: true });
        assert_eq!(
            parse_console_command("item?"),
            Members {
                expression: "item",
                all: false
            }
        );
        assert_eq!(
            parse_console_command("item.child ??"),
            Members {
                expression: "item.child",
                all: true
            }
        );
        assert_eq!(
            parse_console_command("xs[0]?"),
            Members {
                expression: "xs[0]",
                all: false
            }
        );
        // A question mark inside the expression is left to the evaluator.
        assert_eq!(
            parse_console_command("s.endswith('?')"),
            Eval("s.endswith('?')")
        );
    }
}
