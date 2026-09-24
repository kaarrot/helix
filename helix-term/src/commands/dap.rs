use super::{Context, Editor};
use crate::{
    compositor::{self, Compositor},
    job::{Callback, Jobs},
    ui::{self, overlay::overlaid, Picker, Prompt, PromptEvent},
};
use dap::{StackFrame, Thread, ThreadStates};
use helix_core::syntax::config::{DebugArgumentValue, DebugConfigCompletion, DebugTemplate};
use helix_core::{Range, RopeSlice, Selection, Transaction};
use helix_dap::{self as dap, requests::TerminateArguments};
use helix_lsp::block_on;
use helix_view::editor::{Action, Breakpoint, DapEvalResult};
use helix_view::DocumentId;

use serde_json::{to_value, Value};

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::path::PathBuf;

use anyhow::{anyhow, bail};

use helix_view::handlers::dap::{breakpoints_changed, jump_to_stack_frame, select_thread_id};

fn thread_picker(
    cx: &mut Context,
    callback_fn: impl Fn(&mut Editor, &dap::Thread) + Send + 'static,
) {
    let debugger = debugger!(cx.editor);

    let future = debugger.threads();
    dap_callback(
        cx.jobs,
        future,
        move |editor, compositor, response: dap::requests::ThreadsResponse| {
            let threads = response.threads;
            if threads.len() == 1 {
                callback_fn(editor, &threads[0]);
                return;
            }
            let debugger = debugger!(editor);

            let thread_states = debugger.thread_states.clone();
            let columns = [
                ui::PickerColumn::new("name", |item: &Thread, _| item.name.as_str().into()),
                ui::PickerColumn::new("state", |item: &Thread, thread_states: &ThreadStates| {
                    thread_states
                        .get(&item.id)
                        .map(|state| state.as_str())
                        .unwrap_or("unknown")
                        .into()
                }),
            ];
            let picker = Picker::new(
                columns,
                0,
                threads,
                thread_states,
                move |cx, thread, _action| callback_fn(cx.editor, thread),
            )
            .with_preview(move |editor, thread| {
                let frames = editor
                    .debug_adapters
                    .get_active_client()
                    .as_ref()?
                    .stack_frames
                    .get(&thread.id)?;
                let frame = frames.first()?;
                let path = frame.source.as_ref()?.path.as_ref()?.as_path();
                let pos = Some((
                    frame.line.saturating_sub(1),
                    frame.end_line.unwrap_or(frame.line).saturating_sub(1),
                ));
                Some((path.into(), pos))
            });
            compositor.push(Box::new(picker));
        },
    );
}

/// Prompt history for "process" parameters. It holds process names only (see
/// `remember_process_name`), so Up/Down recall e.g. `hython-bin`.
const PROCESS_HISTORY_REGISTER: char = '>';

/// A running process a "process" parameter can refer to.
struct ProcessInfo {
    pid: u32,
    /// Seconds since the process started.
    age: u64,
    argv: Vec<String>,
}

impl ProcessInfo {
    /// Completion entry: PID, age and the arguments after the executable,
    /// which for an interpreter start with the script it runs.
    fn summary(&self) -> String {
        let args = self.argv.get(1..).filter(|args| !args.is_empty());
        let args = args.unwrap_or(&self.argv).join(" ");
        format!("{}  {} ago  {}", self.pid, format_age(self.age), args)
    }

    /// The whole command line, one argument per line, a flag sharing its line
    /// with the value that follows it.
    fn describe(&self) -> String {
        let mut lines: Vec<String> = Vec::new();
        for arg in &self.argv {
            match lines.last_mut() {
                Some(flag)
                    if flag.starts_with('-') && !flag.contains(' ') && !arg.starts_with('-') =>
                {
                    flag.push(' ');
                    flag.push_str(arg);
                }
                _ => lines.push(arg.clone()),
            }
        }
        format!(
            "PID {}, started {} ago\n{}",
            self.pid,
            format_age(self.age),
            lines.join("\n")
        )
    }
}

fn format_age(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86400),
    }
}

/// Reads `/proc/<pid>`: the start time in clock ticks since boot (0 when
/// unreadable), `comm`, and the process info.
#[cfg(target_os = "linux")]
fn read_process(pid: u32, uptime: u64) -> Option<(u64, String, ProcessInfo)> {
    use std::fs;

    // `/proc` times are in USER_HZ, which Linux fixes at 100 for userspace.
    const TICKS_PER_SEC: u64 = 100;

    let dir = format!("/proc/{pid}");
    let comm = fs::read_to_string(format!("{dir}/comm"))
        .ok()?
        .trim()
        .to_owned();

    // Field 22 of `stat`. `comm` is wrapped in `(...)` and may contain spaces
    // and parens, so count from the last `)`: starttime is the 20th token.
    let starttime = fs::read_to_string(format!("{dir}/stat"))
        .ok()
        .and_then(|stat| {
            let rest = &stat[stat.rfind(')')? + 1..];
            rest.split_whitespace().nth(19)?.parse().ok()
        })
        .unwrap_or(0);

    let mut argv: Vec<String> = fs::read(format!("{dir}/cmdline"))
        .unwrap_or_default()
        .split(|&c| c == 0)
        .filter_map(|seg| std::str::from_utf8(seg).ok())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    if argv.is_empty() {
        argv.push(comm.clone());
    }

    let age = uptime.saturating_sub(starttime / TICKS_PER_SEC);
    Some((starttime, comm, ProcessInfo { pid, age, argv }))
}

#[cfg(target_os = "linux")]
fn uptime_secs() -> u64 {
    std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next()?.parse::<f64>().ok())
        .unwrap_or(0.0) as u64
}

#[cfg(target_os = "linux")]
fn process_by_pid(pid: u32) -> Option<ProcessInfo> {
    read_process(pid, uptime_secs()).map(|(_, _, process)| process)
}

/// Processes whose `comm`, `argv[0]` basename, or first non-flag argument
/// (basename or stem) is `name`, most recently started first. The last two
/// cover interpreters, where `comm` is `python3` but the argument is the
/// script being run.
#[cfg(target_os = "linux")]
fn find_processes(name: &str) -> Vec<ProcessInfo> {
    use std::path::Path;

    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let own_pid = std::process::id();
    let uptime = uptime_secs();

    let mut matches: Vec<(u64, ProcessInfo)> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .filter(|&pid| pid != own_pid)
        .filter_map(|pid| read_process(pid, uptime))
        .filter(|(_, comm, process)| {
            let basename = |s: &str| Path::new(s).file_name()?.to_str().map(str::to_owned);
            let stem = |s: &str| Path::new(s).file_stem()?.to_str().map(str::to_owned);
            let arg0 = &process.argv[0];
            let script = process.argv.iter().skip(1).find(|s| !s.starts_with('-'));

            comm == name
                || basename(arg0).as_deref().unwrap_or(arg0) == name
                || script.and_then(|s| basename(s)).as_deref() == Some(name)
                || script.and_then(|s| stem(s)).as_deref() == Some(name)
        })
        .map(|(starttime, _, process)| (starttime, process))
        .collect();

    matches.sort_by(|a, b| b.0.cmp(&a.0));
    matches.into_iter().map(|(_, process)| process).collect()
}

#[cfg(not(target_os = "linux"))]
fn process_by_pid(_pid: u32) -> Option<ProcessInfo> {
    None
}

#[cfg(not(target_os = "linux"))]
fn find_processes(_name: &str) -> Vec<ProcessInfo> {
    Vec::new()
}

/// Splits a "process" prompt line into its name and PID. The line is a PID,
/// a name, or a name followed by the PID a Tab completion appended.
fn parse_process_input(input: &str) -> (Option<&str>, Option<u32>) {
    let mut tokens = input.split_whitespace();
    let first = tokens.next();
    match first.and_then(|t| t.parse().ok()) {
        Some(pid) => (None, Some(pid)),
        None => (first, tokens.next().and_then(|t| t.parse().ok())),
    }
}

