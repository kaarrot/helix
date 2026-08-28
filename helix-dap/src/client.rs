use crate::{
    registry::DebugAdapterId,
    requests::{DisconnectArguments, TerminateArguments},
    transport::{Payload, Request, Response, Transport},
    types::*,
    Error, Result,
};
use helix_core::syntax::config::{DebugAdapterConfig, DebuggerQuirks};

use serde_json::Value;

use anyhow::anyhow;
use std::{
    collections::HashMap,
    future::Future,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncBufRead, AsyncWrite, BufReader, BufWriter},
    net::TcpStream,
    process::{Child, Command},
    sync::mpsc::{channel, unbounded_channel, UnboundedReceiver, UnboundedSender},
    time,
};

#[derive(Debug)]
pub struct Client {
    id: DebugAdapterId,
    _process: Option<Child>,
    server_tx: UnboundedSender<Payload>,
    request_counter: Arc<AtomicU64>,
    connection_type: Option<ConnectionType>,
    starting_request_args: Option<Value>,
    /// The socket address of the debugger, if using TCP transport.
    pub socket: Option<SocketAddr>,
    pub caps: Option<DebuggerCapabilities>,
    // thread_id -> frames
    pub stack_frames: HashMap<ThreadId, Vec<StackFrame>>,
    pub thread_states: ThreadStates,
    pub thread_id: Option<ThreadId>,
    /// Currently active frame for the current thread.
    pub active_frame: Option<usize>,
    pub quirks: DebuggerQuirks,
    /// The config which was used to start this debugger.
    pub config: Option<DebugAdapterConfig>,
}

impl Client {
    // Spawn a process and communicate with it by either TCP or stdio
    // The returned stream includes the Client ID so consumers can differentiate between multiple clients
    pub async fn process(
        transport: &str,
        command: &str,
        args: Vec<&str>,
        port_arg: Option<&str>,
        id: DebugAdapterId,
    ) -> Result<(Self, UnboundedReceiver<(DebugAdapterId, Payload)>)> {
        if command.is_empty() {
            return Result::Err(Error::Other(anyhow!("Command not provided")));
        }
        match (transport, port_arg) {
            ("tcp", Some(port_arg)) => Self::tcp_process(command, args, port_arg, id).await,
            ("stdio", _) => Self::stdio(command, args, id),
            // Connect directly to an already-running DAP server (e.g. a process that
            // called debugpy.listen()); `command` holds the "host:port" to connect to.
            ("connect", _) => {
                let addr = command.parse().map_err(|e| {
                    Error::Other(anyhow!("Invalid connect address {:?}: {}", command, e))
                })?;
                Self::tcp(addr, id).await
            }
            _ => Result::Err(Error::Other(anyhow!("Incorrect transport {}", transport))),
        }
    }

    pub fn streams(
        rx: Box<dyn AsyncBufRead + Unpin + Send>,
        tx: Box<dyn AsyncWrite + Unpin + Send>,
        err: Option<Box<dyn AsyncBufRead + Unpin + Send>>,
        id: DebugAdapterId,
        process: Option<Child>,
    ) -> Result<(Self, UnboundedReceiver<(DebugAdapterId, Payload)>)> {
        let (server_rx, server_tx) = Transport::start(rx, tx, err, id);
        let (client_tx, client_rx) = unbounded_channel();

        let client = Self {
            id,
            _process: process,
            server_tx,
            request_counter: Arc::new(AtomicU64::new(0)),
            caps: None,
            connection_type: None,
            starting_request_args: None,
            socket: None,
            stack_frames: HashMap::new(),
            thread_states: HashMap::new(),
            thread_id: None,
            active_frame: None,
            quirks: DebuggerQuirks::default(),
            config: None,
        };

        tokio::spawn(Self::recv(id, server_rx, client_tx));

        Ok((client, client_rx))
    }

