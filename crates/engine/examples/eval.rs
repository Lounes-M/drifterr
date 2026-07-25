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
//!   cargo run -p drifterr-engine --example eval                  # scans fixtures/
//!   cargo run -p drifterr-engine --example eval -- eval/         # a bigger set
//!   cargo run -p drifterr-engine --example eval -- eval/ --gate  # CI gate mode
//!   cargo run -p drifterr-engine --example eval -- eval/ --sweep # calibrate goal thresholds
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
/// Soft, advisory signals the pure engine emits (AMBER-capped).
const SOFT_SIGNALS: [&str; 2] = ["goal_alignment", "degradation"];

/// Release-gate thresholds, loaded from `eval/thresholds.conf` (falling back to
/// these defaults). See that file + eval/SCHEMA.md for the enforcement tiers.
struct Thresholds {
    min_cases: usize,
    hard_max_median_delay: i64,
    soft_min_precision: f64,
    soft_max_median_delay: i64,
    baseline_min_uplift_pts: f64,
    goal_min_recall: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds {
            min_cases: 30,
            hard_max_median_delay: 0,
            soft_min_precision: 0.60,
            soft_max_median_delay: 2,
            baseline_min_uplift_pts: 10.0,
            goal_min_recall: 0.70,
        }
    }
}

impl Thresholds {
    /// Load overrides from `eval/thresholds.conf` (a flat `key = number` file),
    /// keeping defaults for anything absent or unparseable.
    fn load() -> Self {
        let mut t = Thresholds::default();
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("eval")
            .join("thresholds.conf");
        let Ok(text) = fs::read_to_string(&path) else {
            return t;
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "min_cases" => {
                    if let Ok(n) = v.parse() {
                        t.min_cases = n;
                    }
                }
                "hard.max_median_delay" => {
                    if let Ok(n) = v.parse() {
                        t.hard_max_median_delay = n;
                    }
                }
                "soft.min_precision" => {
                    if let Ok(n) = v.parse() {
                        t.soft_min_precision = n;
                    }
                }
                "soft.max_median_delay" => {
                    if let Ok(n) = v.parse() {
                        t.soft_max_median_delay = n;
                    }
                }
                "baseline.min_uplift_pts" => {
                    if let Ok(n) = v.parse() {
                        t.baseline_min_uplift_pts = n;
                    }
                }
                "goal.min_recall" => {
                    if let Ok(n) = v.parse() {
                        t.goal_min_recall = n;
                    }
                }
                _ => {}
            }
        }
        t
    }
}

/// Median of a slice (rounds the lower-middle for even counts). Empty ⇒ None.
fn median_i64(xs: &mut [i64]) -> Option<i64> {
    if xs.is_empty() {
        return None;
    }
    xs.sort_unstable();
    Some(xs[xs.len() / 2])
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

fn state_str(s: State) -> &'static str {
    match s {
        State::Green => "green",
        State::Amber => "amber",
        State::Red => "red",
    }
}

