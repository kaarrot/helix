//! The seam between review threads and whatever answers them.
//!
//! Helix owns the agent process rather than the other way round: sending a
//! comment starts a turn, and the reply is applied back onto that same thread.
//! The trait exists so that a different transport can replace the child without
//! touching the store, the rendering or the keymap.

use super::ThreadId;
use std::fmt;

/// Which child answers review comments.
///
/// Chosen with `:review-session [claude|grok]`, not config. The two CLIs take
/// the prompt differently, so the editor picks the spawn path from this. Each
/// comment is its own conversation either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReviewAgentKind {
    #[default]
    Claude,
    Grok,
}

impl ReviewAgentKind {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "claude" => Some(Self::Claude),
            "grok" => Some(Self::Grok),
            _ => None,
        }
    }
}

impl fmt::Display for ReviewAgentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Claude => "claude",
            Self::Grok => "grok",
        })
    }
}

/// Something that can answer review comments.
///
/// Implementations deliver replies by mutating the store from the editor
/// thread; they do not report back through this trait. A one-shot request and
/// response would not fit, because a reply arrives in pieces and long after the
/// call that asked for it.
pub trait ReviewAgent: Send + std::fmt::Debug {
    /// Ask for a reply to `thread` in `session`.
    ///
    /// `prompt` is the fully composed message, context included — the agent is
    /// not expected to go looking for it. `session` belongs to that thread
    /// alone: the first turn creates it (`--session-id`) and a later turn
    /// resumes it (`--resume`), so two comments never share a conversation and
    /// a reply cannot be written onto the wrong one.
    fn send(&mut self, thread: ThreadId, session: String, prompt: String) -> anyhow::Result<()>;

    /// Stop accepting work and let the underlying process finish.
    fn shutdown(&mut self);
}

/// What an agent reports back about a thread it is answering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentEvent {
    /// The request went out; a reply is being composed.
    Started(ThreadId),
    /// Part of a reply arrived. Rendering these as they come is what stops a
    /// long answer looking like nothing is happening.
    Chunk(ThreadId, String),
    /// The reply is complete. Carries the full text, since the chunks may have
    /// been coalesced or dropped.
    Completed(ThreadId, String),
    /// The request failed. The text is shown in place of a reply rather than
    /// only in the statusline, so the failure stays attached to what asked.
    Failed(ThreadId, String),
}
