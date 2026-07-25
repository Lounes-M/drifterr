//! Turn real local sessions into eval annotation stubs.
//!
//! The engine's detection quality currently rests on a handful of hand-written
//! fixtures, authored by the same person who wrote the engine. That is enough to
//! catch regressions and nowhere near enough to claim accuracy. The missing
//! ingredient is **real annotated sessions**, and the friction in getting them was
//! that every case had to be hand-transcribed into the eval schema.
//!
//! This tool removes that friction: it reads your own local store (or a feedback
//! file) and writes schema-valid cases into `eval/`. You still do the annotation —
//! see below for why that is deliberate.
//!
//! ```bash
//! # Sessions from the local DB → stubs you then annotate by hand
//! cargo run -p drifterr-store --example annotate -- --db ~/.drifterr/drifterr.db --out eval/inbox
//!
//! # "This wasn't drift" reports → ready-to-use green cases (the user is the label)
//! cargo run -p drifterr-store --example annotate -- --feedback ~/.drifterr/feedback.jsonl --out eval/inbox
//! ```
//!
//! # Why store-derived cases are NOT pre-labelled
//!
//! It would be trivial to run the engine over each session and write its own verdict
//! into `expect`. It would also make the corpus worthless: the engine would be
//! graded against its own output, every case would pass by construction, and the
//! resulting accuracy number would measure nothing. So store-derived stubs ship with
//! `expect.state: "TODO"`, and the eval harness **refuses to load** a case still
//! marked TODO. The only way to get a number out of this pipeline is to have a human
//! decide what the right answer was.
//!
//! Feedback-derived cases are different, and legitimately auto-labelled: a
//! "this wasn't drift" report *is* a human label. The user looked at an alert and
//! said it was wrong, so `expect.state` is `green` on their authority, not the
//! engine's.
//!
//! # Privacy
//!
//! Output contains your conversations **verbatim**. It is written locally and nothing
//! is uploaded, but these files are exactly what you'd be sharing if you contributed
//! a corpus upstream. Read them before sharing them; redact anything you wouldn't
//! post publicly. The tool prints this warning on every run for a reason.

use drifterr_store::Store;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn main() -> std::process::ExitCode {
    let mut db: Option<PathBuf> = None;
    let mut feedback: Option<PathBuf> = None;
    let mut out = PathBuf::from("eval/inbox");
    let mut limit = 200usize;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--db" => {
                db = args.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--feedback" => {
                feedback = args.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--out" => {
                if let Some(v) = args.get(i + 1) {
                    out = PathBuf::from(v);
                }
                i += 2;
            }
            "--limit" => {
                limit = args
                    .get(i + 1)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(limit);
                i += 2;
            }
            "--help" | "-h" => {
                print_help();
                return std::process::ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument: {other}");
                print_help();
                return std::process::ExitCode::FAILURE;
            }
        }
    }

    if db.is_none() && feedback.is_none() {
        eprintln!("nothing to do: pass --db and/or --feedback\n");
        print_help();
        return std::process::ExitCode::FAILURE;
    }

    if let Err(e) = std::fs::create_dir_all(&out) {
        eprintln!("cannot create {}: {e}", out.display());
        return std::process::ExitCode::FAILURE;
    }

    let mut written = 0usize;
    if let Some(path) = &db {
        match export_sessions(path, &out, limit) {
            Ok(n) => {
                written += n;
                println!("{n} session stub(s) written from {}", path.display());
            }
            Err(e) => {
                eprintln!("store export failed: {e}");
                return std::process::ExitCode::FAILURE;
            }
        }
    }
    if let Some(path) = &feedback {
        match export_feedback(path, &out) {
            Ok(n) => {
                written += n;
                println!("{n} feedback case(s) written from {}", path.display());
            }
            Err(e) => {
                eprintln!("feedback export failed: {e}");
                return std::process::ExitCode::FAILURE;
            }
        }
    }

    if written == 0 {
        println!("\nNothing to export — no sessions or feedback found.");
        return std::process::ExitCode::SUCCESS;
    }

    println!("\nOutput: {}", out.display());
    println!(
        "\n\x1b[1mPRIVACY:\x1b[0m these files contain your conversations verbatim. Nothing was\n\
         uploaded, but read them before sharing and redact anything you would not post\n\
         publicly."
    );
    println!(
        "\n\x1b[1mNEXT:\x1b[0m session stubs carry `expect.state: \"TODO\"`. Fill in what the right\n\
         answer actually was — the state, the cause, and the turn the drift starts — then\n\
         move the file into eval/ (development) or eval/blind/ (holdout). The eval harness\n\
         refuses TODO cases on purpose: an engine graded against its own output measures\n\
         nothing. See eval/SCHEMA.md."
    );
    std::process::ExitCode::SUCCESS
}

