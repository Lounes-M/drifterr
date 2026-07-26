//! The normalized format — the central contract of the whole system.
//!
//! Every channel (proxy, file watcher, browser extension) produces *exactly*
//! this and nothing else. The engine is channel-agnostic: it only ever sees a
//! [`Conversation`]. Adding a new source means writing an adapter that emits
//! this shape — never touching the engine.
//!
//! The serde representation matches the TypeScript contract in the brief so the
//! browser extension and the Rust core agree on the wire format byte-for-byte.

use serde::{Deserialize, Serialize};

/// Who produced a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    Tool,
}

/// A single message in the conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turn {
    pub index: usize,
    pub role: Role,
    pub content: String,
    /// Exact when sourced from the proxy, estimated otherwise. See
    /// [`ContextState::exact`].
    pub tokens: usize,
    /// Unix epoch milliseconds.
    pub timestamp: i64,
}

/// The model's context-window occupancy at the point of capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextState {
    /// Total window size of the model in tokens (known per model).
    #[serde(rename = "windowSize")]
    pub window_size: usize,
    /// Tokens currently used. Exact via proxy, estimated elsewhere.
    #[serde(rename = "usedTokens")]
    pub used_tokens: usize,
    /// `true` *only* when sourced from the proxy. The UI uses this to decide
    /// whether to present the saturation percentage as truth or estimate. We
    /// never lie about precision.
    pub exact: bool,
    /// Whether `used_tokens` describes the *live* context at all.
    ///
    /// Distinct from [`exact`](Self::exact), which is about precision. This is about
    /// meaning: an append-only transcript that outlives a context compaction records
    /// more conversation than the model can be holding, and nothing in it says where
    /// the reset happened. Summing it answers "how much was ever said", not "how full
    /// is the window" — a different question with a superficially similar answer.
    ///
    /// When this is false, occupancy is a **lower bound** and saturation must not drive
    /// RED. Measured on a real 178-turn Claude Code session: the previous text-only sum
    /// reported 8% for a context that had already compacted (so the hard signal was
    /// effectively switched off), while naively counting everything reports over 100%
    /// (a false RED — the one unforgivable failure per `CLAUDE.md`). Neither is
    /// honest, so the adapter says "unknown" instead and the signal abstains.
    ///
    /// Defaults to `true` so every existing fixture and eval case stays valid.
    #[serde(rename = "occupancyKnown", default = "default_true")]
    pub occupancy_known: bool,
    #[serde(rename = "toolCallCount")]
    pub tool_call_count: usize,
}

fn default_true() -> bool {
    true
}

/// Where a conversation came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Proxy,
    File,
    Browser,
}

/// The complete normalized conversation handed to the engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conversation {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub model: String,
    pub turns: Vec<Turn>,
    pub context: ContextState,
    pub source: Source,
}

impl Conversation {
    /// The most recent assistant turn, if any. Most signals evaluate against
    /// the latest assistant output (the "delta").
    pub fn last_assistant(&self) -> Option<&Turn> {
        self.turns.iter().rev().find(|t| t.role == Role::Assistant)
    }

    /// All assistant turns in order.
    pub fn assistant_turns(&self) -> impl Iterator<Item = &Turn> {
        self.turns.iter().filter(|t| t.role == Role::Assistant)
    }

    /// Fraction of the context window consumed, clamped to `[0.0, 1.0]`.
    pub fn saturation_ratio(&self) -> f32 {
        if self.context.window_size == 0 {
            return 0.0;
        }
        (self.context.used_tokens as f32 / self.context.window_size as f32).clamp(0.0, 1.0)
    }
}
