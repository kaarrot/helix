//! Drives a `claude` child process as the answerer for review threads.
//!
//! Helix owns the process, so writing a comment to its stdin *is* the trigger:
//! there is no polling step, and no "go and pick up my comments" gesture.

use std::{
    collections::VecDeque,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

use helix_view::review::{
    agent::{AgentEvent, ReviewAgent},
    ThreadId,
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

fn apply(event: AgentEvent) {
    job::dispatch_blocking(move |editor, _| {
        editor.diff.reviews.apply_agent_event(event);
    });
    schedule_save();
}

/// Whether a write is already pending, so a burst of changes costs one write.
static SAVE_SCHEDULED: AtomicBool = AtomicBool::new(false);

/// Ask for the conversations to be written out shortly.
///
/// Debounced rather than immediate: streamed replies arrive as dozens of deltas
/// a second, and the state directory is commonly on a network filesystem. A
/// crash inside the debounce window loses at most the last moment of typing.
pub fn schedule_save() {
    if SAVE_SCHEDULED.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        SAVE_SCHEDULED.store(false, Ordering::SeqCst);
        job::dispatch_blocking(move |editor, _| editor.save_reviews());
    });
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

#[derive(Debug)]
pub struct ClaudeChildAgent {
    child: Child,
    stdin: Option<ChildStdin>,
    /// Threads awaiting a reply, oldest first. The child answers turns in the
    /// order they were written, so a queue is enough to attribute each reply.
    pending: Arc<Mutex<VecDeque<ThreadId>>>,
}

impl ClaudeChildAgent {
    pub fn spawn(uuid: &str, worktree: &PathBuf) -> anyhow::Result<Self> {
        let resuming = started_marker(uuid).exists();

        let mut child = Command::new("claude")
            .args(child_args(uuid, resuming))
            .current_dir(worktree)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;

        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let pending: Arc<Mutex<VecDeque<ThreadId>>> = Arc::default();

        if let Some(stdout) = stdout {
            let pending = pending.clone();
            let uuid = uuid.to_string();
            // A blocking task rather than a bare thread: dispatching back to the
            // editor goes through `runtime_local!`, which under the integration
            // test configuration resolves via `Handle::current()` and panics
            // outside a runtime context.
            tokio::task::spawn_blocking(move || read_replies(stdout, pending, uuid));
        }

        Ok(Self {
            child,
            stdin,
            pending,
        })
    }
}

/// The child's command line.
///
/// Separated so the permission posture can be asserted in a test: these flags
/// *are* the read-only guarantee, and losing one would quietly hand an agent
/// write access to the tree it is reviewing.
fn child_args(uuid: &str, resuming: bool) -> Vec<String> {
    let mut args: Vec<String> = ["-p", "--input-format", "stream-json"]
        .iter()
        .map(|arg| arg.to_string())
        .collect();
    args.extend(["--output-format", "stream-json", "--verbose"].map(String::from));
    // Without this a reply arrives as one event when it is finished, so a long
    // answer is a spinner followed by a wall of text. With it the text comes in
    // deltas and can be rendered as it is written.
    args.push("--include-partial-messages".into());

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

/// Parse the child's NDJSON and turn it into events on the editor thread.
fn read_replies(
    stdout: std::process::ChildStdout,
    pending: Arc<Mutex<VecDeque<ThreadId>>>,
    uuid: String,
) {
    let reader = BufReader::new(stdout);
    let mut streaming: Option<ThreadId> = None;
    // A child that starts at all announces itself on stdout straight away; one
    // that cannot start writes nothing and exits. That is the whole signal.
    let mut spoke = false;
    let mut marked = false;

    for line in reader.lines() {
        let Ok(line) = line else { break };
        spoke = true;
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };

        match stream_part(&value) {
            // Text deltas. The complete `assistant` message that follows is
            // deliberately ignored: it repeats what the deltas already carried,
            // and appending it too would show the reply twice mid-stream.
            Some(StreamPart::Chunk(text)) => {
                let Some(thread) = streaming.or_else(|| pending.lock().unwrap().front().copied())
                else {
                    continue;
                };
                streaming = Some(thread);
                apply(AgentEvent::Chunk(thread, text));
            }
            Some(StreamPart::Result { is_error, text }) => {
                let Some(thread) = pending.lock().unwrap().pop_front() else {
                    continue;
                };
                streaming = None;
                if !is_error && !marked {
                    // A turn has landed, so the agent now holds this session and
                    // the next editor must continue it rather than create it.
                    marked = true;
                    mark_session_started(&uuid);
                }
                apply(if is_error {
                    AgentEvent::Failed(thread, text)
                } else {
                    AgentEvent::Completed(thread, text)
                });
                finish_one();
            }
            None => {}
        }
    }

    // Nothing at all on stdout means the child could not start, and there are
    // only two ways that happens: we said `--session-id` for a session the agent
    // already has, or `--resume` for one it no longer has. Both are a
    // disagreement between our marker and the agent's memory, so flipping the
    // marker is exactly the repair. The next send starts a fresh child the other
    // way round.
    let failed_to_start = !spoke;
    if failed_to_start {
        let marker = started_marker(&uuid);
        if marker.exists() {
            let _ = std::fs::remove_file(&marker);
        } else {
            let _ = std::fs::create_dir_all(marker.parent().unwrap_or(&marker));
            let _ = std::fs::write(&marker, &uuid);
        }
        // Drop the dead child so the next send spawns rather than writing into
        // a pipe with nothing on the other end.
        job::dispatch_blocking(move |editor, _| editor.diff.agent = None);
    }

    // The child exited or the pipe broke. Anything still queued will never be
    // answered, so say so on the thread rather than leaving it spinning.
    let orphaned: Vec<ThreadId> = pending.lock().unwrap().drain(..).collect();
    for thread in orphaned {
        apply(AgentEvent::Failed(
            thread,
            if failed_to_start {
                "the agent could not start on this conversation; send again to \
                 start it the other way round"
                    .into()
            } else {
                String::from("the agent process ended before replying")
            },
        ));
        finish_one();
    }
}

impl ReviewAgent for ClaudeChildAgent {
    fn send(&mut self, thread: ThreadId, prompt: String) -> anyhow::Result<()> {
        let Some(stdin) = self.stdin.as_mut() else {
            anyhow::bail!("the agent process is not accepting input");
        };

        let payload = serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": prompt },
        });

        // Queue before writing: a fast reply could otherwise arrive before the
        // thread it belongs to is recorded.
        self.pending.lock().unwrap().push_back(thread);
        start_redraw_ticker();

        if let Err(err) = writeln!(stdin, "{payload}").and_then(|()| stdin.flush()) {
            self.pending
                .lock()
                .unwrap()
                .retain(|queued| *queued != thread);
            finish_one();
            return Err(err.into());
        }

        apply(AgentEvent::Started(thread));
        Ok(())
    }

    fn shutdown(&mut self) {
        // Closing stdin is the documented way for the child to finish: it exits
        // cleanly on EOF. Killing it would lose a reply already in flight.
        self.stdin.take();
        let _ = self.child.wait();
    }
}

