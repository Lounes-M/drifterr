//! Turn a real Claude Code session into an eval-case skeleton.
//!
//! Annotating from scratch is tedious; this makes it **review-and-fix**. Point it
//! at a Claude Code `.jsonl` and it:
//!   1. parses the conversation (the normalized shape the engine sees),
//!   2. auto-extracts the baseline (goal + constraints) from your own messages,
//!   3. runs the engine and **pre-fills** `expect` with the current prediction,
//!   4. prints a ready-to-annotate eval case to stdout.
//!
//! You then correct `expect.state` / `triggeringSignal` / `turn` to the GROUND
//! TRUTH (what SHOULD have happened) and delete the `_review` marker. Drop the
//! file in `eval/` (dev) or `eval/blind/` (holdout). See `eval/SCHEMA.md`.
//!
//! Run:
//!   cargo run -p drifterr-adapters --example annotate -- ~/.claude/projects/x/<id>.jsonl \
//!     > eval/my-case.json
//!
//! Nothing leaves the machine — it only reads the file you point it at.

use drifterr_engine::baseline::Baseline;
use drifterr_engine::signals::SignalKind;
use serde_json::json;
use std::path::Path;

fn signal_str(k: SignalKind) -> &'static str {
    match k {
        SignalKind::Constraint => "constraint",
        SignalKind::Saturation => "saturation",
        SignalKind::GoalAlignment => "goal_alignment",
        SignalKind::DecisionCoherence => "decision_coherence",
        SignalKind::Degradation => "degradation",
    }
}

fn main() {
    let path = match std::env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!(
                "usage: cargo run -p drifterr-adapters --example annotate -- <session.jsonl>"
            );
            std::process::exit(2);
        }
    };

    let content =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"));
    let stem = Path::new(&path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("session")
        .to_string();

    let conv = match drifterr_adapters::claude_code::parse_session(&content, &stem) {
        Some(c) => c,
        None => {
            eprintln!("no usable turns in {path} — is it a Claude Code session .jsonl?");
            std::process::exit(1);
        }
    };

    // Auto-extract the baseline from the user's own messages, then predict.
    let baseline = Baseline::extract(&conv.turns);
    let verdict = drifterr_engine::evaluate(&conv, &baseline);
    let trigger = verdict.triggering();

    let mut expect = serde_json::Map::new();
    expect.insert(
        "state".into(),
        json!(format!("{:?}", verdict.state).to_lowercase()),
    );
    if let Some(sig) = trigger.map(|e| signal_str(e.signal)) {
        expect.insert("triggeringSignal".into(), json!(sig));
    }
    if let Some(cid) = trigger.and_then(|e| e.evidence.constraint_id.clone()) {
        expect.insert("triggeringConstraint".into(), json!(cid));
    }
    if let Some(turn) = trigger.and_then(|e| e.evidence.turn_index) {
        expect.insert("turn".into(), json!(turn));
    }
    expect.insert(
        "_review".into(),
        json!(
            "PREFILLED FROM THE ENGINE'S PREDICTION — not ground truth. Correct \
             state / triggeringSignal / turn to what SHOULD have happened, then \
             delete this key. See eval/SCHEMA.md."
        ),
    );

    let case = json!({
        "name": format!("{stem} (REVIEW)"),
        "baseline": baseline,
        "conversation": conv,
        "expect": serde_json::Value::Object(expect),
    });

    println!("{}", serde_json::to_string_pretty(&case).unwrap());

    // Guidance to stderr so stdout stays a clean, redirectable JSON case.
    eprintln!("─────────────────────────────────────────────────────────────");
    eprintln!(
        "Pre-filled eval case written to stdout ({} turns).",
        conv.turns.len()
    );
    eprintln!("Now, in the JSON: set expect.state / triggeringSignal / turn to the");
    eprintln!("GROUND TRUTH, delete the `_review` key, then save under eval/ or");
    eprintln!("eval/blind/. Re-run the harness to measure:");
    eprintln!("  cargo run -p drifterr-engine --example eval -- eval/");
    eprintln!("─────────────────────────────────────────────────────────────");
}
