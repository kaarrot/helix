use crate::document::DocumentOpenError;
use crate::editor::{Action, Breakpoint};
use crate::tree::Direction;
use crate::{align_view, Align, Editor, ViewId};
use dap::requests::DisconnectArguments;
use helix_core::Selection;
use helix_dap::{
    self as dap, registry::DebugAdapterId, Client, ConnectionType, Payload, Request, ThreadId,
};
use helix_lsp::block_on;
use log::{error, warn};
use serde_json::{json, Value};
use std::fmt::Write;
use std::path::PathBuf;

#[macro_export]
macro_rules! debugger {
    ($editor:expr) => {{
        let Some(debugger) = $editor.debug_adapters.get_active_client_mut() else {
            return;
        };
        debugger
    }};
}

// general utils:
pub fn dap_pos_to_pos(doc: &helix_core::Rope, line: usize, column: usize) -> Option<usize> {
    // 1-indexing to 0 indexing
    let line = doc.try_line_to_char(line - 1).ok()?;
    let pos = line + column.saturating_sub(1);
    // TODO: this is probably utf-16 offsets
    Some(pos)
}

pub async fn select_thread_id(editor: &mut Editor, thread_id: ThreadId, force: bool) {
    let debugger = debugger!(editor);

    if !force && debugger.thread_id.is_some() {
        return;
    }

    debugger.thread_id = Some(thread_id);
    fetch_stack_trace(debugger, thread_id).await;

    let frame = debugger.current_stack_frame().cloned();
    if let Some(frame) = &frame {
        jump_to_stack_frame(editor, frame);
    }
}

pub async fn fetch_stack_trace(debugger: &mut Client, thread_id: ThreadId) {
    let (frames, _) = match debugger.stack_trace(thread_id).await {
        Ok(frames) => frames,
        Err(_) => return,
    };
    // `active_frame` indexes the current thread's frames, so fetching another
    // thread's stack must leave it alone. An adapter can also report no frames at
    // all -- debugpy with `justMyCode` for a thread stopped in library code -- and
    // then there is no frame to select.
    if debugger.thread_id == Some(thread_id) {
        debugger.active_frame = (!frames.is_empty()).then_some(0);
    }
    debugger.stack_frames.insert(thread_id, frames);
}

pub fn jump_to_stack_frame(editor: &mut Editor, frame: &helix_dap::StackFrame) {
    let path = if let Some(helix_dap::Source {
        path: Some(ref path),
        ..
    }) = frame.source
    {
        path.clone()
    } else {
        return;
    };

    // Stepping from the debug console shows the source beside it rather than in
    // its place, and leaves the console focused for the next command.
    let shown = if editor.is_dap_console_focused() {
        show_beside_console(editor, &path)
    } else {
        editor
            .open(&path, Action::Replace)
            .map(|_| editor.tree.focus)
    };
    let view_id = match shown {
        Ok(view_id) => view_id,
        Err(e) => {
            editor.set_error(format!("Unable to jump to stack frame: {}", e));
            return;
        }
    };

    let view = editor.tree.get(view_id);
    let doc = editor.documents.get_mut(&view.doc).unwrap();

    let text_end = doc.text().len_chars().saturating_sub(1);
    let start = dap_pos_to_pos(doc.text(), frame.line, frame.column).unwrap_or(0);
    let end = frame
        .end_line
        .and_then(|end_line| dap_pos_to_pos(doc.text(), end_line, frame.end_column.unwrap_or(0)))
        .unwrap_or(start);

    let selection = Selection::single(start.min(text_end), end.min(text_end));
    doc.set_selection(view.id, selection);
    align_view(doc, view, Align::Center);
}

/// Opens `path` in the view the console reserves for source (see
/// `Editor::dap_source_view`) without taking focus from the console, splitting one
/// off when the console is alone on screen. Returns that view.
fn show_beside_console(
    editor: &mut Editor,
    path: &std::path::Path,
) -> Result<ViewId, DocumentOpenError> {
    let console_view = editor.tree.focus;
    let mode = editor.mode;
    // Loading neither moves focus nor leaves insert mode.
    let doc_id = editor.open(path, Action::Load)?;

    if let Some(view_id) = editor.dap_source_view(Some(path)) {
        if editor.tree.get(view_id).doc != doc_id {
            editor.replace_document_in_view(view_id, doc_id);
        }
        return Ok(view_id);
    }

    // Splitting and refocusing both drop back to normal mode on the way; the
    // console was focused throughout as far as the user is concerned.
    editor.switch(doc_id, Action::HorizontalSplit);
    editor.swap_split_in_direction(Direction::Up);
    let view_id = editor.tree.focus;
    editor.focus(console_view);
    editor.mode = mode;
    Ok(view_id)
}