/// Maps a process name to the most recently started process of that name.
fn resolve_process_name(name: &str) -> anyhow::Result<u32> {
    if cfg!(not(target_os = "linux")) {
        bail!("Process name resolution is only implemented on Linux; please enter a numeric PID");
    }
    match find_processes(name).first() {
        Some(process) => Ok(process.pid),
        None => bail!("No process found matching '{}'", name),
    }
}

/// Tab cycles through the processes matching the typed name, newest first.
/// Each entry is appended after the name, so the name stays on the line for
/// `remember_process_name`.
fn process_completer(_editor: &Editor, input: &str) -> Vec<ui::prompt::Completion> {
    let (Some(name), _) = parse_process_input(input) else {
        return Vec::new();
    };
    let start = input.find(name).unwrap_or(0) + name.len();
    find_processes(name)
        .iter()
        .map(|process| (start.., format!(" {}", process.summary()).into()))
        .collect()
}

/// Popup text for a "process" prompt: the full command line of the process
/// the line picks, or that Enter would pick.
fn process_doc(input: &str) -> Option<String> {
    match parse_process_input(input) {
        (_, Some(pid)) => process_by_pid(pid).map(|process| process.describe()),
        (Some(name), None) => {
            let matches = find_processes(name);
            let newest = matches.first()?;
            let header = match matches.len() {
                1 => "Enter attaches to".to_owned(),
                n => format!("Enter attaches to the newest of {n}, Tab cycles"),
            };
            Some(format!("{header}\n{}", newest.describe()))
        }
        (None, None) => None,
    }
}

/// The prompt stores the whole line in its history, and after a Tab
/// completion that carries a PID and command line that go stale. Rewrites
/// the history to the name alone, most recent first and without repeats.
fn remember_process_name(editor: &mut Editor, input: &str) {
    let (Some(name), _) = parse_process_input(input) else {
        return;
    };
    let mut history: Vec<String> = editor
        .registers
        .read(PROCESS_HISTORY_REGISTER, editor)
        .map(|values| values.map(Cow::into_owned).collect())
        .unwrap_or_default();
    history.retain(|entry| parse_process_input(entry).0.is_some_and(|n| n != name));
    history.insert(0, name.to_owned());
    if let Err(err) = editor.registers.write(PROCESS_HISTORY_REGISTER, history) {
        editor.set_error(err.to_string());
    }
}

fn get_breakpoint_at_current_line(editor: &mut Editor) -> Option<(usize, Breakpoint)> {
    let (view, doc) = current!(editor);
    let text = doc.text().slice(..);

    let line = doc.selection(view.id).primary().cursor_line(text);
    let path = doc.path()?;
    editor.breakpoints.get(path).and_then(|breakpoints| {
        let i = breakpoints.iter().position(|b| b.line == line);
        i.map(|i| (i, breakpoints[i].clone()))
    })
}

// -- DAP

fn dap_callback<T, F>(
    jobs: &mut Jobs,
    call: impl Future<Output = helix_dap::Result<serde_json::Value>> + 'static + Send,
    callback: F,
) where
    T: for<'de> serde::Deserialize<'de> + Send + 'static,
    F: FnOnce(&mut Editor, &mut Compositor, T) + Send + 'static,
{
    let callback = Box::pin(async move {
        let json = call.await?;
        let response = serde_json::from_value(json)?;
        let call: Callback = Callback::EditorCompositor(Box::new(
            move |editor: &mut Editor, compositor: &mut Compositor| {
                callback(editor, compositor, response)
            },
        ));
        Ok(call)
    });

    jobs.callback(callback);
}

/// Resolves a single template parameter: paths are canonicalized and process
/// names are mapped onto their PID.
fn resolve_parameter(
    completion: Option<&DebugConfigCompletion>,
    value: &str,
) -> Result<String, anyhow::Error> {
    let Some(DebugConfigCompletion::Advanced(cfg)) = completion else {
        return Ok(value.to_owned());
    };

    Ok(match cfg.completion.as_deref() {
        Some("filename" | "directory") => std::fs::canonicalize(value)
            .ok()
            .and_then(|pb| pb.into_os_string().into_string().ok())
            .unwrap_or_else(|| value.to_owned()),
        // A PID is kept, a bare name maps to its newest process.
        Some("process") => match parse_process_input(value) {
            (_, Some(pid)) => pid.to_string(),
            (Some(name), None) => resolve_process_name(name)?.to_string(),
            (None, None) => value.to_owned(),
        },
        _ => value.to_owned(),
    })
}

/// Replaces `{0}`, `{1}`, ... with the corresponding parameter.
fn substitute_params(value: &str, params: &[String]) -> String {
    params
        .iter()
        .enumerate()
        .fold(value.to_owned(), |value, (i, param)| {
            value.replace(&format!("{{{}}}", i), param)
        })
}

pub fn dap_start_impl(
    cx: &mut compositor::Context,
    name: Option<&str>,
    socket: Option<std::net::SocketAddr>,
    params: Option<Vec<std::borrow::Cow<str>>>,
) -> Result<(), anyhow::Error> {
    // TODO: avoid refetching all of this... pass a config in
    let (mut config, template) = {
        let doc = doc!(cx.editor);
        let config = doc
            .language_config()
            .and_then(|config| config.debugger.as_ref())
            .ok_or_else(|| anyhow!("No debug adapter available for language"))?;

        let template = match name {
            Some(name) => config.templates.iter().find(|t| t.name == name),
            None => config.templates.first(),
        }
        .ok_or_else(|| anyhow!("No debug config with given name"))?
        .clone();

        (config.clone(), template)
    };

    // Resolve every parameter once so the same value ends up in the adapter
    // command and in the request arguments.
    let params: Vec<String> = params
        .unwrap_or_default()
        .iter()
        .enumerate()
        .map(|(i, x)| resolve_parameter(template.completion.get(i), x))
        .collect::<Result<_, anyhow::Error>>()?;

    // `{0}`, `{1}`, ... are substituted into the adapter command and args too, so
    // that e.g. the "host:port" a `connect` transport dials can be prompted for.
    if !params.is_empty() {
        config.command = substitute_params(&config.command, &params);
        for arg in &mut config.args {
            *arg = substitute_params(arg, &params);
        }
    }

    // A template may dial an already-running adapter itself, the same way
    // `:debug-remote` does -- an address passed on the command line wins.
    let socket = match (socket, template.connect.as_deref()) {
        (Some(socket), _) => Some(socket),
        (None, Some(addr)) => {
            let addr = substitute_params(addr, &params);
            Some(addr.parse().map_err(|e| {
                anyhow!(
                    "Invalid connect address {:?} in template {:?}: {}",
                    addr,
                    template.name,
                    e
                )
            })?)
        }
        (None, None) => None,
    };

    let id = cx
        .editor
        .debug_adapters
        .start_client(socket, &config)
        .map_err(|e| anyhow!("Failed to start debug client: {}", e))?;

    let mut args: HashMap<&str, Value> = HashMap::new();

    for (k, t) in &template.args {
        let value = match t.clone() {
            // TODO: just use toml::Value -> json::Value
            DebugArgumentValue::String(v) => {
                DebugArgumentValue::String(substitute_params(&v, &params))
            }
            DebugArgumentValue::Array(arr) => DebugArgumentValue::Array(
                arr.iter().map(|v| substitute_params(v, &params)).collect(),
            ),
            value @ DebugArgumentValue::Boolean(_) => value,
            DebugArgumentValue::Table(map) => DebugArgumentValue::Table(
                map.into_iter()
                    .map(|(mk, mv)| {
                        (
                            substitute_params(&mk, &params),
                            substitute_params(&mv, &params),
                        )
                    })
                    .collect(),
            ),
        };

        match value {
            DebugArgumentValue::String(string) => {
                if let Ok(integer) = string.parse::<usize>() {
                    args.insert(k, to_value(integer).unwrap());
                } else {
                    args.insert(k, to_value(string).unwrap());
                }
            }
            DebugArgumentValue::Array(arr) => {
                args.insert(k, to_value(arr).unwrap());
            }
            DebugArgumentValue::Boolean(bool) => {
                args.insert(k, to_value(bool).unwrap());
            }
            DebugArgumentValue::Table(map) => {
                args.insert(k, to_value(map).unwrap());
            }
        }
    }

    args.insert("cwd", to_value(helix_stdx::env::current_working_dir())?);

    let args = to_value(args).unwrap();

    let callback = |_editor: &mut Editor, _compositor: &mut Compositor, _response: Value| {
        // if let Err(e) = result {
        //     editor.set_error(format!("Failed {} target: {}", template.request, e));
        // }
    };

    let debugger = match cx.editor.debug_adapters.get_client_mut(id) {
        Some(child) => child,
        None => {
            bail!("Failed to get child debugger.");
        }
    };

    match &template.request[..] {
        "launch" => {
            let call = debugger.launch(args);
            dap_callback(cx.jobs, call, callback);
        }
        "attach" => {
            let call = debugger.attach(args);
            dap_callback(cx.jobs, call, callback);
        }
        request => bail!("Unsupported request '{}'", request),
    };

    // TODO: either await "initialized" or buffer commands until event is received
    Ok(())
}

