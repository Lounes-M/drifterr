//! Detection-quality eval harness.
//!
//! `fixtures.rs` is a go/no-go gate — it *panics* on any mismatch, so it can
//! only ever hold a set the engine already gets 100% right. This harness does
//! the opposite: it runs the engine over an annotated set, **tolerates** wrong
//! predictions, and *measures* them. It reports, in order:
//!
//!   * a per-case table (expected → predicted),
//!   * a state confusion matrix + accuracy,
//!   * per-signal precision / recall / F1 on the triggering signal,
//!   * the **hard-signal false-positive rate** (the release go/no-go: a hard
//!     signal — constraint or saturation — that fires when it shouldn't, or a
//!     RED that shouldn't be RED, destroys trust and must be zero),
//!   * the **alert delay** (turns between the annotated drift turn and the turn
//!     the engine actually names — too late is useless, too early is noise),
//!   * an **uplift vs a naive baseline** (saturation-only): if the full engine
//!     doesn't beat "just watch the context fill up", that's an alarm.
//!
//! Run:
//!   cargo run -p drifterr-engine --example eval                 # scans fixtures/
//!   cargo run -p drifterr-engine --example eval -- eval/        # a bigger set
//!   cargo run -p drifterr-engine --example eval -- eval/ --gate # CI gate mode
//!
//! `--gate` makes the harness a real release gate: it exits non-zero if any
//! hard-signal false positive (or false-RED) is present. Wire it into CI to
//! enforce "zero hard false positives" on whatever set you point it at — ideally
//! the blind holdout in `eval/blind/` (see eval/SCHEMA.md).
//!
//! Scope: this measures the pure-engine signals (constraint, saturation, goal
//! alignment, degradation). The judge signals (decision coherence, fuzzy
//! constraints) run in the proxy against a live model and are evaluated
//! separately — a fixture that expects one of those is reported as `skipped`.
//!
//! Fixture shape (see eval/SCHEMA.md): { name, baseline, conversation,
//! expect: { state, triggeringSignal?, triggeringConstraint?, turn? } }.

use drifterr_engine::baseline::Baseline;
use drifterr_engine::conversation::Conversation;
use drifterr_engine::signals::{SignalKind, State};
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Deserialize)]
struct Expect {
    state: String,
    #[serde(rename = "triggeringSignal")]
    triggering_signal: Option<String>,
    /// The turn (0-based) where the drift the engine should catch begins. Optional
    /// — only cases that carry it contribute to the alert-delay metric.
    turn: Option<usize>,
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
    exp_turn: Option<usize>,
    pred_turn: Option<usize>,
    /// A naive saturation-only prediction (the baseline we must beat).
    naive_state: String,
}

const STATES: [&str; 3] = ["green", "amber", "red"];
const ENGINE_SIGNALS: [&str; 4] = ["constraint", "saturation", "goal_alignment", "degradation"];
/// Signals that may legitimately drive RED. A wrong prediction here is the
/// trust-killer the release gate protects against.
const HARD_SIGNALS: [&str; 2] = ["constraint", "saturation"];

fn signal_str(k: SignalKind) -> &'static str {
    match k {
        SignalKind::Constraint => "constraint",
        SignalKind::Saturation => "saturation",
        SignalKind::GoalAlignment => "goal_alignment",
        SignalKind::DecisionCoherence => "decision_coherence",
        SignalKind::Degradation => "degradation",
    }
}

fn state_str(s: State) -> &'static str {
    match s {
        State::Green => "green",
        State::Amber => "amber",
        State::Red => "red",
    }
}