    pub async fn tcp(
        addr: std::net::SocketAddr,
        id: DebugAdapterId,
    ) -> Result<(Self, UnboundedReceiver<(DebugAdapterId, Payload)>)> {
        let stream = TcpStream::connect(addr).await?;
        let (rx, tx) = stream.into_split();
        Self::streams(Box::new(BufReader::new(rx)), Box::new(tx), None, id, None)
    }

    pub fn stdio(
        cmd: &str,
        args: Vec<&str>,
        id: DebugAdapterId,
    ) -> Result<(Self, UnboundedReceiver<(DebugAdapterId, Payload)>)> {
        // Resolve path to the binary
        let cmd = helix_stdx::env::which(cmd)?;

        let process = Command::new(cmd)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // make sure the process is reaped on drop
            .kill_on_drop(true)
            .spawn();

        let mut process = process?;

        // TODO: do we need bufreader/writer here? or do we use async wrappers on unblock?
        let writer = BufWriter::new(process.stdin.take().expect("Failed to open stdin"));
        let reader = BufReader::new(process.stdout.take().expect("Failed to open stdout"));
        let stderr = BufReader::new(process.stderr.take().expect("Failed to open stderr"));

        Self::streams(
            Box::new(reader),
            Box::new(writer),
            Some(Box::new(stderr)),
            id,
            Some(process),
        )
    }