pub fn dap_launch(cx: &mut Context) {
    // TODO: Now that we support multiple Clients, we could run multiple debuggers at once but for now keep this as is
    if cx.editor.debug_adapters.get_active_client().is_some() {
        cx.editor.set_error("Debugger is already running");
        return;
    }

    let doc = doc!(cx.editor);

    let config = match doc
        .language_config()
        .and_then(|config| config.debugger.as_ref())
    {
        Some(c) => c,
        None => {
            cx.editor
                .set_error("No debug adapter available for language");
            return;
        }
    };

    let templates = config.templates.clone();

    let columns = [ui::PickerColumn::new(
        "template",
        |item: &DebugTemplate, _| item.name.as_str().into(),
    )];

    cx.push_layer(Box::new(overlaid(Picker::new(
        columns,
        0,
        templates,
        (),
        |cx, template, _action| {
            if template.completion.is_empty() {
                if let Err(err) = dap_start_impl(cx, Some(&template.name), None, None) {
                    cx.editor.set_error(err.to_string());
                }
            } else {
                let completions = template.completion.clone();
                let name = template.name.clone();
                let callback = Box::pin(async move {
                    let call: Callback =
                        Callback::EditorCompositor(Box::new(move |editor, compositor| {
                            let prompt =
                                debug_parameter_prompt(editor, completions, name, Vec::new());
                            compositor.push(Box::new(prompt));
                        }));
                    Ok(call)
                });
                cx.jobs.callback(callback);
            }
        },
    ))));
}

pub fn dap_restart(cx: &mut Context) {
    let debugger = match cx.editor.debug_adapters.get_active_client_mut() {
        Some(debugger) => debugger,
        None => {
            cx.editor.set_error("Debugger is not running");
            return;
        }
    };
    if debugger.starting_request_args().is_none() {
        cx.editor
            .set_error("No arguments found with which to restart the sessions");
        return;
    }

    // Prefer the adapter's native `restart` request. For adapters that report
    // `supportsRestartRequest = false` (e.g. LLVM-14 lldb-dap), fall back to a
    // full disconnect + relaunch so `dap_restart` still works as one key.
    if debugger
        .capabilities()
        .supports_restart_request
        .unwrap_or(false)
    {
        dap_callback(
            cx.jobs,
            debugger.restart(),
            |editor, _compositor, _resp: serde_json::Value| {
                editor.set_status("Debugging session restarted")
            },
        );
    } else {
        dap_callback(
            cx.jobs,
            debugger.restart_relaunch(),
            |editor, _compositor, _resp: serde_json::Value| {
                editor.set_status("Debugging session restarted")
            },
        );
    }
}

fn debug_parameter_prompt(
    editor: &Editor,
    completions: Vec<DebugConfigCompletion>,
    config_name: String,
    mut params: Vec<String>,
) -> Prompt {
    let completion = completions.get(params.len()).unwrap();
    let field_type = if let DebugConfigCompletion::Advanced(cfg) = completion {
        cfg.completion.as_deref().unwrap_or("")
    } else {
        ""
    };
    let name = match completion {
        DebugConfigCompletion::Advanced(cfg) => cfg.name.as_deref().unwrap_or(field_type),
        DebugConfigCompletion::Named(name) => name.as_str(),
    };
    let default = match completion {
        DebugConfigCompletion::Advanced(cfg) => cfg.default.as_deref().unwrap_or(""),
        _ => "",
    }
    .to_owned();
    let default_val = default.clone();

    let completer = match field_type {
        "filename" => |editor: &Editor, input: &str| {
            ui::completers::filename_with_git_ignore(editor, input, false)
        },
        "directory" => |editor: &Editor, input: &str| {
            ui::completers::directory_with_git_ignore(editor, input, false)
        },
        "process" => process_completer,
        _ => ui::completers::none,
    };
    let is_process = field_type == "process";

    // Pre-fill the prompt with the configured default so it is visible (and
    // editable) instead of the user having to guess what an empty input means.
    let mut prompt = Prompt::new(
        format!("{}: ", name).into(),
        is_process.then_some(PROCESS_HISTORY_REGISTER),
        completer,
        move |cx, input: &str, event: PromptEvent| {
            if event != PromptEvent::Validate {
                return;
            }

            let mut value = input.to_owned();
            if value.is_empty() {
                value = default_val.clone();
            }
            if is_process {
                remember_process_name(cx.editor, &value);
            }
            params.push(value);

            if params.len() < completions.len() {
                let completions = completions.clone();
                let config_name = config_name.clone();
                let params = params.clone();
                let callback = Box::pin(async move {
                    let call: Callback =
                        Callback::EditorCompositor(Box::new(move |editor, compositor| {
                            let prompt =
                                debug_parameter_prompt(editor, completions, config_name, params);
                            compositor.push(Box::new(prompt));
                        }));
                    Ok(call)
                });
                cx.jobs.callback(callback);
            } else if let Err(err) = dap_start_impl(
                cx,
                Some(&config_name),
                None,
                Some(params.iter().map(|x| x.into()).collect()),
            ) {
                cx.editor.set_error(err.to_string());
            }
        },
    );

    if is_process {
        // Renders on every redraw, so the `/proc` scan is cached per line.
        let cache = std::cell::RefCell::new((String::new(), None));
        prompt.doc_fn = Box::new(move |input| {
            let mut cache = cache.borrow_mut();
            if cache.0 != input {
                *cache = (input.to_owned(), process_doc(input));
            }
            cache.1.clone().map(Cow::Owned)
        });
    }

    if default.is_empty() {
        prompt
    } else {
        prompt.with_line(default, editor)
    }
}

pub fn dap_toggle_breakpoint(cx: &mut Context) {
    let (view, doc) = current!(cx.editor);
    let path = match doc.path() {
        Some(path) => path.clone(),
        None => {
            cx.editor
                .set_error("Can't set breakpoint: document has no path");
            return;
        }
    };
    let text = doc.text().slice(..);
    let line = doc.selection(view.id).primary().cursor_line(text);
    dap_toggle_breakpoint_impl(cx, path, line);
}

