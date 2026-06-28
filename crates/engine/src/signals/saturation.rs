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

    let state = if ratio >= RED_RATIO || fill_rate >= RED_FILL_RATE {
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
    let mut detail = format!(
        "context {pct}% full ({precision}: {}/{} tokens)",
        conv.context.used_tokens, conv.context.window_size
    );
    if fill_rate >= RED_FILL_RATE {
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