    async fn get_port() -> Option<u16> {
        Some(
            tokio::net::TcpListener::bind(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                0,
            ))
            .await
            .ok()?
            .local_addr()
            .ok()?
            .port(),
        )
    }

    pub fn starting_request_args(&self) -> Option<&Value> {
        self.starting_request_args.as_ref()
    }

    pub async fn tcp_process(
        cmd: &str,
        args: Vec<&str>,
        port_format: &str,
        id: DebugAdapterId,
    ) -> Result<(Self, UnboundedReceiver<(DebugAdapterId, Payload)>)> {
        let port = Self::get_port().await.unwrap();

        let process = Command::new(cmd)
            .args(args)
            .args(port_format.replace("{}", &port.to_string()).split(' '))
            // silence messages
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // Do not kill debug adapter when leaving, it should exit automatically
            .spawn()?;

        // Wait for adapter to become ready for connection
        time::sleep(time::Duration::from_millis(500)).await;
        let socket = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), port);
        let stream = TcpStream::connect(socket).await?;

        let (rx, tx) = stream.into_split();
        let mut result = Self::streams(
            Box::new(BufReader::new(rx)),
            Box::new(tx),
            None,
            id,
            Some(process),
        );

        // Set the socket address for the client
        if let Ok((client, _)) = &mut result {
            client.socket = Some(socket);
        }

        result
    }

    async fn recv(
        id: DebugAdapterId,
        mut server_rx: UnboundedReceiver<Payload>,
        client_tx: UnboundedSender<(DebugAdapterId, Payload)>,
    ) {
        while let Some(msg) = server_rx.recv().await {
            match msg {
                Payload::Event(ev) => {
                    client_tx
                        .send((id, Payload::Event(ev)))
                        .expect("Failed to send");
                }
                Payload::Response(_) => unreachable!(),
                Payload::Request(req) => {
                    client_tx
                        .send((id, Payload::Request(req)))
                        .expect("Failed to send");
                }
            }
        }
    }

    pub fn id(&self) -> DebugAdapterId {
        self.id
    }

    pub fn connection_type(&self) -> Option<ConnectionType> {
        self.connection_type
    }

    // Internal, called by specific DAP commands when resuming
    pub fn resume_application(&mut self) {
        if let Some(thread_id) = self.thread_id {
            self.thread_states.insert(thread_id, "running".to_string());
            self.stack_frames.remove(&thread_id);
        }
        self.active_frame = None;
        self.thread_id = None;
    }

    /// A handle for issuing requests without borrowing the client.
    pub fn requester(&self) -> Requester {
        Requester {
            server_tx: self.server_tx.clone(),
            request_counter: Arc::clone(&self.request_counter),
        }
    }

    /// Execute a RPC request on the debugger.
    pub fn call<R: crate::types::Request>(
        &self,
        arguments: R::Arguments,
    ) -> impl Future<Output = Result<Value>> + Send + 'static
    where
        R::Arguments: serde::Serialize,
    {
        self.requester().call::<R>(arguments)
    }

    pub async fn request<R: crate::types::Request>(&self, params: R::Arguments) -> Result<R::Result>
    where
        R::Arguments: serde::Serialize,
        R::Result: core::fmt::Debug, // TODO: temporary
    {
        self.requester().request::<R>(params).await
    }

    pub fn reply(
        &self,
        request_seq: u64,
        command: &str,
        result: core::result::Result<Value, Error>,
    ) -> impl Future<Output = Result<()>> {
        let server_tx = self.server_tx.clone();
        let command = command.to_string();

        async move {
            let response = match result {
                Ok(result) => Response {
                    request_seq,
                    command,
                    success: true,
                    message: None,
                    body: Some(result),
                },
                Err(error) => Response {
                    request_seq,
                    command,
                    success: false,
                    message: Some(error.to_string()),
                    body: None,
                },
            };

            server_tx
                .send(Payload::Response(response))
                .map_err(|e| Error::Other(e.into()))?;

            Ok(())
        }
    }

    pub fn capabilities(&self) -> &DebuggerCapabilities {
        self.caps.as_ref().expect("debugger not yet initialized!")
    }

    pub async fn initialize(&mut self, adapter_id: String) -> Result<()> {
        let args = requests::InitializeArguments {
            client_id: Some("hx".to_owned()),
            client_name: Some("helix".to_owned()),
            adapter_id,
            locale: Some("en-us".to_owned()),
            lines_start_at_one: Some(true),
            columns_start_at_one: Some(true),
            path_format: Some("path".to_owned()),
            supports_variable_type: Some(true),
            supports_variable_paging: Some(false),
            supports_run_in_terminal_request: Some(true),
            supports_memory_references: Some(false),
            supports_progress_reporting: Some(false),
            supports_invalidated_event: Some(false),
        };

        let response = self.request::<requests::Initialize>(args).await?;
        self.caps = Some(response);

        Ok(())
    }

    pub fn disconnect(
        &mut self,
        args: Option<DisconnectArguments>,
    ) -> impl Future<Output = Result<Value>> {
        self.connection_type = None;
        self.call::<requests::Disconnect>(args)
    }

    pub fn terminate(
        &mut self,
        args: Option<TerminateArguments>,
    ) -> impl Future<Output = Result<Value>> {
        self.connection_type = None;
        self.call::<requests::Terminate>(args)
    }

    pub fn launch(&mut self, args: serde_json::Value) -> impl Future<Output = Result<Value>> {
        self.connection_type = Some(ConnectionType::Launch);
        self.starting_request_args = Some(args.clone());
        self.call::<requests::Launch>(args)
    }

    pub fn attach(&mut self, args: serde_json::Value) -> impl Future<Output = Result<Value>> {
        self.connection_type = Some(ConnectionType::Attach);
        self.starting_request_args = Some(args.clone());
        self.call::<requests::Attach>(args)
    }

    pub fn restart(&self) -> impl Future<Output = Result<Value>> {
        // Per the DAP spec, the `restart` request's arguments are `RestartArguments`,
        // which nests the latest `launch`/`attach` configuration under an `arguments`
        // key. Send that shape so spec-conformant adapters (e.g. CodeLLDB) accept it;
        // sending the launch args flat makes CodeLLDB fail to parse and panic.
        let args = if let Some(args) = &self.starting_request_args {
            serde_json::json!({ "arguments": args })
        } else {
            Value::Null
        };
        self.call::<requests::Restart>(args)
    }

    /// Restart by tearing the session down and re-issuing the original
    /// `launch`/`attach` request. This is a fallback for adapters that report
    /// `supportsRestartRequest = false` (e.g. the LLVM-14 `lldb-dap`), which
    /// cannot honour the native `restart` request.
    pub fn restart_relaunch(&mut self) -> impl Future<Output = Result<Value>> {
        use futures_util::future::FutureExt;

        let connection_type = self.connection_type;
        let args = self.starting_request_args.clone().unwrap_or(Value::Null);

        let disconnect = self.disconnect(Some(DisconnectArguments {
            restart: Some(true),
            terminate_debuggee: None,
            suspend_debuggee: None,
        }));

        // `disconnect` cleared these; restore them so a subsequent restart still
        // has the configuration to relaunch with.
        self.connection_type = connection_type;
        self.starting_request_args = Some(args.clone());

        let relaunch = match connection_type {
            Some(ConnectionType::Attach) => self.call::<requests::Attach>(args).boxed(),
            _ => self.call::<requests::Launch>(args).boxed(),
        };

        async move {
            disconnect.await?;
            relaunch.await
        }
    }

    pub async fn set_breakpoints(
        &self,
        file: PathBuf,
        breakpoints: Vec<SourceBreakpoint>,
    ) -> Result<Option<Vec<Breakpoint>>> {
        let args = requests::SetBreakpointsArguments {
            source: Source {
                path: Some(file),
                name: None,
                source_reference: None,
                presentation_hint: None,
                origin: None,
                sources: None,
                adapter_data: None,
                checksums: None,
            },
            breakpoints: Some(breakpoints),
            source_modified: Some(false),
        };

        let response = self.request::<requests::SetBreakpoints>(args).await?;

        Ok(response.breakpoints)
    }

    pub async fn configuration_done(&self) -> Result<()> {
        self.request::<requests::ConfigurationDone>(()).await
    }

    pub fn continue_thread(&self, thread_id: ThreadId) -> impl Future<Output = Result<Value>> {
        let args = requests::ContinueArguments { thread_id };

        self.call::<requests::Continue>(args)
    }

    pub async fn stack_trace(
        &self,
        thread_id: ThreadId,
    ) -> Result<(Vec<StackFrame>, Option<usize>)> {
        let args = requests::StackTraceArguments {
            thread_id,
            start_frame: None,
            levels: None,
            format: None,
        };

        let response = self.request::<requests::StackTrace>(args).await?;
        Ok((response.stack_frames, response.total_frames))
    }

    pub fn threads(&self) -> impl Future<Output = Result<Value>> {
        self.call::<requests::Threads>(())
    }

    pub async fn scopes(&self, frame_id: usize) -> Result<Vec<Scope>> {
        self.requester().scopes(frame_id).await
    }

    pub async fn variables(&self, variables_reference: usize) -> Result<Vec<Variable>> {
        self.requester().variables(variables_reference).await
    }

    pub fn step_in(&self, thread_id: ThreadId) -> impl Future<Output = Result<Value>> {
        let args = requests::StepInArguments {
            thread_id,
            target_id: None,
            granularity: None,
        };

        self.call::<requests::StepIn>(args)
    }

    pub fn step_out(&self, thread_id: ThreadId) -> impl Future<Output = Result<Value>> {
        let args = requests::StepOutArguments {
            thread_id,
            granularity: None,
        };

        self.call::<requests::StepOut>(args)
    }

    pub fn next(&self, thread_id: ThreadId) -> impl Future<Output = Result<Value>> {
        let args = requests::NextArguments {
            thread_id,
            granularity: None,
        };

        self.call::<requests::Next>(args)
    }

    pub fn pause(&self, thread_id: ThreadId) -> impl Future<Output = Result<Value>> {
        let args = requests::PauseArguments { thread_id };

        self.call::<requests::Pause>(args)
    }

    pub fn goto_targets(
        &self,
        source: Source,
        line: usize,
        column: Option<usize>,
    ) -> impl Future<Output = Result<Value>> {
        let args = requests::GotoTargetsArguments {
            source,
            line,
            column,
        };

        self.call::<requests::GotoTargets>(args)
    }

    pub fn goto(
        &self,
        thread_id: ThreadId,
        target_id: usize,
    ) -> impl Future<Output = Result<Value>> {
        let args = requests::GotoArguments {
            thread_id,
            target_id,
        };

        self.call::<requests::Goto>(args)
    }

    pub async fn eval(
        &self,
        expression: String,
        frame_id: Option<usize>,
    ) -> Result<requests::EvaluateResponse> {
        // "repl" is what lets an adapter run statements rather than only
        // expressions -- pydevd falls back to exec for this context alone, which
        // is what makes `x = 1` update the frame instead of raising SyntaxError.
        self.eval_with_context(expression, frame_id, "repl").await
    }

    /// Evaluate asking for the value in full. Adapters trim what they hand back
    /// -- pydevd caps strings at 64K -- and the "clipboard" context asks for the
    /// whole thing. It cannot run statements, so it is only for reading values.
    pub async fn eval_full(
        &self,
        expression: String,
        frame_id: Option<usize>,
    ) -> Result<requests::EvaluateResponse> {
        let context = match self.supports_clipboard_context() {
            true => "clipboard",
            false => "repl",
        };

        self.eval_with_context(expression, frame_id, context).await
    }

    pub fn supports_clipboard_context(&self) -> bool {
        self.caps
            .as_ref()
            .and_then(|caps| caps.supports_clipboard_context)
            .unwrap_or_default()
    }

    pub fn supports_set_variable(&self) -> bool {
        self.caps
            .as_ref()
            .and_then(|caps| caps.supports_set_variable)
            .unwrap_or_default()
    }

    pub fn supports_set_expression(&self) -> bool {
        self.caps
            .as_ref()
            .and_then(|caps| caps.supports_set_expression)
            .unwrap_or_default()
    }

    async fn eval_with_context(
        &self,
        expression: String,
        frame_id: Option<usize>,
        context: &str,
    ) -> Result<requests::EvaluateResponse> {
        self.requester()
            .evaluate(expression, frame_id, context, DEFAULT_REQUEST_TIMEOUT)
            .await
    }

    pub async fn set_variable(
        &self,
        variables_reference: usize,
        name: String,
        value: String,
    ) -> Result<requests::SetVariableResponse> {
        self.requester()
            .set_variable(variables_reference, name, value)
            .await
    }

    pub async fn set_expression(
        &self,
        expression: String,
        value: String,
        frame_id: Option<usize>,
    ) -> Result<requests::SetExpressionResponse> {
        self.requester()
            .set_expression(expression, value, frame_id)
            .await
    }

    pub fn set_exception_breakpoints(
        &self,
        filters: Vec<String>,
    ) -> impl Future<Output = Result<Value>> {
        let args = requests::SetExceptionBreakpointsArguments { filters };

        self.call::<requests::SetExceptionBreakpoints>(args)
    }

    pub fn current_stack_frame(&self) -> Option<&StackFrame> {
        self.stack_frames
            .get(&self.thread_id?)?
            .get(self.active_frame?)
    }
}