fn main() -> ExitCode {
    // Args: an optional directory (first non-flag) and an optional `--gate`.
    let mut dir: Option<PathBuf> = None;
    let mut gate = false;
    for arg in std::env::args().skip(1) {
        if arg == "--gate" {
            gate = true;
        } else if !arg.starts_with("--") {
            dir = Some(PathBuf::from(arg));
        }
    }
    let dir = dir.unwrap_or_else(|| {
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
        let trigger = verdict.triggering();
        // Naive baseline: the state you'd report if you only watched saturation.
        let naive_state = verdict
            .events
            .iter()
            .find(|e| e.signal == SignalKind::Saturation)
            .map(|e| state_str(e.state))
            .unwrap_or("green")
            .to_string();

        rows.push(Row {
            name: fx.name,
            exp_state: fx.expect.state,
            pred_state: state_str(verdict.state).to_string(),
            exp_sig,
            pred_sig: trigger
                .map(|e| signal_str(e.signal))
                .unwrap_or("none")
                .to_string(),
            exp_turn: fx.expect.turn,
            pred_turn: trigger.and_then(|e| e.evidence.turn_index),
            naive_state,
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
    print_alert_delay(&rows);
    print_baseline_uplift(&rows);
    let hard_fp = print_hard_signal_gate(&rows);

    if !skipped.is_empty() {
        println!("\nSkipped (judge signals — evaluated separately):");
        for s in &skipped {
            println!("  • {s}");
        }
    }
    println!();

    // Gate mode: fail the process on any hard-signal false positive.
    if gate && hard_fp > 0 {
        eprintln!(
            "GATE FAILED: {hard_fp} hard-signal false positive(s). Hard signals must never cry wolf."
        );
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
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

/// Alert delay: turns between the annotated drift turn and the turn the engine
/// names as the cause. 0 = caught on the exact turn; positive = late (missed the
/// early window); negative = fired before the annotated onset (potential noise).
fn print_alert_delay(rows: &[Row]) {
    // Only correctly-triggered cases that carry both an expected and predicted
    // turn contribute — a delay is meaningless if we caught the wrong thing.
    let mut deltas: Vec<i64> = rows
        .iter()
        .filter(|r| r.exp_sig == r.pred_sig && r.exp_sig != "none")
        .filter_map(|r| match (r.exp_turn, r.pred_turn) {
            (Some(e), Some(p)) => Some(p as i64 - e as i64),
            _ => None,
        })
        .collect();
    println!("\nAlert delay (turns; predicted-turn − annotated drift-turn):");
    if deltas.is_empty() {
        println!("  (no cases carry a `turn` annotation — add one to measure timing)");
        return;
    }
    deltas.sort_unstable();
    let n = deltas.len();
    let median = deltas[n / 2];
    let mean = deltas.iter().sum::<i64>() as f64 / n as f64;
    let on_time = deltas.iter().filter(|d| **d == 0).count();
    println!(
        "  median: {median}   mean: {mean:.1}   on-the-turn: {on_time}/{n}   (range {}..{})",
        deltas.first().unwrap(),
        deltas.last().unwrap()
    );
}

/// Uplift of the full engine over a naive saturation-only detector. If the
/// engine can't beat "just watch the context fill up", the extra signals aren't
/// earning their keep.
fn print_baseline_uplift(rows: &[Row]) {
    if rows.is_empty() {
        return;
    }
    let engine_correct = rows.iter().filter(|r| r.exp_state == r.pred_state).count();
    let naive_correct = rows.iter().filter(|r| r.exp_state == r.naive_state).count();
    let n = rows.len();
    let engine_acc = 100.0 * engine_correct as f64 / n as f64;
    let naive_acc = 100.0 * naive_correct as f64 / n as f64;
    println!("\nBaseline comparison (state accuracy):");
    println!("  naive (saturation-only): {naive_correct}/{n} = {naive_acc:.1}%");
    println!("  full engine:             {engine_correct}/{n} = {engine_acc:.1}%");
    println!(
        "  uplift:                  {:+.1} pts",
        engine_acc - naive_acc
    );
}

/// The release go/no-go. Counts the two trust-killers and returns the total so
/// `--gate` can fail the build on any of them:
///   * hard false positive — a hard signal (constraint/saturation) named as the
///     cause when the annotation says the cause is something else (or nothing);
///   * false RED — predicting RED when the case is not RED.
///
/// Returns the number of offending cases (0 = clean).
fn print_hard_signal_gate(rows: &[Row]) -> usize {
    let hard_fps: Vec<&Row> = rows
        .iter()
        .filter(|r| HARD_SIGNALS.contains(&r.pred_sig.as_str()) && r.pred_sig != r.exp_sig)
        .collect();
    let false_reds: Vec<&Row> = rows
        .iter()
        .filter(|r| r.pred_state == "red" && r.exp_state != "red")
        .collect();

    // Union of the two (a case can be both).
    let offenders: Vec<&&Row> =
        hard_fps
            .iter()
            .chain(false_reds.iter())
            .fold(Vec::new(), |mut acc, r| {
                if !acc.iter().any(|x: &&&Row| x.name == r.name) {
                    acc.push(r);
                }
                acc
            });

    println!("\n{}", "-".repeat(66));
    println!("RELEASE GATE — hard signals must never cry wolf");
    println!("{}", "-".repeat(66));
    let n = rows.len();
    let rate = if n == 0 {
        0.0
    } else {
        100.0 * offenders.len() as f64 / n as f64
    };
    println!(
        "  hard-signal false positives: {}    false REDs: {}",
        hard_fps.len(),
        false_reds.len()
    );
    println!(
        "  offending cases: {}/{} = {rate:.1}%   →   {}",
        offenders.len(),
        n,
        if offenders.is_empty() { "PASS" } else { "FAIL" }
    );
    for r in &offenders {
        println!(
            "    ✗ {}  (expected {}/{}, got {}/{})",
            r.name, r.exp_state, r.exp_sig, r.pred_state, r.pred_sig
        );
    }
    offenders.len()
}