pub fn dap_toggle_breakpoint_impl(cx: &mut Context, path: PathBuf, line: usize) {
    // TODO: need to map breakpoints over edits and update them?
    // we shouldn't really allow editing while debug is running though

    let breakpoints = cx.editor.breakpoints.entry(path.clone()).or_default();
    // TODO: always keep breakpoints sorted and use binary search to determine insertion point
    if let Some(pos) = breakpoints
        .iter()
        .position(|breakpoint| breakpoint.line == line)
    {
        breakpoints.remove(pos);
    } else {
        breakpoints.push(Breakpoint {
            line,
            ..Default::default()
        });
    }

    let debugger = debugger!(cx.editor);

    if let Err(e) = breakpoints_changed(debugger, path, breakpoints) {
        cx.editor
            .set_error(format!("Failed to set breakpoints: {}", e));
    }
}

pub fn dap_continue(cx: &mut Context) {
    let debugger = debugger!(cx.editor);

    if let Some(thread_id) = debugger.thread_id {
        let request = debugger.continue_thread(thread_id);

        dap_callback(
            cx.jobs,
            request,
            |editor, _compositor, _response: dap::requests::ContinueResponse| {
                debugger!(editor).resume_application();
            },
        );
    } else {
        cx.editor
            .set_error("Currently active thread is not stopped. Switch the thread.");
    }
}

pub fn dap_pause(cx: &mut Context) {
    thread_picker(cx, |editor, thread| {
        let debugger = debugger!(editor);
        let request = debugger.pause(thread.id);
        // NOTE: we don't need to set active thread id here because DAP will emit a "stopped" event
        if let Err(e) = block_on(request) {
            editor.set_error(format!("Failed to pause: {}", e));
        }
    })
}

pub fn dap_step_in(cx: &mut Context) {
    let debugger = debugger!(cx.editor);

    if let Some(thread_id) = debugger.thread_id {
        let request = debugger.step_in(thread_id);

        dap_callback(cx.jobs, request, |editor, _compositor, _response: ()| {
            debugger!(editor).resume_application();
        });
    } else {
        cx.editor
            .set_error("Currently active thread is not stopped. Switch the thread.");
    }
}

pub fn dap_step_out(cx: &mut Context) {
    let debugger = debugger!(cx.editor);

    if let Some(thread_id) = debugger.thread_id {
        let request = debugger.step_out(thread_id);
        dap_callback(cx.jobs, request, |editor, _compositor, _response: ()| {
            debugger!(editor).resume_application();
        });
    } else {
        cx.editor
            .set_error("Currently active thread is not stopped. Switch the thread.");
    }
}

pub fn dap_next(cx: &mut Context) {
    let debugger = debugger!(cx.editor);

    if let Some(thread_id) = debugger.thread_id {
        let request = debugger.next(thread_id);
        dap_callback(cx.jobs, request, |editor, _compositor, _response: ()| {
            debugger!(editor).resume_application();
        });
    } else {
        cx.editor
            .set_error("Currently active thread is not stopped. Switch the thread.");
    }
}

pub fn dap_goto_line(cx: &mut Context) {
    let debugger = debugger!(cx.editor);

    if debugger.capabilities().supports_goto_targets_request != Some(true) {
        cx.editor
            .set_error("Debugger does not support jumping to a line");
        return;
    }

    let thread_id = match debugger.thread_id {
        Some(thread_id) => thread_id,
        None => {
            cx.editor
                .set_error("Currently active thread is not stopped. Switch the thread.");
            return;
        }
    };

    let (view, doc) = current!(cx.editor);
    let path = match doc.path() {
        Some(path) => path.clone(),
        None => {
            cx.editor.set_error("Can't jump: document has no path");
            return;
        }
    };
    let text = doc.text().slice(..);
    // DAP lines are 1-indexed.
    let line = doc.selection(view.id).primary().cursor_line(text) + 1;

    let source = dap::Source {
        path: Some(path),
        ..Default::default()
    };

    let debugger = debugger!(cx.editor);
    let request = debugger.goto_targets(source, line, None);
    dap_callback(
        cx.jobs,
        request,
        move |editor, _compositor, response: dap::requests::GotoTargetsResponse| {
            let target = match response.targets.first() {
                Some(target) => target,
                None => {
                    editor.set_error("No valid jump target on this line");
                    return;
                }
            };
            let debugger = debugger!(editor);
            let request = debugger.goto(thread_id, target.id);
            // A successful goto emits a "stopped" event that moves the cursor.
            if let Err(e) = block_on(request) {
                editor.set_error(format!("Failed to jump: {}", e));
            }
        },
    );
}

/// The expression to seed the eval prompt with: the primary selection, when
/// there is one. Getting at the buffer is otherwise awkward mid-session, since
/// the debug menu is sticky and swallows the motion keys needed to select and
/// yank. A bare cursor is not a selection, so it seeds nothing and leaves `Up`
/// reaching straight for the history.
fn expression_from(text: RopeSlice, range: Range) -> String {
    if range.len() <= 1 {
        return String::new();
    }

    let expression = range.fragment(text);
    // A multi-line fragment is never a useful expression.
    match expression.contains('\n') {
        true => String::new(),
        false => expression.trim().to_string(),
    }
}

/// Register backing the eval prompt's history. `=` mirrors vim's expression
/// register and is not otherwise used by Helix.
const EVAL_HISTORY_REGISTER: char = '=';
/// How many past expressions to keep on disk.
const EVAL_HISTORY_LIMIT: usize = 500;

fn eval_history_file() -> PathBuf {
    helix_loader::cache_dir().join("dap-eval-history")
}

/// Parse the history file, whose entries run oldest first, into the newest-first
/// order `Registers::write` expects.
fn history_from_file(contents: &str) -> Vec<String> {
    contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .rev()
        .map(str::to_string)
        .collect()
}

/// Render newest-first history, as `Registers::read` yields it, into the file's
/// oldest-first order, capped at the most recent `EVAL_HISTORY_LIMIT` entries.
/// A pasted newline would corrupt the line-per-entry format, so drop those.
fn history_to_file(values: impl Iterator<Item = String>) -> String {
    let mut entries: Vec<String> = values.filter(|value| !value.contains('\n')).collect();
    entries.reverse();
    entries.drain(..entries.len().saturating_sub(EVAL_HISTORY_LIMIT));

    entries
        .iter()
        .map(|entry| entry.to_string() + "\n")
        .collect()
}

/// Seed the history register from disk, so the prompt recalls expressions from
/// earlier sessions. Only fills an empty register, i.e. once per session.
fn load_eval_history(editor: &mut Editor) {
    let loaded = editor
        .registers
        .read(EVAL_HISTORY_REGISTER, editor)
        .is_some_and(|values| values.len() > 0);
    if loaded {
        return;
    }

    let Ok(contents) = std::fs::read_to_string(eval_history_file()) else {
        return;
    };

    let entries = history_from_file(&contents);
    if entries.is_empty() {
        return;
    }

    if let Err(err) = editor.registers.write(EVAL_HISTORY_REGISTER, entries) {
        log::error!("Failed to restore debug eval history: {}", err);
    }
}

/// Mirror the history register to disk. The prompt pushes the new expression
/// before invoking us, so the file always matches what the prompt recalls.
fn save_eval_history(editor: &Editor) {
    let Some(values) = editor.registers.read(EVAL_HISTORY_REGISTER, editor) else {
        return;
    };

    let contents = history_to_file(values.map(|value| value.to_string()));

    let path = eval_history_file();
    let written = std::fs::create_dir_all(helix_loader::cache_dir())
        .and_then(|_| std::fs::write(&path, contents));
    if let Err(err) = written {
        log::error!("Failed to save debug eval history to {:?}: {}", path, err);
    }
}

