//! M1 go/no-go: run the engine against every hand-annotated fixture and assert
//! the expected state and triggering signal. Adding a fixture file adds a case.

use drifterr_engine::baseline::Baseline;
use drifterr_engine::conversation::Conversation;
use drifterr_engine::signals::{SignalKind, State};
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

#[derive(Deserialize)]
struct Expect {
    state: String,
    #[serde(rename = "triggeringSignal")]
    triggering_signal: Option<String>,
    #[serde(rename = "triggeringConstraint")]
    triggering_constraint: Option<String>,
}

#[derive(Deserialize)]
struct Fixture {
    name: String,
    baseline: Baseline,
    conversation: Conversation,
    expect: Expect,
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
}

fn parse_state(s: &str) -> State {
    match s {
        "red" => State::Red,
        "amber" => State::Amber,
        "green" => State::Green,
        other => panic!("unknown state {other}"),
    }
}

fn signal_str(k: SignalKind) -> &'static str {
    match k {
        SignalKind::Constraint => "constraint",
        SignalKind::Saturation => "saturation",
        SignalKind::GoalAlignment => "goal_alignment",
        SignalKind::DecisionCoherence => "decision_coherence",
        SignalKind::Degradation => "degradation",
    }
}

#[test]
fn all_fixtures_match_expectations() {
    let dir = fixtures_dir();
    let mut ran = 0;

    for entry in fs::read_dir(&dir).expect("read fixtures dir") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let raw = fs::read_to_string(&path).unwrap();
        let fx: Fixture = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));

        let verdict = drifterr_engine::evaluate(&fx.conversation, &fx.baseline);

        let expected_state = parse_state(&fx.expect.state);
        assert_eq!(
            verdict.state, expected_state,
            "[{}] state mismatch: got {:?}, events: {:?}",
            fx.name, verdict.state, verdict.events
        );

        match &fx.expect.triggering_signal {
            None => assert!(
                verdict.triggering().is_none(),
                "[{}] expected no trigger, got {:?}",
                fx.name,
                verdict.triggering()
            ),
            Some(sig) => {
                let trig = verdict
                    .triggering()
                    .unwrap_or_else(|| panic!("[{}] expected a trigger, got none", fx.name));
                assert_eq!(signal_str(trig.signal), sig, "[{}] wrong signal", fx.name);
                if let Some(cid) = &fx.expect.triggering_constraint {
                    assert_eq!(
                        trig.evidence.constraint_id.as_deref(),
                        Some(cid.as_str()),
                        "[{}] wrong constraint id",
                        fx.name
                    );
                }
            }
        }
        ran += 1;
    }

    assert!(ran >= 6, "expected at least 6 fixtures, ran {ran}");
}
