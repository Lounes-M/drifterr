//! End-to-end smoke demo wiring the M0 + M1 pieces together:
//! build a baseline + conversation, run the engine, persist & reload, print the
//! verdict. Run with: `cargo run -p drifterr-store --example demo`.

use drifterr_engine::baseline::{Baseline, Checkable, Constraint, ConstraintType};
use drifterr_engine::conversation::{ContextState, Conversation, Role, Source, Turn};
use drifterr_engine::evaluate;
use drifterr_engine::state_machine::SessionMonitor;
use drifterr_store::Store;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let baseline = Baseline {
        goal: "Refactor the auth module in strict TypeScript".into(),
        constraints: vec![Constraint {
            id: "c1".into(),
            text: "TypeScript only, no JS".into(),
            kind: ConstraintType::Tech,
            checkable: Checkable::Deterministic,
            active: true,
            rule: None, // inferred from the text
        }],
        decisions: vec![],
    };

    let conv = Conversation {
        session_id: "demo".into(),
        model: "claude-opus-4-x".into(),
        turns: vec![
            Turn {
                index: 0,
                role: Role::User,
                content: "Refactor auth in strict TS.".into(),
                tokens: 10,
                timestamp: 1,
            },
            Turn {
                index: 1,
                role: Role::Assistant,
                content: "Let's create auth.js first.".into(),
                tokens: 9,
                timestamp: 2,
            },
        ],
        context: ContextState {
            window_size: 200_000,
            used_tokens: 12_000,
            exact: true,
            tool_call_count: 0,
        },
        source: Source::Proxy,
    };

    // M0: persist and reload, losslessly.
    let mut store = Store::open_in_memory()?;
    store.save_conversation(&conv)?;
    store.save_baseline(&conv.session_id, &baseline)?;
    let conv = store.load_conversation("demo")?;
    let baseline = store.load_baseline("demo")?;

    // M1: detect.
    let verdict = evaluate(&conv, &baseline);
    let mut monitor = SessionMonitor::default();
    let state = monitor.observe(&verdict);

    store.record_events(&conv.session_id, &verdict.events)?;
    store.set_status(&conv.session_id, state)?;

    println!("committed state: {state:?}");
    if let Some(t) = verdict.triggering() {
        println!("triggered by {:?}: {}", t.signal, t.evidence.detail);
        if let Some(span) = &t.evidence.span {
            println!("offending span: {span:?}");
        }
    }
    Ok(())
}