/// Adapters fold long values down with an ellipsis -- pydevd caps how many
/// entries of a collection it renders, and no DAP capability lifts that. When
/// the adapter config says how to ask for the whole value, this returns the
/// expression to evaluate instead. Note it runs the expression a second time.
fn full_value_expression(
    result: &str,
    expression: &str,
    template: Option<&str>,
    supports_clipboard_context: bool,
) -> Option<String> {
    if !result.contains("...") {
        return None;
    }

    match template {
        Some(template) => Some(template.replace("{}", expression)),
        // Without a template there is only something to gain if asking in the
        // "clipboard" context lifts the adapter's own limits.
        None => supports_clipboard_context.then(|| expression.to_string()),
    }
}

/// Re-requesting a value as text hands back a quoted string, e.g. `repr(xs)`
/// evaluates to `'[1, 2, 3]'`. Peel those quotes so what lands in the buffer is
/// the value itself, and undo the escaping that came with them -- a list of
/// strings round trips as `'["a\\'b"]'` and the backslash is not part of the
/// value. Returns the value untouched when it is not a quoted string.
fn unquote(value: &str) -> Cow<'_, str> {
    let mut chars = value.chars();
    let quote = match (chars.next(), chars.next_back()) {
        (Some(first), Some(last)) if first == last && (first == '\'' || first == '"') => first,
        _ => return Cow::Borrowed(value),
    };

    let inner = &value[quote.len_utf8()..value.len() - quote.len_utf8()];
    if !inner.contains('\\') {
        return Cow::Borrowed(inner);
    }

    let mut unescaped = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            unescaped.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => unescaped.push('\n'),
            Some('t') => unescaped.push('\t'),
            Some('r') => unescaped.push('\r'),
            // Anything else escapes to itself (`\\`, `\'`, `\"`). Leave more exotic
            // escapes such as `\x41` alone rather than half-decoding them.
            Some(other) => {
                if !matches!(other, '\\' | '\'' | '"') {
                    unescaped.push('\\');
                }
                unescaped.push(other);
            }
            None => unescaped.push('\\'),
        }
    }
    Cow::Owned(unescaped)
}

/// The target of an assignment, i.e. `xs[0]` in `xs[0] += 1`. An assignment
/// evaluates to nothing, so the target is what the prompt evaluates afterwards
/// to report the value that was just stored. `None` for anything that is not a
/// plain assignment statement.
fn assignment_target(expression: &str) -> Option<&str> {
    let mut depth = 0usize;
    let mut quote: Option<char> = None;

    for (i, ch) in expression.char_indices() {
        match (quote, ch) {
            (Some(open), ch) if ch == open => quote = None,
            (Some(_), _) => (),
            (None, '\'' | '"') => quote = Some(ch),
            (None, '(' | '[' | '{') => depth += 1,
            (None, ')' | ']' | '}') => depth = depth.saturating_sub(1),
            (None, '=') if depth == 0 => {
                let before = &expression[..i];
                let prev = before.chars().next_back();
                // `==` is a comparison, and so is a `=` preceded by one of
                // `!<>:` -- except in `<<=` and `>>=`, which do assign.
                let comparison = expression[i + 1..].starts_with('=')
                    || match prev {
                        Some('!') | Some('=') | Some(':') => true,
                        Some(ch @ ('<' | '>')) => {
                            !before[..before.len() - ch.len_utf8()].ends_with(ch)
                        }
                        _ => false,
                    };
                if comparison {
                    continue;
                }

                // Peel the operator of an augmented assignment off the target.
                let target = before
                    .trim_end_matches(|ch| "+-*/%&|^@<>~".contains(ch))
                    .trim();
                return (!target.is_empty()).then_some(target);
            }
            _ => (),
        }
    }

    None
}

/// Evaluate `expression` in the current frame, asking again for the whole value
/// when the adapter hands back an elided one.
fn eval_complete(debugger: &dap::Client, expression: &str, frame_id: usize) -> dap::Result<String> {
    let mut result = block_on(debugger.eval(expression.to_owned(), Some(frame_id)))?.result;

    // A truncated value is useless to copy, so ask for it again in full.
    let retry = full_value_expression(
        &result,
        expression,
        debugger.quirks.full_value_expression.as_deref(),
        debugger.supports_clipboard_context(),
    );
    if let Some(expression) = retry {
        if let Ok(response) = block_on(debugger.eval_full(expression, Some(frame_id))) {
            result = unquote(&response.result).to_string();
        }
    }

    Ok(result)
}

/// What of an evaluated value to park on the status line. Rendering measures the
/// status message on every frame, so a multi-megabyte repr would be walked over
/// and over -- and only a screenful of it is ever visible anyway. The complete
/// value stays in the clipboard and in `Editor::dap_eval_result`.
fn status_line_value(result: &str) -> String {
    const STATUS_LINE_LIMIT: usize = 2048;

    match result.char_indices().nth(STATUS_LINE_LIMIT) {
        Some((end, _)) => result[..end].to_string(),
        None => result.to_string(),
    }
}

/// The buffer that evaluate results are written to, created on first use and
/// reused afterwards. A long collection is only useful when it can be scrolled and
/// searched, and one shared buffer keeps repeated lookups from piling up scratch
/// buffers. Splits it into view, or focuses the split it is already in.
fn eval_result_buffer(editor: &mut Editor) -> DocumentId {
    let existing = editor
        .dap_eval_buffer
        .filter(|id| editor.documents.contains_key(id));

    let doc_id = match existing {
        Some(doc_id) => doc_id,
        None => {
            let doc_id = editor.new_file(Action::HorizontalSplit);
            editor.dap_eval_buffer = Some(doc_id);
            return doc_id;
        }
    };

    match editor.tree.traverse().find(|(_, view)| view.doc == doc_id) {
        Some((view_id, _)) => editor.focus(view_id),
        None => editor.switch(doc_id, Action::HorizontalSplit),
    }
    doc_id
}

/// Append an evaluated value to the shared evaluate-result buffer and put the
/// cursor on the start of the entry, so the whole value is there to scroll through
/// rather than the screenful the status line can hold.
pub fn append_eval_result(editor: &mut Editor, result: &DapEvalResult) {
    let entry = format!("eval: {}\n{}\n\n", result.expression, result.value);

    let doc_id = eval_result_buffer(editor);
    let view_id = editor.tree.focus;

    let doc = doc_mut!(editor, &doc_id);
    doc.ensure_view_init(view_id);
    let end = doc.text().len_chars();
    let transaction = Transaction::change(doc.text(), [(end, end, Some(entry.into()))].into_iter())
        .with_selection(Selection::point(end));
    doc.apply(&transaction, view_id);

    let view = view_mut!(editor);
    doc.append_changes_to_history(view);
    // Nothing here is worth saving, and a modified buffer would make `:q` complain
    // about unsaved changes on the way out.
    doc.reset_modified();

    editor.ensure_cursor_in_view(view_id);
}