pub fn breakpoints_changed(
    debugger: &mut dap::Client,
    path: PathBuf,
    breakpoints: &mut [Breakpoint],
) -> Result<(), anyhow::Error> {
    // TODO: handle capabilities correctly again, by filtering breakpoints when emitting
    // if breakpoint.condition.is_some()
    //     && !debugger
    //         .caps
    //         .as_ref()
    //         .unwrap()
    //         .supports_conditional_breakpoints
    //         .unwrap_or_default()
    // {
    //     bail!(
    //         "Can't edit breakpoint: debugger does not support conditional breakpoints"
    //     )
    // }
    // if breakpoint.log_message.is_some()
    //     && !debugger
    //         .caps
    //         .as_ref()
    //         .unwrap()
    //         .supports_log_points
    //         .unwrap_or_default()
    // {
    //     bail!("Can't edit breakpoint: debugger does not support logpoints")
    // }
    let source_breakpoints = breakpoints
        .iter()
        .map(|breakpoint| helix_dap::SourceBreakpoint {
            line: breakpoint.line + 1, // convert from 0-indexing to 1-indexing (TODO: could set debugger to 0-indexing on init)
            ..Default::default()
        })
        .collect::<Vec<_>>();

    let request = debugger.set_breakpoints(path, source_breakpoints);
    match block_on(request) {
        Ok(Some(dap_breakpoints)) => {
            for (breakpoint, dap_breakpoint) in breakpoints.iter_mut().zip(dap_breakpoints) {
                breakpoint.id = dap_breakpoint.id;
                breakpoint.verified = dap_breakpoint.verified;
                breakpoint.message = dap_breakpoint.message;
                // TODO: handle breakpoint.message
                // TODO: verify source matches
                // An adapter that cannot place a breakpoint may leave its line out;
                // keep the one asked for rather than moving it to the first line.
                if let Some(line) = dap_breakpoint.line {
                    breakpoint.line = line.saturating_sub(1); // convert to 0-indexing
                }
                breakpoint.column = dap_breakpoint.column;
                // TODO: verify end_linef/col instruction reference, offset
            }
        }
        Err(e) => anyhow::bail!("Failed to set breakpoints: {}", e),
        _ => {}
    };
    Ok(())
}