/// How long to wait for an adapter to answer a request before giving up.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// A `'static` handle for issuing requests to an adapter.
///
/// [`Client::call`] already hands back a future that borrows nothing, which is
/// what lets a single request be driven from a background job. A `Requester`
/// extends that to *chained* requests: it owns both halves a call needs -- the
/// channel to the adapter and the shared sequence counter -- so an async block
/// can decide what to ask next based on the answer it just got, all without
/// holding a borrow on the client.
#[derive(Debug, Clone)]
pub struct Requester {
    server_tx: UnboundedSender<Payload>,
    request_counter: Arc<AtomicU64>,
}

impl Requester {
    fn next_request_id(&self) -> u64 {
        // > The `seq` for the first message sent by a client or debug adapter
        // > is 1, and for each subsequent message is 1 greater than the
        // > previous message sent by that actor
        // <https://microsoft.github.io/debug-adapter-protocol/specification#Base_Protocol_ProtocolMessage>
        self.request_counter.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Execute a RPC request on the debugger.
    pub fn call<R: crate::types::Request>(
        &self,
        arguments: R::Arguments,
    ) -> impl Future<Output = Result<Value>> + Send + 'static
    where
        R::Arguments: serde::Serialize,
    {
        self.call_with_timeout::<R>(arguments, DEFAULT_REQUEST_TIMEOUT)
    }

    /// Execute a RPC request, giving the adapter `timeout` to answer.
    ///
    /// A console evaluation is the reason this is adjustable: running a
    /// deliberate long computation at a REPL is normal, and the deadline that
    /// suits an ordinary request is far too short for it.
    pub fn call_with_timeout<R: crate::types::Request>(
        &self,
        arguments: R::Arguments,
        timeout: Duration,
    ) -> impl Future<Output = Result<Value>> + Send + 'static
    where
        R::Arguments: serde::Serialize,
    {
        let server_tx = self.server_tx.clone();
        let id = self.next_request_id();
        // Serialize up front so the future captures a plain `Value` and stays
        // `Send + 'static` whatever the argument type is.
        let arguments = serde_json::to_value(arguments);

        async move {
            let arguments = Some(arguments?);

            let (callback_tx, mut callback_rx) = channel(1);

            let req = Request {
                back_ch: Some(callback_tx),
                seq: id,
                command: R::COMMAND.to_string(),
                arguments,
            };

            server_tx
                .send(Payload::Request(req))
                .map_err(|e| Error::Other(e.into()))?;

            // TODO: delay other calls until initialize success
            time::timeout(timeout, callback_rx.recv())
                .await
                .map_err(|_| Error::Timeout(id))? // return Timeout
                .ok_or(Error::StreamClosed)?
                .map(|response| response.body.unwrap_or_default())
        }
    }

    pub async fn request<R: crate::types::Request>(&self, params: R::Arguments) -> Result<R::Result>
    where
        R::Arguments: serde::Serialize,
    {
        self.request_with_timeout::<R>(params, DEFAULT_REQUEST_TIMEOUT)
            .await
    }

    pub async fn request_with_timeout<R: crate::types::Request>(
        &self,
        params: R::Arguments,
        timeout: Duration,
    ) -> Result<R::Result>
    where
        R::Arguments: serde::Serialize,
    {
        // a future that resolves into the response
        let json = self.call_with_timeout::<R>(params, timeout).await?;
        let response = serde_json::from_value(json)?;
        Ok(response)
    }

    /// Evaluate `expression` in the given frame under a DAP `context`.
    ///
    /// The context is the caller's to choose because it decides what the
    /// adapter is even willing to do: "repl" is the only one for which pydevd
    /// executes a statement rather than merely evaluating an expression, while
    /// "clipboard" lifts its truncation limits but cannot run statements.
    pub async fn evaluate(
        &self,
        expression: String,
        frame_id: Option<usize>,
        context: &str,
        timeout: Duration,
    ) -> Result<requests::EvaluateResponse> {
        let args = requests::EvaluateArguments {
            expression,
            frame_id,
            context: Some(context.to_string()),
            format: None,
        };

        self.request_with_timeout::<requests::Evaluate>(args, timeout)
            .await
    }

    pub async fn scopes(&self, frame_id: usize) -> Result<Vec<Scope>> {
        let args = requests::ScopesArguments { frame_id };

        let response = self.request::<requests::Scopes>(args).await?;
        Ok(response.scopes)
    }

    pub async fn variables(&self, variables_reference: usize) -> Result<Vec<Variable>> {
        let args = requests::VariablesArguments {
            variables_reference,
            filter: None,
            start: None,
            count: None,
            format: None,
        };

        let response = self.request::<requests::Variables>(args).await?;
        Ok(response.variables)
    }

    pub async fn set_variable(
        &self,
        variables_reference: usize,
        name: String,
        value: String,
    ) -> Result<requests::SetVariableResponse> {
        let args = requests::SetVariableArguments {
            variables_reference,
            name,
            value,
            format: None,
        };

        self.request::<requests::SetVariable>(args).await
    }

    pub async fn set_expression(
        &self,
        expression: String,
        value: String,
        frame_id: Option<usize>,
    ) -> Result<requests::SetExpressionResponse> {
        let args = requests::SetExpressionArguments {
            expression,
            value,
            frame_id,
            format: None,
        };

        self.request::<requests::SetExpression>(args).await
    }
}