/// Calibrate the goal-alignment thresholds against a corpus instead of arguing
/// about them.
///
/// Walks a grid of [`GoalThresholds`] and reports precision / recall / F1 for the
/// goal signal at each point, marking the current defaults. This exists because the
/// old thresholds were three hand-picked constants that nobody could justify, and
/// one of them (an absolute similarity floor) was structurally wrong — the kind of
/// mistake you only see when you can measure alternatives side by side.
///
/// Case accounting, stated explicitly because it's the part that makes the numbers
/// mean something:
///
/// * **positive** — the case annotates `goal_alignment` as the cause. The signal
///   should fire.
/// * **negative** — the case annotates `green`. The signal must NOT fire; these are
///   the false-positive guards.
/// * **ignored** — the case expects some *other* non-green cause (a constraint
///   violation, say). Goal alignment may legitimately also be amber there, so
///   counting it either way would be arbitrary.
///
/// A grid point that fires on nothing scores recall 0, and one that fires on
/// everything scores precision 0, so neither degenerate extreme wins.
fn sweep_goal_thresholds(entries: &[PathBuf]) -> ExitCode {
    use drifterr_engine::signals::goal::{evaluate_with, GoalThresholds};

    struct Case {
        fx: Fixture,
        /// `Some(true)` positive, `Some(false)` negative, `None` ignored.
        polarity: Option<bool>,
    }

    let mut cases = Vec::new();
    for path in entries {
        let Some(fx) = fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Fixture>(&s).ok())
        else {
            continue;
        };
        let sig = fx.expect.triggering_signal.as_deref().unwrap_or("none");
        let polarity = if sig == "goal_alignment" {
            Some(true)
        } else if fx.expect.state == "green" {
            Some(false)
        } else {
            None
        };
        cases.push(Case { fx, polarity });
    }

    let positives = cases.iter().filter(|c| c.polarity == Some(true)).count();
    let negatives = cases.iter().filter(|c| c.polarity == Some(false)).count();
    let ignored = cases.iter().filter(|c| c.polarity.is_none()).count();

    println!("Goal-threshold sweep");
    println!("{}", "=".repeat(78));
    println!(
        "cases: {positives} positive · {negatives} negative (must stay quiet) · {ignored} ignored"
    );
    if positives == 0 {
        println!(
            "\nNo cases annotate goal_alignment as the cause, so precision/recall here are\n\
             not measurable. Add goal-drift cases before trusting a sweep."
        );
    }
    let embedder = drifterr_embeddings::default_embedder();
    let default = GoalThresholds::default();
    println!(
        "\nembedder: {}   (absolute cosine scale differs per embedder — this is why the\n\
         proportional threshold, not an absolute floor, is the real test)",
        if cfg!(feature = "onnx") {
            "semantic (onnx feature on)"
        } else {
            "lexical bag-of-words"
        }
    );

    println!(
        "\n{:>6} {:>9} {:>7} {:>4} {:>4} {:>4} {:>6} {:>6} {:>6}",
        "drop", "rel_drop", "window", "TP", "FP", "FN", "prec", "recall", "F1"
    );
    println!("{}", "-".repeat(78));

    let drops = [0.0_f32, 0.03, 0.05, 0.08, 0.12];
    let rel_drops = [0.10_f32, 0.15, 0.25, 0.35, 0.50];
    let windows = [1_usize, 2, 3];

    let mut best: Option<(f32, GoalThresholds)> = None;
    for &window in &windows {
        for &drop in &drops {
            for &rel in &rel_drops {
                let cfg = GoalThresholds {
                    min_turns: default.min_turns,
                    recent_window: window,
                    min_drop: drop,
                    min_rel_drop: rel,
                };
                let (mut tp, mut fp, mut fnn) = (0usize, 0usize, 0usize);
                for case in &cases {
                    let Some(want) = case.polarity else { continue };
                    let fired =
                        evaluate_with(&case.fx.baseline, &case.fx.conversation, &*embedder, &cfg)
                            .is_some();
                    match (want, fired) {
                        (true, true) => tp += 1,
                        (true, false) => fnn += 1,
                        (false, true) => fp += 1,
                        (false, false) => {}
                    }
                }
                let prec = if tp + fp == 0 {
                    f32::NAN
                } else {
                    tp as f32 / (tp + fp) as f32
                };
                let rec = if tp + fnn == 0 {
                    f32::NAN
                } else {
                    tp as f32 / (tp + fnn) as f32
                };
                let f1 = if prec.is_nan() || rec.is_nan() || prec + rec == 0.0 {
                    f32::NAN
                } else {
                    2.0 * prec * rec / (prec + rec)
                };
                let marker = if cfg == default { "  ← current" } else { "" };
                let fmt = |v: f32| {
                    if v.is_nan() {
                        "  —".to_string()
                    } else {
                        format!("{v:.2}")
                    }
                };
                println!(
                    "{drop:>6.2} {rel:>9.2} {window:>7} {tp:>4} {fp:>4} {fnn:>4} {:>6} {:>6} {:>6}{marker}",
                    fmt(prec),
                    fmt(rec),
                    fmt(f1)
                );
                if !f1.is_nan() && best.as_ref().is_none_or(|(b, _)| f1 > *b) {
                    best = Some((f1, cfg));
                }
            }
        }
    }

    println!("{}", "-".repeat(78));
    match best {
        Some((f1, cfg)) if cfg != default => println!(
            "Best F1 {f1:.2} at drop={:.2} rel_drop={:.2} window={} \
             (current default is drop={:.2} rel_drop={:.2} window={})",
            cfg.min_drop,
            cfg.min_rel_drop,
            cfg.recent_window,
            default.min_drop,
            default.min_rel_drop,
            default.recent_window
        ),
        Some((f1, _)) => {
            println!("Current defaults are already the best point on this grid (F1 {f1:.2}).")
        }
        None => println!("No grid point was measurable on this set."),
    }
    println!(
        "\nOverride at runtime without a rebuild:\n  \
         DRIFTERR_GOAL_MIN_DROP  DRIFTERR_GOAL_MIN_REL_DROP  \
         DRIFTERR_GOAL_RECENT_WINDOW  DRIFTERR_GOAL_MIN_TURNS"
    );
    println!(
        "\nA grid this small on a handful of cases CANNOT justify shipping new defaults —\n\
         it will overfit. Treat the winner as a hypothesis until the corpus is large\n\
         enough for eval/blind/ to confirm it. See eval/SCHEMA.md."
    );
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    // Args: an optional directory (first non-flag) and an optional `--gate`.
    let mut dir: Option<PathBuf> = None;
    let mut gate = false;
    let mut sweep = false;
    for arg in std::env::args().skip(1) {
        if arg == "--gate" {
            gate = true;
        } else if arg == "--sweep" {
            sweep = true;
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

    if sweep {
        return sweep_goal_thresholds(&entries);
    }

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
    let thr = Thresholds::load();
    let release_ok = print_release_gate(&rows, &thr);

    if !skipped.is_empty() {
        println!("\nSkipped (judge signals — evaluated separately):");
        for s in &skipped {
            println!("  • {s}");
        }
    }
    println!();

    // Gate mode: fail the process if any RELEASE-blocking threshold was missed.
    if gate && !release_ok {
        eprintln!(
            "GATE FAILED: a release-blocking threshold was not met (see RELEASE GATE above)."
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

/// The release go/no-go, against `thresholds.conf`. Prints every gated metric
/// with its threshold and verdict, and returns whether the RELEASE-blocking
/// gates all pass (the value `--gate` acts on). Two tiers:
///   * non-negotiables (hard FP / false RED / premature / hard delay) — always
///     block; valid on any set;
///   * statistical (soft precision/delay, uplift) — block only once the relevant
///     case count reaches `min_cases`, else reported `n/a`.
///
/// `goal-drift recall` is reported as a *claim* gate — it never blocks the
/// release, only decides whether the "semantic goal detection" label is earned.
fn print_release_gate(rows: &[Row], thr: &Thresholds) -> bool {
    let n = rows.len();
    let hard = |s: &str| HARD_SIGNALS.contains(&s);
    let soft = |s: &str| SOFT_SIGNALS.contains(&s);

    // --- non-negotiables ---
    let hard_fp = rows
        .iter()
        .filter(|r| hard(&r.pred_sig) && r.pred_sig != r.exp_sig)
        .count();
    let false_red = rows
        .iter()
        .filter(|r| r.pred_state == "red" && r.exp_state != "red")
        .count();
    let premature = rows
        .iter()
        .filter(|r| {
            hard(&r.pred_sig) && matches!((r.exp_turn, r.pred_turn), (Some(e), Some(p)) if p < e)
        })
        .count();
    let mut hard_delays: Vec<i64> = rows
        .iter()
        .filter(|r| r.exp_sig == r.pred_sig && hard(&r.pred_sig))
        .filter_map(|r| match (r.exp_turn, r.pred_turn) {
            (Some(e), Some(p)) => Some(p as i64 - e as i64),
            _ => None,
        })
        .collect();
    let hard_delay_med = median_i64(&mut hard_delays);

    // --- statistical ---
    let soft_pred: Vec<&Row> = rows.iter().filter(|r| soft(&r.pred_sig)).collect();
    let soft_correct = soft_pred.iter().filter(|r| r.pred_sig == r.exp_sig).count();
    let soft_precision = if soft_pred.is_empty() {
        1.0
    } else {
        soft_correct as f64 / soft_pred.len() as f64
    };
    let mut soft_delays: Vec<i64> = rows
        .iter()
        .filter(|r| r.exp_sig == r.pred_sig && soft(&r.pred_sig))
        .filter_map(|r| match (r.exp_turn, r.pred_turn) {
            (Some(e), Some(p)) => Some(p as i64 - e as i64),
            _ => None,
        })
        .collect();
    let soft_delay_n = soft_delays.len();
    let soft_delay_med = median_i64(&mut soft_delays);

    let engine_correct = rows.iter().filter(|r| r.exp_state == r.pred_state).count();
    let naive_correct = rows.iter().filter(|r| r.exp_state == r.naive_state).count();
    let uplift = if n == 0 {
        0.0
    } else {
        100.0 * (engine_correct as f64 - naive_correct as f64) / n as f64
    };

    // --- claim gate ---
    let goal_total = rows
        .iter()
        .filter(|r| r.exp_sig == "goal_alignment")
        .count();
    let goal_hit = rows
        .iter()
        .filter(|r| r.exp_sig == "goal_alignment" && r.pred_sig == "goal_alignment")
        .count();
    let goal_recall = if goal_total == 0 {
        0.0
    } else {
        goal_hit as f64 / goal_total as f64
    };

    // Verdicts. `ok` accumulates only the RELEASE-blocking gates.
    let mut ok = true;
    let pf = |pass: bool| if pass { "PASS" } else { "FAIL" };
    // A statistical gate: enforced only at ≥ min_cases, else n/a.
    let stat = |pass: bool, cases: usize, block: &mut bool| -> String {
        if cases < thr.min_cases {
            format!("n/a (need {} cases, have {cases})", thr.min_cases)
        } else {
            if !pass {
                *block = false;
            }
            pf(pass).to_string()
        }
    };

    if hard_fp > 0 || false_red > 0 || premature > 0 {
        ok = false;
    }
    let hard_delay_ok = hard_delay_med.is_none_or(|m| m <= thr.hard_max_median_delay);
    if !hard_delay_ok {
        ok = false;
    }

    println!("\n{}", "-".repeat(66));
    println!("RELEASE GATE  (thresholds: eval/thresholds.conf)");
    println!("{}", "-".repeat(66));
    println!("Non-negotiable — always enforced:");
    println!(
        "  hard-signal false positives   {hard_fp:>6}       required 0    {}",
        pf(hard_fp == 0)
    );
    println!(
        "  false REDs                    {false_red:>6}       required 0    {}",
        pf(false_red == 0)
    );
    println!(
        "  premature hard alerts         {premature:>6}       required 0    {}",
        pf(premature == 0)
    );
    match hard_delay_med {
        Some(m) => println!(
            "  hard median delay            {m:>6} turns  ≤ {}          {}",
            thr.hard_max_median_delay,
            pf(hard_delay_ok)
        ),
        None => println!(
            "  hard median delay             {:>6}       ≤ {}          n/a (no timed cases)",
            "—", thr.hard_max_median_delay
        ),
    }

    println!("Statistical — enforced at ≥ {} cases:", thr.min_cases);
    let v_soft_p = stat(
        soft_precision >= thr.soft_min_precision,
        soft_pred.len(),
        &mut ok,
    );
    println!(
        "  soft precision              {:>6.2} (n={})   ≥ {:.2}       {v_soft_p}",
        soft_precision,
        soft_pred.len(),
        thr.soft_min_precision
    );
    let v_soft_d = match soft_delay_med {
        Some(m) => {
            let s = stat(m <= thr.soft_max_median_delay, soft_delay_n, &mut ok);
            format!(
                "{m} turns (n={soft_delay_n})   ≤ {}       {s}",
                thr.soft_max_median_delay
            )
        }
        None => format!(
            "—       ≤ {}       n/a (no timed cases)",
            thr.soft_max_median_delay
        ),
    };
    println!("  soft median delay            {v_soft_d}");
    let v_uplift = stat(uplift >= thr.baseline_min_uplift_pts, n, &mut ok);
    println!(
        "  uplift vs baseline          {:>+6.1} pts (n={n})  ≥ {:+.1}   {v_uplift}",
        uplift, thr.baseline_min_uplift_pts
    );

    println!("Claim gate — labels the 'semantic goal' signal, never blocks release:");
    let goal_verdict = if goal_total < thr.min_cases {
        format!("n/a (need {} goal cases, have {goal_total})", thr.min_cases)
    } else if goal_recall >= thr.goal_min_recall {
        "EARNED".to_string()
    } else {
        "NOT EARNED — keep goal signal 'best-effort'".to_string()
    };
    println!(
        "  goal-drift recall           {:>5.0}% (n={goal_total})   ≥ {:.0}%     {goal_verdict}",
        100.0 * goal_recall,
        100.0 * thr.goal_min_recall
    );

    // Offending case detail for the non-negotiables.
    for r in rows.iter().filter(|r| {
        (hard(&r.pred_sig) && r.pred_sig != r.exp_sig)
            || (r.pred_state == "red" && r.exp_state != "red")
    }) {
        println!(
            "    ✗ {}  (expected {}/{}, got {}/{})",
            r.name, r.exp_state, r.exp_sig, r.pred_state, r.pred_sig
        );
    }

    println!(
        "{}\n  RELEASE: {}",
        "-".repeat(66),
        if ok { "PASS" } else { "FAIL" }
    );
    ok
}