/// Drives a `grok` child per turn.
///
/// Grok's `-p` / `--prompt-file` is one prompt then exit, and headless mode
/// does not read stdin. Each send is therefore its own process, chained with
/// `--session-id` then `--resume` so they still share one conversation. Drafts
/// queued while a turn is in flight wait until that process finishes.
#[derive(Debug)]
pub struct GrokChildAgent {
    uuid: Arc<String>,
    worktree: Arc<PathBuf>,
    queue: Arc<Mutex<VecDeque<(ThreadId, String)>>>,
    running: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
}

impl GrokChildAgent {
    pub fn new(uuid: String, worktree: PathBuf) -> Self {
        Self {
            uuid: Arc::new(uuid),
            worktree: Arc::new(worktree),
            queue: Arc::default(),
            running: Arc::new(AtomicBool::new(false)),
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }
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

fn grok_kick(
    uuid: Arc<String>,
    worktree: Arc<PathBuf>,
    queue: Arc<Mutex<VecDeque<(ThreadId, String)>>>,
    running: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
) {
    loop {
        if shutdown.load(Ordering::SeqCst) {
            return;
        }
        if running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        let next = queue.lock().unwrap().pop_front();
        let Some((thread, prompt)) = next else {
            running.store(false, Ordering::SeqCst);
            if queue.lock().unwrap().is_empty() || shutdown.load(Ordering::SeqCst) {
                return;
            }
            continue;
        };

        let prompt_file = match write_prompt_file(&prompt) {
            Ok(file) => file,
            Err(err) => {
                apply(AgentEvent::Failed(thread, err.to_string()));
                finish_one();
                running.store(false, Ordering::SeqCst);
                continue;
            }
        };

        let resuming = started_marker(uuid.as_str()).exists();
        let mut child = match Command::new("grok")
            .args(grok_args(uuid.as_str(), resuming, prompt_file.path()))
            .current_dir(worktree.as_path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(err) => {
                apply(AgentEvent::Failed(
                    thread,
                    format!("Could not start grok: {err}"),
                ));
                finish_one();
                running.store(false, Ordering::SeqCst);
                continue;
            }
        };

        let stdout = child.stdout.take();
        let uuid_for_child = uuid.clone();
        let worktree_for_child = worktree.clone();
        let queue_for_child = queue.clone();
        let running_for_child = running.clone();
        let shutdown_for_child = shutdown.clone();
        tokio::task::spawn_blocking(move || {
            if let Some(stdout) = stdout {
                read_grok_replies(stdout, thread, uuid_for_child.as_str());
            } else {
                apply(AgentEvent::Failed(
                    thread,
                    String::from("the agent process ended before replying"),
                ));
                finish_one();
            }
            let _ = child.wait();
            drop(prompt_file);
            running_for_child.store(false, Ordering::SeqCst);
            if !shutdown_for_child.load(Ordering::SeqCst) {
                grok_kick(
                    uuid_for_child,
                    worktree_for_child,
                    queue_for_child,
                    running_for_child,
                    shutdown_for_child,
                );
            }
        });
        return;
    }
}

fn write_prompt_file(prompt: &str) -> std::io::Result<tempfile::NamedTempFile> {
    let mut file = tempfile::NamedTempFile::new()?;
    file.write_all(prompt.as_bytes())?;
    file.flush()?;
    Ok(file)
}

/// One Grok process is one turn, so the thread is known before any line arrives.
fn read_grok_replies(stdout: std::process::ChildStdout, thread: ThreadId, uuid: &str) {
    let reader = BufReader::new(stdout);
    let mut spoke = false;
    let mut finished = false;

    for line in reader.lines() {
        let Ok(line) = line else { break };
        spoke = true;
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        match stream_part(&value) {
            Some(StreamPart::Chunk(text)) => {
                apply(AgentEvent::Chunk(thread, text));
            }
            Some(StreamPart::Result { is_error, text }) => {
                if !is_error {
                    mark_session_started(uuid);
                }
                apply(if is_error {
                    AgentEvent::Failed(thread, text)
                } else {
                    AgentEvent::Completed(thread, text)
                });
                finish_one();
                finished = true;
                break;
            }
            None => {}
        }
    }

    if !finished {
        apply(AgentEvent::Failed(
            thread,
            if spoke {
                String::from("the agent process ended before replying")
            } else {
                "the agent could not start on this conversation; send again to \
                 start it the other way round"
                    .into()
            },
        ));
        finish_one();
        if !spoke {
            let marker = started_marker(uuid);
            if marker.exists() {
                let _ = std::fs::remove_file(&marker);
            } else {
                mark_session_started(uuid);
            }
        }
    }
}

impl ReviewAgent for GrokChildAgent {
    fn send(&mut self, thread: ThreadId, prompt: String) -> anyhow::Result<()> {
        if self.shutdown.load(Ordering::SeqCst) {
            anyhow::bail!("the agent process is not accepting input");
        }
        self.queue.lock().unwrap().push_back((thread, prompt));
        start_redraw_ticker();
        apply(AgentEvent::Started(thread));
        grok_kick(
            self.uuid.clone(),
            self.worktree.clone(),
            self.queue.clone(),
            self.running.clone(),
            self.shutdown.clone(),
        );
        Ok(())
    }

    fn shutdown(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        let leftover: Vec<ThreadId> = self
            .queue
            .lock()
            .unwrap()
            .drain(..)
            .map(|(thread, _)| thread)
            .collect();
        for thread in leftover {
            apply(AgentEvent::Failed(
                thread,
                String::from("the agent process ended before replying"),
            ));
            finish_one();
        }
        // The in-flight child is waited on by its blocking task. Spinning here
        // would block the editor thread on a network turn; dropping is enough
        // to refuse further sends, and the running process is allowed to finish.
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
}
