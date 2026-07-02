//! Detection-quality eval harness.
//!
//! `fixtures.rs` is a go/no-go gate — it *panics* on any mismatch, so it can
//! only ever hold a set the engine already gets 100% right. This harness does
//! the opposite: it runs the engine over an annotated set, **tolerates** wrong
//! predictions, and *measures* them — a state confusion matrix plus per-signal
//! precision / recall / F1 on the triggering signal. That turns "well built"
//! into "measured": drop real, messy sessions into a directory and get numbers.
//!
//! Run:
//!   cargo run -p drifterr-engine --example eval               # scans fixtures/
//!   cargo run -p drifterr-engine --example eval -- eval/       # a bigger set
//!
//! Scope: this measures the pure-engine signals (constraint, saturation, goal
//! alignment, degradation). The judge signals (decision coherence, fuzzy
//! constraints) run in the proxy against a live model and are evaluated
//! separately — a fixture that expects one of those is reported as `skipped`.
//!
//! Fixture shape (same as fixtures/): { name, baseline, conversation,
//! expect: { state, triggeringSignal?, triggeringConstraint? } }.

use drifterr_engine::baseline::Baseline;
use drifterr_engine::conversation::Conversation;
use drifterr_engine::signals::SignalKind;
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

#[derive(Deserialize)]
struct Expect {
    state: String,
    #[serde(rename = "triggeringSignal")]
    triggering_signal: Option<String>,
}

#[derive(Deserialize)]
struct Fixture {
    name: String,
    baseline: Baseline,
    conversation: Conversation,
    expect: Expect,
}

/// One evaluated case: what was expected vs what the engine predicted.
struct Row {
    name: String,
    exp_state: String,
    pred_state: String,
    exp_sig: String,
    pred_sig: String,
}

/// Signals the engine's pure `evaluate` can actually emit. Anything else in an
/// `expect` is a judge signal → out of scope here (reported as skipped).
const ENGINE_SIGNALS: &[&str] = &["constraint", "saturation", "goal_alignment", "degradation"];
const STATES: &[&str] = &["green", "amber", "red"];

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
    let dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("..")
                .join("fixtures")
        });

    let mut entries: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
        .collect();
    entries.sort();

    let mut rows: Vec<Row> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut errors = 0usize;

    for path in &entries {
        let fx: Fixture = match fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
        {
            Some(f) => f,
            None => {
                eprintln!("skip (unreadable/unparseable): {}", path.display());
                errors += 1;
                continue;
            }
        };

        let exp_sig = fx
            .expect
            .triggering_signal
            .as_deref()
            .unwrap_or("none")
            .to_string();
        // A judge-only expected cause can't be judged by the pure engine.
        if exp_sig != "none" && !ENGINE_SIGNALS.contains(&exp_sig.as_str()) {
            skipped.push(format!("{} (judge signal: {exp_sig})", fx.name));
            continue;
        }

        let verdict = drifterr_engine::evaluate(&fx.conversation, &fx.baseline);
        rows.push(Row {
            name: fx.name,
            exp_state: fx.expect.state,
            pred_state: format!("{:?}", verdict.state).to_lowercase(),
            exp_sig,
            pred_sig: verdict
                .triggering()
                .map(|e| signal_str(e.signal))
                .unwrap_or("none")
                .to_string(),
        });
    }

    println!("\n{}", "=".repeat(66));
    println!(
        "Detection eval — {}  ({} cases, {} skipped, {} errors)",
        dir.display(),
        rows.len(),
        skipped.len(),
        errors
    );
    println!("{}", "=".repeat(66));

    print_cases(&rows);
    print_state_matrix(&rows);
    print_signal_metrics(&rows);

    if !skipped.is_empty() {
        println!("\nSkipped (judge signals — evaluated separately):");
        for s in &skipped {
            println!("  • {s}");
        }
    }
    println!();
}

fn print_cases(rows: &[Row]) {
    println!("\nPer-case (expected → predicted):");
    for r in rows {
        let ok = r.exp_state == r.pred_state && r.exp_sig == r.pred_sig;
        let short: String = r.name.chars().take(44).collect();
        println!(
            "  {} {short:<44}  {:>5}/{:<14} → {:>5}/{}",
            if ok { "✓" } else { "✗" },
            r.exp_state,
            r.exp_sig,
            r.pred_state,
            r.pred_sig
        );
    }
}

/// State confusion matrix (rows = expected, cols = predicted) + accuracy.
fn print_state_matrix(rows: &[Row]) {
    if rows.is_empty() {
        return;
    }
    println!("\nState confusion (rows = expected, cols = predicted):");
    print!("           ");
    for c in STATES {
        print!("{c:>8}");
    }
    println!("    total");

    let mut correct = 0usize;
    for exp in STATES {
        print!("  {exp:>7}  ");
        let mut row_total = 0usize;
        for pred in STATES {
            let n = rows
                .iter()
                .filter(|r| r.exp_state == *exp && r.pred_state == *pred)
                .count();
            if exp == pred {
                correct += n;
            }
            row_total += n;
            print!("{n:>8}");
        }
        println!("    {row_total:>5}");
    }
    println!(
        "\n  State accuracy: {}/{} = {:.1}%",
        correct,
        rows.len(),
        100.0 * correct as f64 / rows.len() as f64
    );
}

/// Per-signal precision / recall / F1 on the triggering label (+ "none").
fn print_signal_metrics(rows: &[Row]) {
    if rows.is_empty() {
        return;
    }
    let mut classes: Vec<&str> = ENGINE_SIGNALS.to_vec();
    classes.push("none");

    println!("\nTriggering-signal precision / recall / F1:");
    println!(
        "  {:<16}{:>9}{:>9}{:>9}{:>7}",
        "signal", "prec", "recall", "f1", "n"
    );

    let mut macro_f1 = 0.0;
    let mut counted = 0usize;

    for cls in &classes {
        let tp = rows
            .iter()
            .filter(|r| r.exp_sig == *cls && r.pred_sig == *cls)
            .count();
        let fp = rows
            .iter()
            .filter(|r| r.exp_sig != *cls && r.pred_sig == *cls)
            .count();
        let fn_ = rows
            .iter()
            .filter(|r| r.exp_sig == *cls && r.pred_sig != *cls)
            .count();
        let support = rows.iter().filter(|r| r.exp_sig == *cls).count();
        if support == 0 && tp + fp == 0 {
            continue; // class not present in this set at all
        }
        let p = if tp + fp == 0 {
            0.0
        } else {
            tp as f64 / (tp + fp) as f64
        };
        let r = if tp + fn_ == 0 {
            0.0
        } else {
            tp as f64 / (tp + fn_) as f64
        };
        let f1 = if p + r == 0.0 {
            0.0
        } else {
            2.0 * p * r / (p + r)
        };
        if support > 0 {
            macro_f1 += f1;
            counted += 1;
        }
        println!("  {cls:<16}{p:>9.2}{r:>9.2}{f1:>9.2}{support:>7}");
    }
    if counted > 0 {
        println!(
            "\n  Macro-F1 (over {counted} present classes): {:.2}",
            macro_f1 / counted as f64
        );
    }
}