impl Editor {
    pub async fn handle_debugger_message(
        &mut self,
        id: DebugAdapterId,
        payload: helix_dap::Payload,
    ) -> bool {
        use helix_dap::{events, Event};

        match payload {
            Payload::Event(event) => {
                let event = match Event::parse(&event.event, event.body) {
                    Ok(event) => event,
                    Err(dap::Error::Unhandled) => {
                        log::info!("Discarding unknown DAP event '{}'", event.event);
                        return false;
                    }
                    Err(err) => {
                        log::warn!("Discarding invalid DAP event '{}': {err}", event.event);
                        return false;
                    }
                };
                match event {
                    Event::Stopped(events::StoppedBody {
                        thread_id,
                        description,
                        text,
                        reason,
                        all_threads_stopped,
                        ..
                    }) => {
                        let debugger = match self.debug_adapters.get_client_mut(id) {
                            Some(debugger) => debugger,
                            None => return false,
                        };

                        let all_threads_stopped = all_threads_stopped.unwrap_or_default();

                        if all_threads_stopped {
                            if let Ok(response) =
                                debugger.request::<dap::requests::Threads>(()).await
                            {
                                for thread in response.threads {
                                    fetch_stack_trace(debugger, thread.id).await;
                                }
                                select_thread_id(self, thread_id.unwrap_or_default(), false).await;
                            }
                        } else if let Some(thread_id) = thread_id {
                            debugger.thread_states.insert(thread_id, reason.clone()); // TODO: dap uses "type" || "reason" here

                            fetch_stack_trace(debugger, thread_id).await;
                            // whichever thread stops is made "current" (if no previously selected thread).
                            select_thread_id(self, thread_id, false).await;
                        }

                        let scope = match thread_id {
                            Some(id) => format!("Thread {}", id),
                            None => "Target".to_owned(),
                        };

                        let mut status = format!("{} stopped because of {}", scope, reason);
                        if let Some(desc) = description {
                            write!(status, " {}", desc).unwrap();
                        }
                        if let Some(text) = text {
                            write!(status, " {}", text).unwrap();
                        }
                        if all_threads_stopped {
                            status.push_str(" (all threads stopped)");
                        }

                        self.set_status(status);
                        self.debug_adapters.set_active_client(id);

                        self.dap_console_flush_output();
                        self.dap_console_print_location();
                    }
                    Event::Continued(events::ContinuedBody { thread_id, .. }) => {
                        let debugger = match self.debug_adapters.get_client_mut(id) {
                            Some(debugger) => debugger,
                            None => return false,
                        };

                        debugger
                            .thread_states
                            .insert(thread_id, "running".to_owned());
                        if debugger.thread_id == Some(thread_id) {
                            debugger.resume_application();
                        }
                    }
                    Event::Thread(thread) => {
                        self.set_status(format!("Thread {}: {}", thread.thread_id, thread.reason));
                        let debugger = match self.debug_adapters.get_client_mut(id) {
                            Some(debugger) => debugger,
                            None => return false,
                        };

                        debugger.thread_id = Some(thread.thread_id);
                        // set the stack frame for the thread
                    }
                    Event::Breakpoint(events::BreakpointBody { reason, breakpoint }) => {
                        match &reason[..] {
                            "new" => {
                                if let Some(source) = breakpoint.source {
                                    self.breakpoints
                                        .entry(source.path.unwrap()) // TODO: no unwraps
                                        .or_default()
                                        .push(Breakpoint {
                                            id: breakpoint.id,
                                            verified: breakpoint.verified,
                                            message: breakpoint.message,
                                            line: breakpoint.line.unwrap().saturating_sub(1), // TODO: no unwrap
                                            column: breakpoint.column,
                                            ..Default::default()
                                        });
                                }
                            }
                            "changed" => {
                                for breakpoints in self.breakpoints.values_mut() {
                                    if let Some(i) =
                                        breakpoints.iter().position(|b| b.id == breakpoint.id)
                                    {
                                        breakpoints[i].verified = breakpoint.verified;
                                        breakpoints[i].message = breakpoint
                                            .message
                                            .clone()
                                            .or_else(|| breakpoints[i].message.take());
                                        breakpoints[i].line =
                                            breakpoint.line.map_or(breakpoints[i].line, |line| {
                                                line.saturating_sub(1)
                                            });
                                        breakpoints[i].column =
                                            breakpoint.column.or(breakpoints[i].column);
                                    }
                                }
                            }
                            "removed" => {
                                for breakpoints in self.breakpoints.values_mut() {
                                    if let Some(i) =
                                        breakpoints.iter().position(|b| b.id == breakpoint.id)
                                    {
                                        breakpoints.remove(i);
                                    }
                                }
                            }
                            reason => {
                                warn!("Unknown breakpoint event: {}", reason);
                            }
                        }
                    }
                    Event::Output(events::OutputBody {
                        category, output, ..
                    }) => {
                        let prefix = match &category {
                            Some(category) => {
                                if category == "telemetry" {
                                    return false;
                                }
                                format!("Debug ({}):", category)
                            }
                            None => "Debug:".to_owned(),
                        };

                        let category = category.as_deref().unwrap_or("console");
                        let program_output = matches!(category, "stdout" | "stderr");
                        if self.dap_console_doc().is_some() {
                            // Gathered and written once per frame, which also
                            // renders it; a chatty program would otherwise cost an
                            // edit and a frame per line.
                            self.dap_console_output(category, &output);
                            return false;
                        }
                        if program_output {
                            // Without a console there is nowhere to show a program's
                            // own output that would not bury the debugger's messages
                            // on the status line. It still goes to its terminal.
                            log::debug!("{}", output);
                            return false;
                        }
                        log::info!("{}", output);
                        self.set_status(format!("{} {}", prefix, output));
                    }
                    Event::Initialized(_) => {
                        self.set_status("Debugger initialized...");
                        let debugger = match self.debug_adapters.get_client_mut(id) {
                            Some(debugger) => debugger,
                            None => return false,
                        };

                        // send existing breakpoints
                        for (path, breakpoints) in &mut self.breakpoints {
                            // TODO: call futures in parallel, await all
                            let _ = breakpoints_changed(debugger, path.clone(), breakpoints);
                        }
                        // TODO: fetch breakpoints (in case we're attaching)

                        if debugger.configuration_done().await.is_ok() {
                            self.set_status("Debugged application started");
                        }; // TODO: do we need to handle error?
                    }
                    Event::Terminated(terminated) => {
                        // Nothing more is coming to finish a line the program left
                        // open, e.g. `print("done", end="")`.
                        self.dap_console_flush_output();
                        let debugger = match self.debug_adapters.get_client_mut(id) {
                            Some(debugger) => debugger,
                            None => return false,
                        };

                        let restart_arg = if let Some(terminated) = terminated {
                            terminated.restart
                        } else {
                            None
                        };

                        let restart_bool = restart_arg
                            .as_ref()
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let disconnect_args = Some(DisconnectArguments {
                            restart: Some(restart_bool),
                            terminate_debuggee: None,
                            suspend_debuggee: None,
                        });

                        if let Err(err) = debugger.disconnect(disconnect_args).await {
                            // An in-process adapter (debugpy.listen(in_process_debug_adapter=True))
                            // exits with the debuggee right after `terminated`, so nothing is left
                            // to answer; the session is over either way.
                            if !matches!(err, dap::Error::StreamClosed) {
                                self.set_error(format!(
                                    "Cannot disconnect debugger upon terminated event receival {:?}",
                                    err
                                ));
                                return false;
                            }
                        }

                        match restart_arg {
                            Some(Value::Bool(false)) | None => {
                                self.debug_adapters.remove_client(id);
                                self.debug_adapters.unset_active_client();
                                self.set_status(
                                    "Terminated debugging session and disconnected debugger.",
                                );

                                // Go through all breakpoints and set verfified to false
                                // this should update the UI to show the breakpoints are no longer connected
                                for breakpoints in self.breakpoints.values_mut() {
                                    for breakpoint in breakpoints.iter_mut() {
                                        breakpoint.verified = false;
                                    }
                                }
                            }
                            Some(val) => {
                                log::info!("Attempting to restart debug session.");
                                let connection_type = match debugger.connection_type() {
                                    Some(connection_type) => connection_type,
                                    None => {
                                        self.set_error("No starting request found, to be used in restarting the debugging session.");
                                        return false;
                                    }
                                };

                                let relaunch_resp = if let ConnectionType::Launch = connection_type
                                {
                                    debugger.launch(val).await
                                } else {
                                    debugger.attach(val).await
                                };

                                if let Err(err) = relaunch_resp {
                                    self.set_error(format!(
                                        "Failed to restart debugging session: {:?}",
                                        err
                                    ));
                                }
                            }
                        }
                    }
                    Event::Exited(resp) => {
                        self.dap_console_flush_output();
                        let exit_code = resp.exit_code;
                        if exit_code != 0 {
                            self.set_error(format!(
                                "Debuggee failed to exit successfully (exit code: {exit_code})."
                            ));
                        }
                    }
                    ev => {
                        log::warn!("Unhandled event {:?}", ev);
                        return false; // return early to skip render
                    }
                }
            }
            Payload::Response(_) => unreachable!(),
            Payload::Request(request) => {
                let reply = match Request::parse(&request.command, request.arguments) {
                    Ok(Request::RunInTerminal(arguments)) => {
                        let config = self.config();
                        let Some(config) = config.terminal.as_ref() else {
                            self.set_error("No external terminal defined");
                            return true;
                        };

                        let process = match std::process::Command::new(&config.command)
                            .args(&config.args)
                            .arg(arguments.args.join(" "))
                            .spawn()
                        {
                            Ok(process) => process,
                            Err(err) => {
                                self.set_error(format!(
                                    "Error starting external terminal: {}",
                                    err
                                ));
                                return true;
                            }
                        };

                        Ok(json!(dap::requests::RunInTerminalResponse {
                            process_id: Some(process.id()),
                            shell_process_id: None,
                        }))
                    }
                    Ok(Request::StartDebugging(arguments)) => {
                        let debugger = match self.debug_adapters.get_client_mut(id) {
                            Some(debugger) => debugger,
                            None => {
                                self.set_error("No active debugger found.");
                                return true;
                            }
                        };
                        // Currently we only support starting a child debugger if the parent is using the TCP transport
                        let socket = match debugger.socket {
                            Some(socket) => socket,
                            None => {
                                self.set_error("Child debugger can only be started if the parent debugger is using TCP transport.");
                                return true;
                            }
                        };

                        let config = match debugger.config.clone() {
                            Some(config) => config,
                            None => {
                                error!("No configuration found for the debugger.");
                                return true;
                            }
                        };

                        let result = self.debug_adapters.start_client(Some(socket), &config);

                        let client_id = match result {
                            Ok(child) => child,
                            Err(err) => {
                                self.set_error(format!(
                                    "Failed to create child debugger: {:?}",
                                    err
                                ));
                                return true;
                            }
                        };

                        let client = match self.debug_adapters.get_client_mut(client_id) {
                            Some(child) => child,
                            None => {
                                self.set_error("Failed to get child debugger.");
                                return true;
                            }
                        };

                        let relaunch_resp = if let ConnectionType::Launch = arguments.request {
                            client.launch(arguments.configuration).await
                        } else {
                            client.attach(arguments.configuration).await
                        };
                        if let Err(err) = relaunch_resp {
                            self.set_error(format!("Failed to start debugging session: {:?}", err));
                            return true;
                        }

                        Ok(json!({
                            "success": true,
                        }))
                    }
                    Err(err) => Err(err),
                };

                if let Some(debugger) = self.debug_adapters.get_client_mut(id) {
                    debugger
                        .reply(request.seq, &request.command, reply)
                        .await
                        .ok();
                }
            }
        }
        true
    }
}