pub fn dap_evaluate(cx: &mut Context) {
    if cx.editor.debug_adapters.get_active_client().is_none() {
        cx.editor.set_error("Debugger is not running");
        return;
    }

    let expression = {
        let (view, doc) = current_ref!(cx.editor);
        expression_from(doc.text().slice(..), doc.selection(view.id).primary())
    };

    load_eval_history(cx.editor);

    let prompt = Prompt::new(
        "eval: ".into(),
        Some(EVAL_HISTORY_REGISTER),
        ui::completers::none,
        |cx, input: &str, event: PromptEvent| {
            if event != PromptEvent::Validate || input.is_empty() {
                return;
            }

            save_eval_history(cx.editor);

            let debugger = match cx.editor.debug_adapters.get_active_client() {
                Some(debugger) => debugger,
                None => {
                    cx.editor.set_error("Debugger is not running");
                    return;
                }
            };
            let (frame, thread_id) = match (debugger.active_frame, debugger.thread_id) {
                (Some(frame), Some(thread_id)) => (frame, thread_id),
                _ => {
                    cx.editor
                        .set_error("Cannot find current stack frame to access variables");
                    return;
                }
            };
            let frame_id = debugger.stack_frames[&thread_id][frame].id;
            let mut result = eval_complete(debugger, input, frame_id);

            // An assignment evaluates to nothing, so read the target back to
            // report what was stored rather than a bare "evaluated".
            if matches!(&result, Ok(value) if value.is_empty()) {
                if let Some(target) = assignment_target(input) {
                    if let Ok(value) = eval_complete(debugger, target, frame_id) {
                        if !value.is_empty() {
                            result = Ok(value);
                        }
                    }
                }
            }

            match result {
                Ok(value) if value.is_empty() => {
                    // Statements evaluate to nothing. Say so rather than leaving
                    // the previous result on the status line, which reads as if
                    // nothing had happened.
                    cx.editor.dap_eval_result = None;
                    cx.editor.set_status("evaluated");
                }
                Ok(value) => {
                    // The status line can only ever show a screenful, so hand the
                    // whole value to the clipboard for pasting elsewhere.
                    if let Err(err) = cx.editor.registers.write('+', vec![value.clone()]) {
                        cx.editor
                            .set_error(format!("Failed to yank result: {}", err));
                    }

                    let status = status_line_value(&value);
                    cx.editor.dap_eval_result = Some(DapEvalResult {
                        expression: input.to_string(),
                        value,
                    });
                    cx.editor.set_status(status);
                }
                Err(e) => {
                    cx.editor.dap_eval_result = None;
                    cx.editor.set_error(format!("Failed to evaluate: {}", e));
                }
            }
        },
    );
    let prompt = match expression.is_empty() {
        true => prompt,
        false => prompt.with_line(expression, cx.editor),
    };
    cx.push_layer(Box::new(prompt));
}

#[derive(Clone)]
struct VariableItem {
    scope: String,
    name: String,
    ty: Option<String>,
    value: String,
    evaluate_name: Option<String>,
    attributes: Vec<String>,
    container_reference: usize,
    frame_id: usize,
}

#[derive(Debug, PartialEq, Eq)]
enum SetTarget {
    Expression {
        expression: String,
        frame_id: usize,
    },
    Variable {
        variables_reference: usize,
        name: String,
    },
}

#[derive(Debug, PartialEq, Eq)]
enum SetRefuse {
    ReadOnly,
    ReturnValue,
    Unsupported,
}

impl fmt::Display for SetRefuse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadOnly => write!(f, "Variable is read-only"),
            Self::ReturnValue => write!(f, "Cannot change return value"),
            Self::Unsupported => write!(f, "Adapter does not support setting variables"),
        }
    }
}

/// Which DAP request to use for a row in the variables picker.
/// Prefers `setExpression` when the adapter supports it and the variable has an
/// evaluate name; otherwise `setVariable` against the parent scope.
fn set_target(
    name: &str,
    evaluate_name: Option<&str>,
    attributes: &[String],
    container_reference: usize,
    frame_id: usize,
    supports_set_expression: bool,
    supports_set_variable: bool,
) -> Result<SetTarget, SetRefuse> {
    if name.starts_with("(return) ") {
        return Err(SetRefuse::ReturnValue);
    }
    if attributes.iter().any(|attr| attr == "readOnly") {
        return Err(SetRefuse::ReadOnly);
    }

    if supports_set_expression {
        if let Some(expression) = evaluate_name.filter(|name| !name.is_empty()) {
            return Ok(SetTarget::Expression {
                expression: expression.to_string(),
                frame_id,
            });
        }
    }
    if supports_set_variable {
        return Ok(SetTarget::Variable {
            variables_reference: container_reference,
            name: name.to_string(),
        });
    }
    Err(SetRefuse::Unsupported)
}

fn collect_variable_items(editor: &mut Editor) -> Option<Vec<VariableItem>> {
    let debugger = match editor.debug_adapters.get_active_client() {
        Some(debugger) => debugger,
        None => {
            editor.set_error("Debugger is not running");
            return None;
        }
    };

    if debugger.thread_id.is_none() {
        editor.set_status("Cannot access variables while target is running.");
        return None;
    }
    let (frame, thread_id) = match (debugger.active_frame, debugger.thread_id) {
        (Some(frame), Some(thread_id)) => (frame, thread_id),
        _ => {
            editor.set_status("Cannot find current stack frame to access variables.");
            return None;
        }
    };

    let thread_frame = match debugger.stack_frames.get(&thread_id) {
        Some(thread_frame) => thread_frame,
        None => {
            editor.set_error(format!("Failed to get stack frame for thread: {thread_id}"));
            return None;
        }
    };
    let stack_frame = match thread_frame.get(frame) {
        Some(stack_frame) => stack_frame,
        None => {
            editor.set_error(format!(
                "Failed to get stack frame for thread {thread_id} and frame {frame}."
            ));
            return None;
        }
    };

    let frame_id = stack_frame.id;
    let scopes = match block_on(debugger.scopes(frame_id)) {
        Ok(s) => s,
        Err(e) => {
            editor.set_error(format!("Failed to get scopes: {}", e));
            return None;
        }
    };

    // TODO: allow expanding variables into sub-fields
    let mut items = Vec::new();
    for scope in scopes {
        let response = block_on(debugger.variables(scope.variables_reference));
        let Ok(vars) = response else {
            continue;
        };
        items.reserve(vars.len());
        for var in vars {
            items.push(VariableItem {
                scope: scope.name.clone(),
                name: var.name,
                ty: var.ty,
                value: var.value,
                evaluate_name: var.evaluate_name,
                attributes: var
                    .presentation_hint
                    .and_then(|hint| hint.attributes)
                    .unwrap_or_default(),
                container_reference: scope.variables_reference,
                frame_id,
            });
        }
    }

    Some(items)
}

fn variables_picker(items: Vec<VariableItem>) -> Picker<VariableItem, ()> {
    let columns = [
        ui::PickerColumn::new("scope", |item: &VariableItem, _| item.scope.as_str().into()),
        ui::PickerColumn::new("name", |item: &VariableItem, _| item.name.as_str().into()),
        ui::PickerColumn::new("type", |item: &VariableItem, _| {
            item.ty.as_deref().unwrap_or("").into()
        }),
        ui::PickerColumn::new("value", |item: &VariableItem, _| item.value.as_str().into()),
    ];

    Picker::new(columns, 1, items, (), |cx, item, _action| {
        prompt_set_variable(cx, item.clone());
    })
    // Paths truncate at the start so the filename stays visible; values should
    // keep the start so a long list shows `[0, 1, 2, …` rather than `…, 98, 99]`.
    .truncate_start(false)
}

fn prompt_set_variable(cx: &mut compositor::Context, item: VariableItem) {
    let callback = Box::pin(async move {
        let call: Callback = Callback::EditorCompositor(Box::new(move |editor, compositor| {
            let current = item.value.clone();
            let mut prompt = Prompt::new(
                "value: ".into(),
                None,
                ui::completers::none,
                move |cx, input: &str, event: PromptEvent| {
                    if event != PromptEvent::Validate {
                        return;
                    }
                    apply_set(cx, &item, input);
                },
            );
            prompt.insert_str(&current, editor);
            compositor.push(Box::new(prompt));
        }));
        Ok(call)
    });
    cx.jobs.callback(callback);
}

