//! Signal 3 — decision coherence (judge-backed).
//!
//! Watches for the assistant reintroducing or contradicting a decision the user
//! explicitly **rejected**. Retrieval is local (embedding cosine over rejected
//! decisions); the final call is the judge's, on the single best candidate, so
//! we make at most one model call per turn — and only when something looks close.
//!
//! Emits **AMBER** (support only — never RED on its own, like the other
//! non-hard signals).

use crate::Judge;
use drifterr_embeddings::{cosine, Embedder};
use drifterr_engine::baseline::Decision;
use drifterr_engine::conversation::Turn;
use drifterr_engine::signals::{Evidence, SignalEvent, SignalKind, State};

/// A rejected decision must be at least this similar to the reply before we
/// spend a judge call on it.
const CANDIDATE_SIM: f32 = 0.18;

/// Evaluate Signal 3 for the latest assistant turn. `None` when the judge is
/// disabled, there are no rejected decisions, nothing is close enough, or the
/// judge says it's fine.
pub async fn decision_coherence(
    last_assistant: &Turn,
    decisions: &[Decision],
    embedder: &dyn Embedder,
    judge: &Judge,
) -> Option<SignalEvent> {
    if !judge.enabled() {
        return None;
    }
    let rejected: Vec<&Decision> = decisions.iter().filter(|d| d.rejected).collect();
    if rejected.is_empty() {
        return None;
    }

    // Rank rejected decisions by similarity to the reply; take the best.
    let reply_vec = embedder.embed(&last_assistant.content);
    let best = rejected
        .iter()
        .map(|d| (d, cosine(&reply_vec, &embedder.embed(&d.text))))
        .max_by(|a, b| a.1.total_cmp(&b.1))?;
    if best.1 < CANDIDATE_SIM {
        return None;
    }
    let decision = best.0;

    let question = format!(
        "A previously REJECTED decision was: \"{}\". Does the assistant message \
         below reintroduce, re-propose, or rely on that rejected decision? \
         Answer yes only if it clearly does.",
        decision.text
    );
    let answer = judge.check(&question, &last_assistant.content).await;
    if !answer.yes {
        return None;
    }

    Some(SignalEvent::new(
        SignalKind::DecisionCoherence,
        State::Amber,
        Evidence {
            detail: format!(
                "reintroduces a rejected decision (\"{}\"): {}",
                decision.text, answer.reason
            ),
            turn_index: Some(last_assistant.index),
            constraint_id: None,
            span: None,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StubJudge;
    use drifterr_embeddings::BagEmbedder;
    use drifterr_engine::conversation::Role;

    fn turn(content: &str) -> Turn {
        Turn {
            index: 7,
            role: Role::Assistant,
            content: content.to_string(),
            tokens: 0,
            timestamp: 0,
        }
    }
    fn rejected(text: &str) -> Decision {
        Decision {
            text: text.to_string(),
            rejected: true,
        }
    }

    #[tokio::test]
    async fn fires_when_rejected_decision_reintroduced() {
        let decisions = vec![
            rejected("use bcrypt for password hashing"),
            Decision {
                text: "use postgres".into(),
                rejected: false,
            },
        ];
        let judge = Judge::Stub(StubJudge::new(&["bcrypt"]));
        let e = decision_coherence(
            &turn("Let's hash the passwords with bcrypt and a salt."),
            &decisions,
            &BagEmbedder::default(),
            &judge,
        )
        .await
        .expect("should fire");
        assert_eq!(e.state, State::Amber);
        assert_eq!(e.signal, SignalKind::DecisionCoherence);
        assert!(e.evidence.detail.contains("bcrypt"));
    }

    #[tokio::test]
    async fn quiet_when_judge_says_no() {
        let decisions = vec![rejected("use bcrypt for password hashing")];
        let judge = Judge::Stub(StubJudge::new(&["something-else"]));
        let r = decision_coherence(
            &turn("Let's hash the passwords with bcrypt."),
            &decisions,
            &BagEmbedder::default(),
            &judge,
        )
        .await;
        assert!(r.is_none());
    }

    #[tokio::test]
    async fn quiet_when_disabled_or_no_rejected() {
        let decisions = vec![rejected("use bcrypt")];
        assert!(decision_coherence(
            &turn("bcrypt"),
            &decisions,
            &BagEmbedder::default(),
            &Judge::Disabled
        )
        .await
        .is_none());
        let judge = Judge::Stub(StubJudge::new(&["bcrypt"]));
        assert!(
            decision_coherence(&turn("bcrypt"), &[], &BagEmbedder::default(), &judge)
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn quiet_when_nothing_similar() {
        // Rejected decision unrelated to the reply ⇒ no judge call, no fire.
        let decisions = vec![rejected("deploy to kubernetes with helm charts")];
        let judge = Judge::Stub(StubJudge::new(&["anything"]));
        let r = decision_coherence(
            &turn("the quick brown fox jumps over the lazy dog"),
            &decisions,
            &BagEmbedder::default(),
            &judge,
        )
        .await;
        assert!(r.is_none());
    }
}
