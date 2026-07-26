//! Signal 4 — context saturation.
//!
//! Free and, via the proxy, exact. This is the *leading* indicator: it warns
//! before drift becomes visible in the output. It blends three things:
//!
//! * occupancy — `usedTokens / windowSize`
//! * fill rate — how fast the window is filling per turn (a steep slope means
//!   we will hit the wall soon even if occupancy is moderate now)
//! * tool volume — heavy tool output is a known accelerant of rot
//!
//! Thresholds follow the brief: ≥0.80 (or a high fill rate) ⇒ RED, ≥0.55 ⇒
//! AMBER, else GREEN.

use crate::conversation::Conversation;
use crate::signals::{Evidence, SignalEvent, SignalKind, State};

/// Occupancy at or above this fraction is critical.
pub const RED_RATIO: f32 = 0.80;
/// Occupancy at or above this fraction is a warning.
pub const AMBER_RATIO: f32 = 0.55;
/// If the window is filling faster than this fraction *per turn* (averaged over
/// assistant turns), we escalate to RED regardless of current occupancy — the
/// wall is close.
pub const RED_FILL_RATE: f32 = 0.10;

/// Evaluate Signal 4 for the conversation. Always returns exactly one event
/// (saturation is always meaningful), so callers can read current occupancy
/// even in the GREEN case.
pub fn evaluate(conv: &Conversation) -> SignalEvent {
    let ratio = conv.saturation_ratio();
    let fill_rate = fill_rate(conv);
    // Occupancy we cannot vouch for must not drive a red. See
    // `ContextState::occupancy_known`: on a channel whose transcript outlives a context
    // compaction, the token sum is a lower bound on what was ever said, not a measure of
    // what the window currently holds. A hard signal firing on that would be crying
    // wolf, which `CLAUDE.md` treats as the one unforgivable failure — so it caps at
    // AMBER and says why.
    let known = conv.context.occupancy_known;

    let state = if known && (ratio >= RED_RATIO || fill_rate >= RED_FILL_RATE) {
        State::Red
    } else if ratio >= AMBER_RATIO {
        State::Amber
    } else {
        State::Green
    };

    let pct = (ratio * 100.0).round() as u32;
    let precision = if conv.context.exact {
        "exact"
    } else {
        "estimated"
    };
    let mut detail = if known {
        format!(
            "context {pct}% full ({precision}: {}/{} tokens)",
            conv.context.used_tokens, conv.context.window_size
        )
    } else {
        format!(
            "context at least {pct}% full ({}/{} tokens seen); true occupancy unknown — \
             this transcript spans a context compaction",
            conv.context.used_tokens, conv.context.window_size
        )
    };
    // Fill rate is derived from the same unreliable sum, so it is only worth mentioning
    // when occupancy means something.
    if known && fill_rate >= RED_FILL_RATE {
        detail.push_str(&format!(", filling fast ({:.0}%/turn)", fill_rate * 100.0));
    }
    if conv.context.tool_call_count > 0 {
        detail.push_str(&format!(", {} tool calls", conv.context.tool_call_count));
    }

    SignalEvent::new(
        SignalKind::Saturation,
        state,
        Evidence {
            detail,
            turn_index: conv.turns.last().map(|t| t.index),
            constraint_id: None,
            span: None,
        },
    )
}

/// Average fraction of the window consumed per assistant turn so far.
///
/// Approximated as current occupancy spread over the number of assistant
/// turns — a cheap proxy for slope that needs no history. With zero assistant
/// turns the rate is zero (nothing has been generated yet).
fn fill_rate(conv: &Conversation) -> f32 {
    let assistant_turns = conv.assistant_turns().count();
    if assistant_turns == 0 {
        return 0.0;
    }
    conv.saturation_ratio() / assistant_turns as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::{ContextState, Role, Source, Turn};

    fn conv(used: usize, window: usize, assistant_turns: usize, tools: usize) -> Conversation {
        let mut turns = Vec::new();
        for i in 0..assistant_turns {
            turns.push(Turn {
                index: i,
                role: Role::Assistant,
                content: "x".into(),
                tokens: 0,
                timestamp: 0,
            });
        }
        Conversation {
            session_id: "s".into(),
            model: "claude-opus-4-x".into(),
            turns,
            context: ContextState {
                window_size: window,
                used_tokens: used,
                exact: true,
                occupancy_known: true,
                tool_call_count: tools,
            },
            source: Source::Proxy,
        }
    }

    #[test]
    fn green_below_amber() {
        // 30% over 10 turns: low occupancy, low fill rate.
        assert_eq!(evaluate(&conv(3000, 10000, 10, 0)).state, State::Green);
    }

    #[test]
    fn amber_band() {
        // 60% over 10 turns: fill rate 6%/turn (under RED), occupancy in band.
        assert_eq!(evaluate(&conv(6000, 10000, 10, 0)).state, State::Amber);
    }

    #[test]
    fn red_on_occupancy() {
        assert_eq!(evaluate(&conv(8500, 10000, 20, 0)).state, State::Red);
    }

    #[test]
    fn red_on_fill_rate_even_if_low_occupancy() {
        // 40% reached in only 2 turns ⇒ 20%/turn ⇒ RED despite <55% occupancy.
        assert_eq!(evaluate(&conv(4000, 10000, 2, 0)).state, State::Red);
    }

    /// The guardrail: occupancy we cannot vouch for must never drive RED.
    ///
    /// On a channel whose transcript outlives a context compaction, the token sum is a
    /// lower bound on what was ever said rather than a measure of what the window holds.
    /// A hard signal firing on that is crying wolf — and it would fire on *every* long
    /// session, not occasionally.
    #[test]
    fn unknown_occupancy_never_drives_red() {
        let unknown = |used, window, turns| Conversation {
            context: ContextState {
                occupancy_known: false,
                exact: false,
                ..conv(used, window, turns, 0).context
            },
            ..conv(used, window, turns, 0)
        };

        // A full-looking window: RED when known, capped at AMBER when not.
        assert_eq!(evaluate(&conv(9_900, 10_000, 20, 0)).state, State::Red);
        let ev = evaluate(&unknown(9_900, 10_000, 20));
        assert_eq!(ev.state, State::Amber, "must abstain from RED");
        assert!(
            ev.evidence.detail.contains("at least") && ev.evidence.detail.contains("unknown"),
            "must explain that this is a lower bound: {}",
            ev.evidence.detail
        );
        assert!(
            !ev.evidence.detail.contains("filling fast"),
            "fill rate comes from the same unreliable sum and must not be quoted: {}",
            ev.evidence.detail
        );

        // The fill-rate path is gated too: 40% in 2 turns is RED when known.
        assert_eq!(evaluate(&conv(4_000, 10_000, 2, 0)).state, State::Red);
        assert_ne!(evaluate(&unknown(4_000, 10_000, 2)).state, State::Red);

        // And a genuinely low reading still reads green — abstention is not alarm.
        assert_eq!(evaluate(&unknown(1_000, 10_000, 10)).state, State::Green);
    }

    #[test]
    fn detail_reports_precision() {
        let estimated = Conversation {
            context: ContextState {
                exact: false,
                ..conv(3000, 10000, 10, 0).context
            },
            ..conv(3000, 10000, 10, 0)
        };
        assert!(evaluate(&estimated).evidence.detail.contains("estimated"));
        assert!(evaluate(&conv(3000, 10000, 10, 0))
            .evidence
            .detail
            .contains("exact"));
    }
}