fn apply_set(cx: &mut compositor::Context, item: &VariableItem, value: &str) {
    let debugger = match cx.editor.debug_adapters.get_active_client() {
        Some(debugger) => debugger,
        None => {
            cx.editor.set_error("Debugger is not running");
            return;
        }
    };

    let target = match set_target(
        &item.name,
        item.evaluate_name.as_deref(),
        &item.attributes,
        item.container_reference,
        item.frame_id,
        debugger.supports_set_expression(),
        debugger.supports_set_variable(),
    ) {
        Ok(target) => target,
        Err(reason) => {
            cx.editor.set_error(reason.to_string());
            return;
        }
    };

    let result = match target {
        SetTarget::Expression {
            expression,
            frame_id,
        } => block_on(debugger.set_expression(expression, value.to_owned(), Some(frame_id)))
            .map(|response| response.value),
        SetTarget::Variable {
            variables_reference,
            name,
        } => block_on(debugger.set_variable(variables_reference, name, value.to_owned()))
            .map(|response| response.value),
    };

    match result {
        Ok(new_value) => {
            cx.editor.set_status(new_value);
            reopen_variables_picker(cx);
        }
        Err(e) => {
            cx.editor
                .set_error(format!("Failed to set variable: {}", e));
        }
    }
}

fn reopen_variables_picker(cx: &mut compositor::Context) {
    let Some(items) = collect_variable_items(cx.editor) else {
        return;
    };
    let callback = Box::pin(async move {
        let call: Callback = Callback::EditorCompositor(Box::new(move |_editor, compositor| {
            compositor.push(Box::new(variables_picker(items)));
        }));
        Ok(call)
    });
    cx.jobs.callback(callback);
}

pub fn dap_variables(cx: &mut Context) {
    let Some(items) = collect_variable_items(cx.editor) else {
        return;
    };
    cx.push_layer(Box::new(variables_picker(items)));
}

pub fn dap_terminate(cx: &mut Context) {
    cx.editor.set_status("Terminating debug session...");
    let debugger = debugger!(cx.editor);

    let terminate_arguments = Some(TerminateArguments {
        restart: Some(false),
    });

    let request = debugger.terminate(terminate_arguments);
    dap_callback(cx.jobs, request, |editor, _compositor, _response: ()| {
        // editor.set_error(format!("Failed to disconnect: {}", e));
        editor.debug_adapters.unset_active_client();
    });
}

pub fn dap_enable_exceptions(cx: &mut Context) {
    let debugger = debugger!(cx.editor);

    let filters = match &debugger.capabilities().exception_breakpoint_filters {
        Some(filters) => filters.iter().map(|f| f.filter.clone()).collect(),
        None => return,
    };

    let request = debugger.set_exception_breakpoints(filters);

    dap_callback(
        cx.jobs,
        request,
        |_editor, _compositor, _response: dap::requests::SetExceptionBreakpointsResponse| {
            // editor.set_error(format!("Failed to set up exception breakpoints: {}", e));
        },
    )
}

pub fn dap_disable_exceptions(cx: &mut Context) {
    let debugger = debugger!(cx.editor);

    let request = debugger.set_exception_breakpoints(Vec::new());

    dap_callback(
        cx.jobs,
        request,
        |_editor, _compositor, _response: dap::requests::SetExceptionBreakpointsResponse| {
            // editor.set_error(format!("Failed to set up exception breakpoints: {}", e));
        },
    )
}

// TODO: both edit condition and edit log need to be stable: we might get new breakpoints from the debugger which can change offsets
pub fn dap_edit_condition(cx: &mut Context) {
    if let Some((pos, breakpoint)) = get_breakpoint_at_current_line(cx.editor) {
        let path = match doc!(cx.editor).path() {
            Some(path) => path.clone(),
            None => return,
        };
        let callback = Box::pin(async move {
            let call: Callback = Callback::EditorCompositor(Box::new(move |editor, compositor| {
                let mut prompt = Prompt::new(
                    "condition:".into(),
                    None,
                    ui::completers::none,
                    move |cx, input: &str, event: PromptEvent| {
                        if event != PromptEvent::Validate {
                            return;
                        }

                        let breakpoints = &mut cx.editor.breakpoints.get_mut(&path).unwrap();
                        breakpoints[pos].condition = match input {
                            "" => None,
                            input => Some(input.to_owned()),
                        };

                        let debugger = debugger!(cx.editor);

                        if let Err(e) = breakpoints_changed(debugger, path.clone(), breakpoints) {
                            cx.editor
                                .set_error(format!("Failed to set breakpoints: {}", e));
                        }
                    },
                );
                if let Some(condition) = breakpoint.condition {
                    prompt.insert_str(&condition, editor)
                }
                compositor.push(Box::new(prompt));
            }));
            Ok(call)
        });
        cx.jobs.callback(callback);
    }
}

pub fn dap_edit_log(cx: &mut Context) {
    if let Some((pos, breakpoint)) = get_breakpoint_at_current_line(cx.editor) {
        let path = match doc!(cx.editor).path() {
            Some(path) => path.clone(),
            None => return,
        };
        let callback = Box::pin(async move {
            let call: Callback = Callback::EditorCompositor(Box::new(move |editor, compositor| {
                let mut prompt = Prompt::new(
                    "log-message:".into(),
                    None,
                    ui::completers::none,
                    move |cx, input: &str, event: PromptEvent| {
                        if event != PromptEvent::Validate {
                            return;
                        }

                        let breakpoints = &mut cx.editor.breakpoints.get_mut(&path).unwrap();
                        breakpoints[pos].log_message = match input {
                            "" => None,
                            input => Some(input.to_owned()),
                        };

                        let debugger = debugger!(cx.editor);
                        if let Err(e) = breakpoints_changed(debugger, path.clone(), breakpoints) {
                            cx.editor
                                .set_error(format!("Failed to set breakpoints: {}", e));
                        }
                    },
                );
                if let Some(log_message) = breakpoint.log_message {
                    prompt.insert_str(&log_message, editor);
                }
                compositor.push(Box::new(prompt));
            }));
            Ok(call)
        });
        cx.jobs.callback(callback);
    }
}

