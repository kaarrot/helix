//! Drives a `claude` or `grok` child as the answerer for one review thread.
//!
//! Each thread has its own conversation id. A turn is one process: the first
//! creates that id with `--session-id`, and a follow-up resumes it with
//! `--resume`. Two threads therefore cannot see each other's comments or take
//! each other's reply, and their turns run at the same time. A follow-up sent
//! while its thread's reply is still arriving is held as a draft until that
//! reply lands (see [`apply_event`]), so one conversation is never driven by two
//! turns at once and two replies are never written into one entry.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

use helix_view::{
    review::{
        agent::{AgentEvent, ReviewAgent},
        ThreadId,
    },
    Editor,
};
use once_cell::sync::Lazy;

use crate::{job, ui::Spinner};

/// Frames advance with elapsed time, so one long-lived spinner animates every
/// waiting thread correctly. There is no per-thread state to start, stop or
/// leak — the thread's own `awaiting` flag decides whether it is drawn.
static SPINNER: Lazy<Spinner> = Lazy::new(|| {
    let mut spinner = Spinner::dots(80);
    spinner.start();
    spinner
});

/// Replies in flight. Only used to decide whether to keep asking for redraws;
/// the authoritative state is each thread's `awaiting` flag.
static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

pub fn spinner_frame() -> Option<&'static str> {
    (IN_FLIGHT.load(Ordering::Relaxed) > 0)
        .then(|| SPINNER.frame())
        .flatten()
}

/// Keep asking for redraws while anything is in flight, so the spinner advances
/// during the silence between sending and the first chunk coming back.
fn start_redraw_ticker() {
    // Only the transition into "busy" starts a ticker, so they cannot pile up.
    if IN_FLIGHT.fetch_add(1, Ordering::SeqCst) > 0 {
        return;
    }
    tokio::spawn(async move {
        while IN_FLIGHT.load(Ordering::SeqCst) > 0 {
            helix_event::request_redraw();
            tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        }
        // One last frame so the spinner is replaced rather than left mid-spin.
        helix_event::request_redraw();
    });
}

fn finish_one() {
    let _ = IN_FLIGHT.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
        Some(n.saturating_sub(1))
    });
}

/// Unit tests spawn a child without an editor job queue. Dispatching would
/// block forever on that queue, so those tests set this and drop the event.
#[cfg(test)]
static SUPPRESS_DISPATCH: AtomicBool = AtomicBool::new(false);

fn apply(session: &str, event: AgentEvent) {
    #[cfg(test)]
    if SUPPRESS_DISPATCH.load(Ordering::SeqCst) {
        return;
    }
    let session = session.to_string();
    job::dispatch_blocking(move |editor, _| apply_event(editor, &session, event));
    schedule_save();
}

/// Fold one event from the turn answering `session` into the editor.
///
/// A finished turn frees its conversation, so a follow-up that was sent while
/// the reply was still arriving goes out now.
pub fn apply_event(editor: &mut Editor, session: &str, event: AgentEvent) {
    let finished = match &event {
        AgentEvent::Completed(id, _) | AgentEvent::Failed(id, _) => Some(*id),
        AgentEvent::Started(_) | AgentEvent::Chunk(..) => None,
    };
    editor.diff.reviews.apply_agent_event_for(session, event);
    if let Some(id) = finished {
        crate::commands::review::send_queued(editor, id);
    }
}

/// Whether a write is already pending, so a burst of changes costs one write.
static SAVE_SCHEDULED: AtomicBool = AtomicBool::new(false);

/// Bumped when a pending debounce is cancelled, so a sleeping save does not
/// dispatch after the editor has already flushed (or gone).
static SAVE_GENERATION: AtomicUsize = AtomicUsize::new(0);

