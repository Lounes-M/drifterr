//! Signal 5 — degradation symptoms (soft).
//!
//! Cheap text statistics over the assistant's recent replies that *correlate*
//! with context rot:
//!
//! * **looping** — near-duplicate consecutive replies (the model going in
//!   circles), detected via embedding cosine similarity;
//! * **verbosity** — a reply that balloons far beyond the running average;
//! * **hedging** — a spike in non-committal filler ("it depends", "I think", …).
//!
//! Soft by nature: it never drives RED, only AMBER as support. Thresholds are
//! conservative — a single clear symptom fires; noise does not.

use crate::conversation::Conversation;
use crate::signals::{Evidence, SignalEvent, SignalKind, State};
use drifterr_embeddings::{cosine, tokenize, Embedder};

const MIN_TURNS: usize = 3;
/// Cosine similarity between two recent replies above this ⇒ looping.
const LOOP_SIM: f32 = 0.92;
/// Last reply longer than this multiple of the running mean ⇒ verbose…
const VERBOSE_RATIO: f32 = 2.5;
/// …but only once it's also substantial in absolute terms (words).
const VERBOSE_MIN_WORDS: usize = 80;
/// Hedging phrase hits in the last reply at/above this ⇒ hedging.
const HEDGE_HITS: usize = 3;

const HEDGES: &[&str] = &[
    // English
    "i'm not sure",
    "i am not sure",
    "it depends",
    "as an ai",
    "i cannot be certain",
    "hard to say",
    "it seems",
    "i think",
    "perhaps",
    "maybe",
    "might be",
    "possibly",
    "in general",
    "to some extent",
    "it's worth noting",
    "i'm not entirely sure",
    "it's difficult to say",
    "generally speaking",
    // French (the product's second audience)
    "je ne suis pas sûr",
    "je ne suis pas certain",
    "ça dépend",
    "cela dépend",
    "il semble",
    "il semblerait",
    "peut-être",
    "je pense",
    "il se peut",
    "en général",
    "dans une certaine mesure",
    "difficile à dire",
    "il est possible que",
];

/// Evaluate Signal 5. Returns `Some(AMBER event)` naming the symptom(s), or
/// `None` when the replies look healthy.
pub fn evaluate(conv: &Conversation, embedder: &dyn Embedder) -> Option<SignalEvent> {
    let turns: Vec<&crate::conversation::Turn> = conv.assistant_turns().collect();
    if turns.len() < MIN_TURNS {
        return None;
    }
    let last = *turns.last().unwrap();
    let mut symptoms: Vec<String> = Vec::new();

    // Looping: max similarity among the last three replies.
    let recent = &turns[turns.len() - 3..];
    let mut max_sim = 0.0f32;
    for i in 0..recent.len() {
        for j in (i + 1)..recent.len() {
            let s = cosine(
                &embedder.embed(&recent[i].content),
                &embedder.embed(&recent[j].content),
            );
            if s > max_sim {
                max_sim = s;
            }
        }
    }
    if max_sim >= LOOP_SIM {
        symptoms.push("repeating itself".to_string());
    }

    // Verbosity: last reply vs the running mean of the earlier ones.
    let words: Vec<usize> = turns.iter().map(|t| word_count(&t.content)).collect();
    let prev_mean = mean_usize(&words[..words.len() - 1]);
    let last_words = *words.last().unwrap();
    if prev_mean > 0.0
        && last_words as f32 > prev_mean * VERBOSE_RATIO
        && last_words >= VERBOSE_MIN_WORDS
    {
        symptoms.push(format!("reply ballooning ({last_words} words)"));
    }

    // Hedging density in the last reply.
    let lc = last.content.to_ascii_lowercase();
    let hedges = HEDGES.iter().filter(|h| lc.contains(*h)).count();
    if hedges >= HEDGE_HITS {
        symptoms.push("heavy hedging".to_string());
    }

    if symptoms.is_empty() {
        return None;
    }
    Some(SignalEvent::new(
        SignalKind::Degradation,
        State::Amber,
        Evidence {
            detail: format!("degradation symptoms: {}", symptoms.join(", ")),
            turn_index: Some(last.index),
            constraint_id: None,
            span: None,
        },
    ))
}

fn word_count(s: &str) -> usize {
    tokenize(s).count()
}
fn mean_usize(xs: &[usize]) -> f32 {
    if xs.is_empty() {
        return 0.0;
    }
    xs.iter().sum::<usize>() as f32 / xs.len() as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::{ContextState, Role, Source, Turn};
    use drifterr_embeddings::BagEmbedder;

    fn conv(contents: &[&str]) -> Conversation {
        let turns = contents
            .iter()
            .enumerate()
            .map(|(i, s)| Turn {
                index: i,
                role: Role::Assistant,
                content: s.to_string(),
                tokens: 0,
                timestamp: 0,
            })
            .collect();
        Conversation {
            session_id: "s".into(),
            model: "m".into(),
            turns,
            context: ContextState {
                window_size: 1000,
                used_tokens: 10,
                exact: true,
                tool_call_count: 0,
            },
            source: Source::Proxy,
        }
    }

    #[test]
    fn detects_looping() {
        let c = conv(&[
            "let me try adjusting the configuration now",
            "let me try adjusting the configuration now",
            "let me try adjusting the configuration now",
        ]);
        let e = evaluate(&c, &BagEmbedder::default()).expect("loop");
        assert_eq!(e.state, State::Amber);
        assert!(e.evidence.detail.contains("repeating"));
    }

    #[test]
    fn detects_hedging() {
        let c = conv(&[
            "the function returns the user id",
            "we add the validation layer next",
            "I think it depends, and it seems this might be the cause, perhaps",
        ]);
        let e = evaluate(&c, &BagEmbedder::default()).expect("hedge");
        assert!(e.evidence.detail.contains("hedging"));
    }

    #[test]
    fn detects_hedging_french() {
        let c = conv(&[
            "la fonction renvoie l'identifiant",
            "on ajoute la couche de validation",
            "je pense que ça dépend, il semble que ce soit possible, peut-être",
        ]);
        let e = evaluate(&c, &BagEmbedder::default()).expect("hedge FR");
        assert!(e.evidence.detail.contains("hedging"));
    }

    #[test]
    fn quiet_on_healthy_varied_replies() {
        let c = conv(&[
            "added the login route and its handler",
            "wrote unit tests for the token parser",
            "documented the session expiry behavior",
        ]);
        assert!(evaluate(&c, &BagEmbedder::default()).is_none());
    }

    #[test]
    fn quiet_when_too_few_turns() {
        let c = conv(&["same", "same"]);
        assert!(evaluate(&c, &BagEmbedder::default()).is_none());
    }
}