pub fn dap_switch_thread(cx: &mut Context) {
    thread_picker(cx, |editor, thread| {
        block_on(select_thread_id(editor, thread.id, true));
    })
}
pub fn dap_switch_stack_frame(cx: &mut Context) {
    let debugger = debugger!(cx.editor);

    let thread_id = match debugger.thread_id {
        Some(thread_id) => thread_id,
        None => {
            cx.editor.set_error("No thread is currently active");
            return;
        }
    };

    let frames = debugger.stack_frames[&thread_id].clone();

    let columns = [ui::PickerColumn::new("frame", |item: &StackFrame, _| {
        item.name.as_str().into() // TODO: include thread_states in the label
    })];
    let picker = Picker::new(columns, 0, frames, (), move |cx, frame, _action| {
        let debugger = debugger!(cx.editor);
        // TODO: this should be simpler to find
        let pos = debugger.stack_frames[&thread_id]
            .iter()
            .position(|f| f.id == frame.id);
        debugger.active_frame = pos;

        let frame = debugger.stack_frames[&thread_id]
            .get(pos.unwrap_or(0))
            .cloned();
        if let Some(frame) = &frame {
            jump_to_stack_frame(cx.editor, frame);
        }
    })
    .with_preview(move |_editor, frame| {
        frame
            .source
            .as_ref()
            .and_then(|source| source.path.as_ref())
            .map(|path| {
                (
                    path.as_path().into(),
                    Some((
                        frame.line.saturating_sub(1),
                        frame.end_line.unwrap_or(frame.line).saturating_sub(1),
                    )),
                )
            })
    });
    cx.push_layer(Box::new(picker))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_numbered_params() {
        let params = vec!["5678".to_string(), "127.0.0.1".to_string()];

        assert_eq!(substitute_params("{1}:{0}", &params), "127.0.0.1:5678");
        // A template address with the port filled in must be a valid socket.
        assert!(substitute_params("127.0.0.1:{0}", &params)
            .parse::<std::net::SocketAddr>()
            .is_ok());
        // Unreferenced placeholders and plain text are left alone.
        assert_eq!(substitute_params("{2}", &params), "{2}");
        assert_eq!(substitute_params("python3", &[]), "python3");
    }

    #[test]
    fn parses_process_input() {
        assert_eq!(parse_process_input("1234"), (None, Some(1234)));
        assert_eq!(
            parse_process_input("hython-bin"),
            (Some("hython-bin"), None)
        );
        // A Tab completion appends the PID, age and arguments after the name.
        assert_eq!(
            parse_process_input("hython-bin 1234  2m ago  /a/run.py /a/test.py"),
            (Some("hython-bin"), Some(1234))
        );
        assert_eq!(parse_process_input("  "), (None, None));
    }

    #[test]
    fn formats_process_age() {
        assert_eq!(format_age(59), "59s");
        assert_eq!(format_age(60), "1m");
        assert_eq!(format_age(7200), "2h");
        assert_eq!(format_age(172800), "2d");
    }

    #[test]
    fn finds_the_target_of_an_assignment() {
        // Plain, chained and augmented assignments all read back the target.
        assert_eq!(assignment_target("x = 1"), Some("x"));
        assert_eq!(assignment_target("a, b = 1, 2"), Some("a, b"));
        assert_eq!(assignment_target("self.count += 1"), Some("self.count"));
        assert_eq!(assignment_target("xs[0] //= 2"), Some("xs[0]"));
        assert_eq!(assignment_target("bits <<= 1"), Some("bits"));
        assert_eq!(assignment_target("s = \"a=b\""), Some("s"));

        // Comparisons and expressions are not assignments.
        assert_eq!(assignment_target("a == b"), None);
        assert_eq!(assignment_target("a != b"), None);
        assert_eq!(assignment_target("a >= b"), None);
        assert_eq!(assignment_target("(n := 1)"), None);
        assert_eq!(assignment_target("len(xs)"), None);
        // A keyword argument is bracketed, and the call returns a value anyway.
        assert_eq!(assignment_target("f(a=1)"), None);
        // Nor is an `=` inside a string.
        assert_eq!(assignment_target("\"a=b\""), None);
        // Nothing to read back without a target.
        assert_eq!(assignment_target("= 5"), None);
    }

    #[test]
    fn seeds_expression_from_selection() {
        let rope = helix_core::Rope::from("total = count + 1\nnext()\n");
        let text = rope.slice(..);

        // A selection wider than the cursor is taken as-is, trimmed.
        assert_eq!(expression_from(text, Range::new(8, 13)), "count");
        assert_eq!(expression_from(text, Range::new(7, 14)), "count");

        // A bare cursor seeds nothing, whatever it happens to sit on.
        assert_eq!(expression_from(text, Range::point(9)), "");
        assert_eq!(expression_from(text, Range::point(1)), "");

        // Spanning a line break gives nothing to evaluate.
        assert_eq!(expression_from(text, Range::new(0, 20)), "");
    }

    #[test]
    fn retries_only_truncated_values() {
        let template = Some("repr({})");

        assert_eq!(
            full_value_expression("[0, 1, ...]", "xs", template, true),
            Some("repr(xs)".to_string())
        );
        // A complete value is left alone.
        assert_eq!(
            full_value_expression("[0, 1, 2]", "xs", template, true),
            None
        );
        // Without a template, asking again only helps if the adapter honors the
        // clipboard context.
        assert_eq!(
            full_value_expression("[0, 1, ...]", "xs", None, true),
            Some("xs".to_string())
        );
        assert_eq!(
            full_value_expression("[0, 1, ...]", "xs", None, false),
            None
        );
    }

    #[test]
    fn unquotes_re_requested_values() {
        assert_eq!(unquote("'[0, 1, 2]'"), "[0, 1, 2]");
        assert_eq!(unquote("\"[0, 1, 2]\""), "[0, 1, 2]");
        // Values that are not wrapped, or too short to be, stay as they are.
        assert_eq!(unquote("[0, 1, 2]"), "[0, 1, 2]");
        assert_eq!(unquote("'"), "'");
        assert_eq!(unquote(""), "");
    }

    #[test]
    fn unquoting_undoes_the_escaping_of_the_round_trip() {
        // `repr(["a'b"])` comes back as a quoted string with the inner quote escaped.
        assert_eq!(unquote(r#"'["a\'b"]'"#), r#"["a'b"]"#);
        assert_eq!(unquote(r#""['a\"b']""#), r#"['a"b']"#);
        // A backslash in the value itself was doubled on the way out.
        assert_eq!(unquote(r"'C:\\tmp'"), r"C:\tmp");
        assert_eq!(unquote(r"'a\nb'"), "a\nb");
        // Escapes that are not undone keep their backslash rather than losing it.
        assert_eq!(unquote(r"'\x41'"), r"\x41");
    }

    #[test]
    fn status_line_value_is_clamped() {
        assert_eq!(status_line_value("[1, 2, 3]"), "[1, 2, 3]");

        // Long values are cut well past any terminal width, and only for the
        // status line -- never for the clipboard.
        let long = "x".repeat(10_000);
        assert_eq!(status_line_value(&long).len(), 2048);

        // Cutting happens on character boundaries.
        let wide = "\u{e9}".repeat(4096);
        assert_eq!(status_line_value(&wide).chars().count(), 2048);
    }

    #[test]
    fn history_round_trips_newest_first() {
        // `Registers::read` hands back the newest entry first.
        let contents = history_to_file(["x + 1", "len(items)"].map(String::from).into_iter());
        // The file reads oldest first, like a log.
        assert_eq!(contents, "len(items)\nx + 1\n");
        assert_eq!(history_from_file(&contents), vec!["x + 1", "len(items)"]);

        // Entries that would break the line-per-entry format are dropped.
        assert_eq!(
            history_to_file(["a\nb", "c"].map(String::from).into_iter()),
            "c\n"
        );
        assert!(history_from_file("\n\n").is_empty());

        // Only the most recent entries are kept.
        let many = (0..EVAL_HISTORY_LIMIT + 10).map(|i| i.to_string());
        let kept = history_from_file(&history_to_file(many));
        assert_eq!(kept.len(), EVAL_HISTORY_LIMIT);
        assert_eq!(kept[0], "0", "the newest entry survives");
        assert_eq!(
            kept[EVAL_HISTORY_LIMIT - 1],
            (EVAL_HISTORY_LIMIT - 1).to_string()
        );
    }

    #[test]
    fn set_target_prefers_expression_when_named() {
        assert_eq!(
            set_target("x", Some("x"), &[], 7, 3, true, true),
            Ok(SetTarget::Expression {
                expression: "x".to_string(),
                frame_id: 3,
            })
        );
        // No evaluate name: fall back to setVariable on the parent scope.
        assert_eq!(
            set_target("x", None, &[], 7, 3, true, true),
            Ok(SetTarget::Variable {
                variables_reference: 7,
                name: "x".to_string(),
            })
        );
        // Only setVariable is advertised.
        assert_eq!(
            set_target("x", Some("x"), &[], 7, 3, false, true),
            Ok(SetTarget::Variable {
                variables_reference: 7,
                name: "x".to_string(),
            })
        );
    }

    #[test]
    fn set_target_refuses_readonly_return_and_missing_capability() {
        assert_eq!(
            set_target("x", Some("x"), &["readOnly".into()], 7, 3, true, true),
            Err(SetRefuse::ReadOnly)
        );
        assert_eq!(
            set_target("(return) x", Some("x"), &[], 7, 3, true, true),
            Err(SetRefuse::ReturnValue)
        );
        assert_eq!(
            set_target("x", None, &[], 7, 3, true, false),
            Err(SetRefuse::Unsupported)
        );
        assert_eq!(
            set_target("x", Some("x"), &[], 7, 3, false, false),
            Err(SetRefuse::Unsupported)
        );
    }
}