/// Ask for the conversations to be written out shortly.
///
/// Debounced rather than immediate: streamed replies arrive as dozens of deltas
/// a second, and the state directory is commonly on a network filesystem. A
/// crash inside the debounce window loses at most the last moment of typing.
/// `:q` flushes immediately via [`cancel_scheduled_save`] plus `save_reviews`.
pub fn schedule_save() {
    if SAVE_SCHEDULED.swap(true, Ordering::SeqCst) {
        return;
    }
    let generation = SAVE_GENERATION.load(Ordering::SeqCst);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        SAVE_SCHEDULED.store(false, Ordering::SeqCst);
        if SAVE_GENERATION.load(Ordering::SeqCst) != generation {
            return;
        }
        job::dispatch_blocking(move |editor, _| editor.save_reviews());
    });
}

/// Drop a pending debounced save so it cannot dispatch after the editor is gone.
///
/// The caller must `save_reviews` itself: this only invalidates the timer.
pub fn cancel_scheduled_save() {
    SAVE_GENERATION.fetch_add(1, Ordering::SeqCst);
    SAVE_SCHEDULED.store(false, Ordering::SeqCst);
}

/// Whether this conversation has been started before, which decides between
/// `--session-id` (create) and `--resume` (continue).
///
/// Tracked with our own marker rather than by looking for the agent's transcript
/// file, whose location is an internal detail we should not depend on.
///
/// Written only once a turn has actually completed, because that is when the
/// agent starts remembering the session. Measured: spawning with `--session-id`
/// and closing stdin without sending anything leaves no session behind, and the
/// same id can be created again afterwards. Writing the marker at spawn would
/// therefore claim a session that does not exist, and the next start would fail.
fn started_marker(uuid: &str) -> PathBuf {
    helix_loader::state_dir()
        .join("review")
        .join(format!("{uuid}.started"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CliKind {
    Claude,
    Grok,
}

impl CliKind {
    fn program(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Grok => "grok",
        }
    }
}

/// One process per in-flight thread.
///
/// Different conversations run together. A second turn for a conversation that
/// already has a process waits in [`TurnState::waiting`] until that process
/// exits, then resumes the same id.
#[derive(Debug, Clone)]
struct TurnState {
    kind: CliKind,
    program: String,
    worktree: Arc<PathBuf>,
    /// Prompts waiting on a session, oldest first. Keyed by that thread's UUID.
    waiting: Arc<Mutex<HashMap<String, VecDeque<(ThreadId, String)>>>>,
    /// Sessions whose process has been started and not yet reaped.
    running: Arc<Mutex<HashSet<String>>>,
    /// Set by shutdown. Turns already running are allowed to finish; nothing
    /// new is started, and anything still queued is failed.
    stopped: Arc<AtomicBool>,
}

impl TurnState {
    fn new(kind: CliKind, worktree: PathBuf) -> Self {
        Self::with_program(kind, kind.program().to_string(), worktree)
    }

    fn with_program(kind: CliKind, program: String, worktree: PathBuf) -> Self {
        Self {
            kind,
            program,
            worktree: Arc::new(worktree),
            waiting: Arc::default(),
            running: Arc::default(),
            stopped: Arc::default(),
        }
    }

    fn send(&self, thread: ThreadId, session: String, prompt: String) -> anyhow::Result<()> {
        // The in-flight count is taken under the same lock as the enqueue, so a
        // shutdown that drains the queue cannot miss it or count it twice.
        if !self.queue(&session, thread, prompt, true) {
            anyhow::bail!("the agent process is not accepting input");
        }
        apply(&session, AgentEvent::Started(thread));
        self.kick();
        Ok(())
    }

    fn shutdown(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        let drained: Vec<(String, ThreadId)> = {
            let mut waiting = self.waiting.lock().unwrap();
            waiting
                .drain()
                .flat_map(|(session, queue)| {
                    queue
                        .into_iter()
                        .map(move |(thread, _)| (session.clone(), thread))
                })
                .collect()
        };
        for (session, thread) in drained {
            apply(
                &session,
                AgentEvent::Failed(
                    thread,
                    String::from("the agent process ended before replying"),
                ),
            );
            finish_one();
        }
        // The in-flight child is waited on by its blocking task. Waiting here
        // would freeze the editor for the rest of a network turn.
    }

    /// Queue `prompt` unless shutdown has won the race.
    ///
    /// `count` counts the turn as in flight. That has to happen before this
    /// lock is released: shutdown drains under the same lock and drops one
    /// count per prompt it takes.
    fn queue(&self, session: &str, thread: ThreadId, prompt: String, count: bool) -> bool {
        if self.stopped.load(Ordering::SeqCst) {
            return false;
        }
        let mut waiting = self.waiting.lock().unwrap();
        if self.stopped.load(Ordering::SeqCst) {
            return false;
        }
        waiting
            .entry(session.to_string())
            .or_default()
            .push_back((thread, prompt));
        if count {
            start_redraw_ticker();
        }
        true
    }

    #[cfg(test)]
    fn push_waiting(&self, session: &str, thread: ThreadId, prompt: String) -> bool {
        self.queue(session, thread, prompt, false)
    }

    /// Take the next turn whose session has no process yet.
    ///
    /// The session is marked running before the lock is released, so two kicks
    /// cannot start it twice.
    fn pop_ready(&self) -> Option<(String, ThreadId, String)> {
        let mut waiting = self.waiting.lock().unwrap();
        let mut running = self.running.lock().unwrap();
        if self.stopped.load(Ordering::SeqCst) {
            return None;
        }
        let session = waiting
            .iter()
            .find(|(session, queue)| !queue.is_empty() && !running.contains(*session))
            .map(|(session, _)| session.clone())?;
        let (thread, prompt) = waiting.get_mut(&session)?.pop_front()?;
        if waiting.get(&session).is_some_and(|queue| queue.is_empty()) {
            waiting.remove(&session);
        }
        running.insert(session.clone());
        Some((session, thread, prompt))
    }

    fn release(&self, session: &str) {
        self.running.lock().unwrap().remove(session);
    }

    fn kick(&self) {
        while let Some((session, thread, prompt)) = self.pop_ready() {
            self.launch(&session, thread, prompt);
        }
    }

    fn fail(&self, session: &str, thread: ThreadId, message: String) {
        self.release(session);
        apply(session, AgentEvent::Failed(thread, message));
        finish_one();
    }

    fn launch(&self, session: &str, thread: ThreadId, prompt: String) {
        let resuming = started_marker(session).exists();
        let prompt_file = if self.kind == CliKind::Grok {
            match write_prompt_file(&prompt) {
                Ok(file) => Some(file),
                Err(err) => {
                    self.fail(session, thread, err.to_string());
                    return;
                }
            }
        } else {
            None
        };

        let args = match &prompt_file {
            Some(file) => grok_args(session, resuming, file.path()),
            None => child_args(session, resuming),
        };

        let mut command = Command::new(&self.program);
        command
            .args(&args)
            .current_dir(self.worktree.as_path())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if prompt_file.is_some() {
            command.stdin(Stdio::null());
        } else {
            command.stdin(Stdio::piped());
        }

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(err) => {
                self.fail(
                    session,
                    thread,
                    format!("Could not start {}: {err}", self.program),
                );
                return;
            }
        };

        // Claude reads the prompt from stdin and exits on EOF. Writing it on
        // another task keeps a large prompt from filling the pipe while this
        // task is still stuck before it starts reading stdout.
        if prompt_file.is_none() {
            let Some(mut stdin) = child.stdin.take() else {
                let _ = child.kill();
                tokio::task::spawn_blocking(move || {
                    let _ = child.wait();
                });
                self.fail(
                    session,
                    thread,
                    String::from("the agent process is not accepting input"),
                );
                return;
            };
            tokio::task::spawn_blocking(move || {
                let _ = stdin.write_all(prompt.as_bytes());
                let _ = stdin.flush();
            });
        }

        let stdout = child.stdout.take();
        let session = session.to_string();
        let state = self.clone();
        tokio::task::spawn_blocking(move || {
            if let Some(stdout) = stdout {
                read_turn(stdout, thread, &session);
            } else {
                apply(
                    &session,
                    AgentEvent::Failed(
                        thread,
                        String::from("the agent process ended before replying"),
                    ),
                );
                finish_one();
            }
            let _ = child.wait();
            drop(prompt_file);
            state.release(&session);
            if !state.stopped.load(Ordering::SeqCst) {
                state.kick();
            }
        });
    }
}

/// Drives `claude -p` once per turn. The prompt is the stdin of that process.
#[derive(Debug)]
pub struct ClaudeChildAgent {
    turns: TurnState,
}

impl ClaudeChildAgent {
    pub fn new(worktree: PathBuf) -> Self {
        Self {
            turns: TurnState::new(CliKind::Claude, worktree),
        }
    }

    #[cfg(test)]
    fn with_program(program: String, worktree: PathBuf) -> Self {
        Self {
            turns: TurnState::with_program(CliKind::Claude, program, worktree),
        }
    }
}

impl ReviewAgent for ClaudeChildAgent {
    fn send(&mut self, thread: ThreadId, session: String, prompt: String) -> anyhow::Result<()> {
        self.turns.send(thread, session, prompt)
    }

    fn shutdown(&mut self) {
        self.turns.shutdown();
    }
}

/// Drives `grok --prompt-file` once per turn.
#[derive(Debug)]
pub struct GrokChildAgent {
    turns: TurnState,
}

impl GrokChildAgent {
    pub fn new(worktree: PathBuf) -> Self {
        Self {
            turns: TurnState::new(CliKind::Grok, worktree),
        }
    }
}

impl ReviewAgent for GrokChildAgent {
    fn send(&mut self, thread: ThreadId, session: String, prompt: String) -> anyhow::Result<()> {
        self.turns.send(thread, session, prompt)
    }

    fn shutdown(&mut self) {
        self.turns.shutdown();
    }
}

/// Command line for one Claude turn.
///
/// The prompt is written to stdin and the pipe is closed; `-p` prints the reply
/// and exits. Separated so the permission posture can be asserted in a test:
/// these flags *are* the full-access grant, and losing one would quietly change
/// what the agent is allowed to do to the tree it is reviewing.
fn child_args(uuid: &str, resuming: bool) -> Vec<String> {
    let mut args = vec![
        "-p".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        // Without this a reply arrives as one event when it is finished, so a
        // long answer is a spinner followed by a wall of text. With it the text
        // comes in deltas and can be rendered as it is written.
        "--include-partial-messages".into(),
    ];

    // A session id can only be used to *create*; reusing one errors with
    // "Session ID ... is already in use", so continuing needs --resume.
    if resuming {
        args.extend(["--resume".into(), uuid.to_string()]);
    } else {
        args.extend(["--session-id".into(), uuid.to_string()]);
    }

    // Full access, chosen deliberately: a comment on a line usually implies a
    // change to it, and an agent that can only describe the fix while the
    // reviewer applies it by hand is doing half the job.
    //
    // This is a real grant. The agent can edit the worktree and run commands
    // with no approval step, because a process Helix spawned has no terminal to
    // ask in. The protections that remain are that it runs in the worktree, its
    // edits land in the diff being reviewed where they are visible, and the
    // conversation is recorded.
    args.extend(["--permission-mode".into(), "bypassPermissions".into()]);

    args
}

/// A parsed line from Claude Code `stream-json` or Grok `streaming-messages-json`.
#[derive(Debug)]
enum StreamPart {
    Chunk(String),
    Result { is_error: bool, text: String },
}

/// Pull a text delta or a terminal result out of one NDJSON object.
///
/// Claude Code wraps deltas as `stream_event` / `content_block_delta`. Grok's
/// `--include-partial-messages` may emit that wrapping or the inner event
/// unwrapped. The finished-turn `result` line is the same on both.
fn stream_part(value: &serde_json::Value) -> Option<StreamPart> {
    match value.get("type").and_then(|t| t.as_str()) {
        Some("stream_event") => {
            if value.pointer("/event/type").and_then(|t| t.as_str()) != Some("content_block_delta")
            {
                return None;
            }
            value
                .pointer("/event/delta/text")
                .and_then(|t| t.as_str())
                .filter(|text| !text.is_empty())
                .map(|text| StreamPart::Chunk(text.to_string()))
        }
        Some("content_block_delta") => value
            .pointer("/delta/text")
            .and_then(|t| t.as_str())
            .filter(|text| !text.is_empty())
            .map(|text| StreamPart::Chunk(text.to_string())),
        Some("result") => Some(StreamPart::Result {
            is_error: value
                .get("is_error")
                .and_then(|e| e.as_bool())
                .unwrap_or(false),
            text: value
                .get("result")
                .and_then(|r| r.as_str())
                .unwrap_or_default()
                .to_string(),
        }),
        _ => None,
    }
}

fn mark_session_started(uuid: &str) {
    let marker = started_marker(uuid);
    let _ = std::fs::create_dir_all(marker.parent().unwrap_or(&marker));
    let _ = std::fs::write(&marker, uuid);
}

/// Command line for one Grok turn. `prompt_file` is the path `--prompt-file`
/// will read; tests pass a placeholder.
fn grok_args(uuid: &str, resuming: bool, prompt_file: &Path) -> Vec<String> {
    let mut args = vec![
        "--prompt-file".into(),
        prompt_file.to_string_lossy().into_owned(),
        // Review prompts are already fully composed; do not rewrite them.
        "--verbatim".into(),
        "--output-format".into(),
        "streaming-messages-json".into(),
        "--include-partial-messages".into(),
        "--permission-mode".into(),
        "bypassPermissions".into(),
    ];
    if resuming {
        args.extend(["--resume".into(), uuid.to_string()]);
    } else {
        args.extend(["--session-id".into(), uuid.to_string()]);
    }
    args
}

fn write_prompt_file(prompt: &str) -> std::io::Result<tempfile::NamedTempFile> {
    let mut file = tempfile::NamedTempFile::new()?;
    file.write_all(prompt.as_bytes())?;
    file.flush()?;
    Ok(file)
}

/// One process is one turn, so the thread is known before any line arrives.
///
/// Lines after the terminal `result` are drained and ignored. Stopping at the
/// result would leave the child blocked once the stdout pipe filled, and
/// `wait` would never return.
fn read_turn(stdout: std::process::ChildStdout, thread: ThreadId, session: &str) {
    let reader = BufReader::new(stdout);
    let mut spoke = false;
    let mut finished = false;

    for line in reader.lines() {
        let Ok(line) = line else { break };
        spoke = true;
        if finished {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        match stream_part(&value) {
            Some(StreamPart::Chunk(text)) => {
                apply(session, AgentEvent::Chunk(thread, text));
            }
            Some(StreamPart::Result { is_error, text }) => {
                if !is_error {
                    mark_session_started(session);
                }
                apply(
                    session,
                    if is_error {
                        AgentEvent::Failed(thread, text)
                    } else {
                        AgentEvent::Completed(thread, text)
                    },
                );
                finish_one();
                finished = true;
            }
            None => {}
        }
    }

    if !finished {
        apply(
            session,
            AgentEvent::Failed(
                thread,
                if spoke {
                    String::from("the agent process ended before replying")
                } else {
                    "the agent could not start on this conversation; send again to \
                     start it the other way round"
                        .into()
                },
            ),
        );
        finish_one();
        if !spoke {
            // Nothing at all on stdout means the child could not start, and
            // there are only two ways that happens: we said `--session-id` for
            // a session the agent already has, or `--resume` for one it no
            // longer has. Both are a disagreement between our marker and the
            // agent's memory, so flipping the marker is the repair. The next
            // send starts a fresh child the other way round.
            let marker = started_marker(session);
            if marker.exists() {
                let _ = std::fs::remove_file(&marker);
            } else {
                mark_session_started(session);
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn the_child_is_given_full_access_deliberately() {
        // The grant is explicit rather than incidental: if this ever changes,
        // it should be because someone meant it to.
        for resuming in [false, true] {
            let args = child_args("11111111-2222-5333-8444-555555555555", resuming);
            assert!(
                args.windows(2)
                    .any(|w| w[0] == "--permission-mode" && w[1] == "bypassPermissions"),
                "the agent is meant to be able to act on comments: {args:?}"
            );
            assert!(
                !args.iter().any(|arg| arg == "--disallowed-tools"),
                "no tool should be withheld: {args:?}"
            );
        }
    }

    #[test]
    fn a_session_is_created_once_and_resumed_after() {
        let fresh = child_args("11111111-2222-5333-8444-555555555555", false);
        assert!(fresh.iter().any(|arg| arg == "--session-id"));
        assert!(!fresh.iter().any(|arg| arg == "--resume"));

        // Reusing a session id fails with "already in use", so a second run has
        // to resume instead.
        let again = child_args("11111111-2222-5333-8444-555555555555", true);
        assert!(again.iter().any(|arg| arg == "--resume"));
        assert!(!again.iter().any(|arg| arg == "--session-id"));
    }

    #[test]
    fn replies_are_streamed_rather_than_arriving_whole() {
        let args = child_args("11111111-2222-5333-8444-555555555555", false);
        assert!(args.iter().any(|arg| arg == "--include-partial-messages"));
    }

    #[test]
    fn claude_is_one_prompt_then_exit_not_a_shared_stream() {
        let args = child_args("11111111-2222-5333-8444-555555555555", false);
        assert!(args.iter().any(|arg| arg == "-p"), "{args:?}");
        assert!(
            !args.iter().any(|arg| arg == "--input-format"),
            "a shared stdin stream is what let one reply land on another comment: {args:?}"
        );
    }

    #[test]
    fn different_threads_run_together_and_one_thread_stays_in_order() {
        let turns = TurnState::new(CliKind::Claude, PathBuf::from("."));
        let first = "00000000-0000-4000-8000-000000000001";
        let second = "00000000-0000-4000-8000-000000000002";
        assert!(turns.push_waiting(first, ThreadId(1), "a1".into()));
        assert!(turns.push_waiting(first, ThreadId(1), "a2".into()));
        assert!(turns.push_waiting(second, ThreadId(2), "b1".into()));

        let mut ready = vec![turns.pop_ready().unwrap(), turns.pop_ready().unwrap()];
        assert!(
            turns.pop_ready().is_none(),
            "a session with a turn already running waits"
        );
        ready.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(ready[0].2, "a1");
        assert_eq!(ready[1].2, "b1");

        turns.release(first);
        let follow = turns.pop_ready().unwrap();
        assert_eq!(follow.0, first);
        assert_eq!(follow.2, "a2");
        assert!(turns.pop_ready().is_none());

        turns.stopped.store(true, Ordering::SeqCst);
        assert!(
            !turns.push_waiting(first, ThreadId(1), "later".into()),
            "shutdown must refuse another turn"
        );
    }

    fn grok_sample(resuming: bool) -> Vec<String> {
        grok_args(
            "11111111-2222-5333-8444-555555555555",
            resuming,
            Path::new("/tmp/review-prompt"),
        )
    }

    #[test]
    fn grok_is_one_prompt_file_then_exit_not_stdin() {
        let args = grok_sample(false);
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--prompt-file" && w[1] == "/tmp/review-prompt"),
            "{args:?}"
        );
        assert!(args.iter().any(|arg| arg == "--verbatim"));
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--output-format" && w[1] == "streaming-messages-json"),
            "{args:?}"
        );
        assert!(!args.iter().any(|arg| arg == "--input-format"));
        assert!(!args.iter().any(|arg| arg == "-p" || arg == "--single"));
    }

    #[test]
    fn grok_is_given_full_access_deliberately() {
        for resuming in [false, true] {
            let args = grok_sample(resuming);
            assert!(
                args.windows(2)
                    .any(|w| w[0] == "--permission-mode" && w[1] == "bypassPermissions"),
                "{args:?}"
            );
        }
    }

    #[test]
    fn grok_session_is_created_once_and_resumed_after() {
        let fresh = grok_sample(false);
        assert!(fresh.iter().any(|arg| arg == "--session-id"));
        assert!(!fresh.iter().any(|arg| arg == "--resume"));

        let again = grok_sample(true);
        assert!(again.iter().any(|arg| arg == "--resume"));
        assert!(!again.iter().any(|arg| arg == "--session-id"));
    }

    #[test]
    fn grok_replies_are_streamed_rather_than_arriving_whole() {
        let args = grok_sample(false);
        assert!(args.iter().any(|arg| arg == "--include-partial-messages"));
    }

    #[test]
    fn stream_part_accepts_claude_wrapping_and_unwrapped_deltas() {
        let wrapped: serde_json::Value = serde_json::json!({
            "type": "stream_event",
            "event": { "type": "content_block_delta", "delta": { "text": "hi" } },
        });
        match stream_part(&wrapped) {
            Some(StreamPart::Chunk(text)) => assert_eq!(text, "hi"),
            other => panic!("{other:?}"),
        }

        let unwrapped: serde_json::Value = serde_json::json!({
            "type": "content_block_delta",
            "delta": { "text": "there" },
        });
        match stream_part(&unwrapped) {
            Some(StreamPart::Chunk(text)) => assert_eq!(text, "there"),
            other => panic!("{other:?}"),
        }

        let result: serde_json::Value = serde_json::json!({
            "type": "result",
            "is_error": false,
            "result": "done",
        });
        match stream_part(&result) {
            Some(StreamPart::Result { is_error, text }) => {
                assert!(!is_error);
                assert_eq!(text, "done");
            }
            other => panic!("{other:?}"),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn claude_shutdown_does_not_wait_for_the_child() {
        // A turn that ignores stdin and stays alive. Waiting on it in shutdown
        // would freeze the editor for the full duration.
        struct Cleanup {
            pid: u32,
            running: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
        }

        impl Drop for Cleanup {
            fn drop(&mut self) {
                if self.pid != 0 {
                    let _ = Command::new("kill")
                        .args(["-9", &self.pid.to_string()])
                        .status();
                }
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
                while !self.running.lock().unwrap().is_empty()
                    && std::time::Instant::now() < deadline
                {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                SUPPRESS_DISPATCH.store(false, Ordering::SeqCst);
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let pid_path = dir.path().join("pid");
        let program = dir.path().join("claude");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\necho $$ > '{}'\nexec sleep 30\n",
                pid_path.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(
            &program,
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();

        SUPPRESS_DISPATCH.store(true, Ordering::SeqCst);
        let mut agent = ClaudeChildAgent::with_program(
            program.to_string_lossy().into_owned(),
            dir.path().to_path_buf(),
        );
        // Declared after the agent so it drops first: kill the child, let the
        // reader finish (its dispatch is suppressed), then clear the flag.
        let mut cleanup = Cleanup {
            pid: 0,
            running: agent.turns.running.clone(),
        };
        agent
            .turns
            .send(
                ThreadId(1),
                "11111111-2222-4333-8444-555555555555".into(),
                "hi".into(),
            )
            .unwrap();

        let started = std::time::Instant::now();
        let pid = loop {
            if let Ok(text) = std::fs::read_to_string(&pid_path) {
                if let Ok(pid) = text.trim().parse::<u32>() {
                    break pid;
                }
            }
            if started.elapsed() > std::time::Duration::from_secs(2) {
                panic!("the turn did not start");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        cleanup.pid = pid;

        let start = std::time::Instant::now();
        agent.shutdown();
        let elapsed = start.elapsed();
        let alive = Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .unwrap()
            .success();

        assert!(alive, "shutdown must leave the turn running");
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "shutdown blocked on the child for {elapsed:?}"
        );
    }
}