fn print_help() {
    println!(
        "Turn real local sessions into eval annotation stubs.\n\n\
         USAGE:\n  \
           annotate [--db <path>] [--feedback <path>] [--out <dir>] [--limit <n>]\n\n\
         OPTIONS:\n  \
           --db <path>        local SQLite store to read sessions from\n  \
           --feedback <path>  feedback.jsonl of \"this wasn't drift\" reports\n  \
           --out <dir>        where to write cases (default: eval/inbox)\n  \
           --limit <n>        max sessions to export (default: 200)\n"
    );
}

/// Export each stored session as an annotation stub.
fn export_sessions(db: &Path, out: &Path, limit: usize) -> Result<usize, String> {
    let store = Store::open(db.to_str().ok_or("db path is not valid UTF-8")?)
        .map_err(|e| format!("cannot open store: {e}"))?;
    let sessions = store
        .list_sessions(limit)
        .map_err(|e| format!("cannot list sessions: {e}"))?;

    let mut n = 0;
    for s in sessions {
        // A session with almost no turns cannot illustrate drift.
        if s.turns < 3 {
            continue;
        }
        let Ok(conv) = store.load_conversation(&s.session_id) else {
            continue;
        };
        let baseline = store.load_baseline(&s.session_id).unwrap_or_else(|_| {
            drifterr_engine::baseline::Baseline {
                goal: String::new(),
                constraints: Vec::new(),
                decisions: Vec::new(),
            }
        });
        // A case with no stated intent has no ground truth to measure against —
        // drift is defined relative to a baseline.
        if baseline.goal.trim().is_empty() && baseline.constraints.is_empty() {
            continue;
        }

        let case = json!({
            "name": format!(
                "REAL session {} ({} turns) — ANNOTATE ME",
                s.session_id, s.turns
            ),
            "_annotation": {
                "instructions": "Replace expect.state with green|amber|red. If not green, \
                    set triggeringSignal (constraint|saturation|goal_alignment|\
                    decision_coherence|degradation) and, where you can, the 0-based turn \
                    where the drift begins. Delete this _annotation block when done.",
                "doNotPrefill": "Do not paste the engine's own verdict here. A corpus \
                    graded against the engine's output measures nothing.",
                "source": "local store",
                "sessionId": s.session_id,
            },
            "baseline": baseline,
            "conversation": conv,
            // Deliberately un-answerable until a human edits it. The harness rejects
            // "TODO", so this file cannot silently inflate an accuracy number.
            "expect": { "state": "TODO" }
        });

        let file = out.join(format!("real_{}.json", sanitize(&s.session_id)));
        write_case(&file, &case)?;
        n += 1;
    }
    Ok(n)
}

/// Export "this wasn't drift" reports as green cases.
///
/// These are legitimately pre-labelled: the user looked at an alert and said it was
/// wrong, so `green` is *their* label. That makes each one a false-positive guard,
/// which is the most valuable kind of case this project can collect — it is the
/// metric the release gate is built around.
fn export_feedback(path: &Path, out: &Path) -> Result<usize, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut n = 0;
    for (idx, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            eprintln!("skip malformed feedback line {}", idx + 1);
            continue;
        };
        // Only the "not drift" label is auto-usable; anything else needs a human.
        if v.get("label").and_then(Value::as_str) != Some("not_drift") {
            continue;
        }
        let session = v
            .get("session")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();

        let case = json!({
            "name": format!(
                "REAL user-reported false positive on {} ({}) — must stay green",
                v.get("triggeringSignal").and_then(Value::as_str).unwrap_or("unknown signal"),
                session
            ),
            "_annotation": {
                "source": "user feedback (this wasn't drift)",
                "note": "expect.state is green on the USER's authority, not the engine's. \
                    The conversation turns are NOT in the feedback record, so paste them \
                    in from the session before using this case.",
                "observedState": v.get("observedState").cloned().unwrap_or(Value::Null),
                "detail": v.get("detail").cloned().unwrap_or(Value::Null),
                "span": v.get("span").cloned().unwrap_or(Value::Null),
            },
            "baseline": {
                "goal": v.get("goal").and_then(Value::as_str).unwrap_or_default(),
                "constraints": [],
                "decisions": []
            },
            "conversation": {
                "sessionId": session,
                "model": v.get("model").and_then(Value::as_str).unwrap_or("unknown"),
                "turns": [],
                "context": { "windowSize": 128000, "usedTokens": 0, "exact": false, "toolCallCount": 0 },
                "source": "file"
            },
            "expect": { "state": "green" }
        });

        let file = out.join(format!("fp_{}_{}.json", sanitize(&session), idx));
        write_case(&file, &case)?;
        n += 1;
    }
    Ok(n)
}

fn write_case(path: &Path, case: &Value) -> Result<(), String> {
    let body = serde_json::to_string_pretty(case)
        .map_err(|e| format!("cannot serialize {}: {e}", path.display()))?;
    std::fs::write(path, body + "\n").map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Make a session id safe for a filename.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}
