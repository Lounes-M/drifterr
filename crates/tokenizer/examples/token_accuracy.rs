//! Token-estimation accuracy harness.
//!
//! Saturation (Signal 4) is only as trustworthy as its token count. On the proxy
//! channel we use the provider's **exact** `usage`; on the file / extension
//! channels we **estimate** with the heuristic tokenizer. This harness measures
//! that estimate against ground truth so the error is a number, not a hope.
//!
//! Ground truth is a JSONL of real (text, token-count) pairs — one per line:
//!
//!   {"text": "Hello, world!", "tokens": 4, "model": "gpt-4o"}
//!
//! The best source is your own proxied traffic: the provider returns the true
//! token count in `usage`, so you can log (assistant_text, usage.completion_tokens)
//! pairs and drop them here. See `ground-truth.example.jsonl` for the shape.
//!
//! Run:
//!   cargo run -p drifterr-tokenizer --example token_accuracy               # the example set
//!   cargo run -p drifterr-tokenizer --example token_accuracy -- truth.jsonl
//!
//! Reports, per provider family and overall: median / p95 / max relative error
//! and the share within 5% / 10% / 20%. The estimator biases high on purpose
//! (never under-report saturation), so a positive mean error is expected.

use drifterr_tokenizer::{HeuristicTokenizer, Tokenizer};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Deserialize)]
struct Truth {
    text: String,
    tokens: usize,
    #[serde(default)]
    model: Option<String>,
}

struct Sample {
    provider: String,
    err: f64, // signed relative error: (estimate - actual) / actual
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ground-truth.example.jsonl")
        });
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));

    let mut samples: Vec<Sample> = Vec::new();
    for (i, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let t: Truth = match serde_json::from_str(line) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("skip line {}: {e}", i + 1);
                continue;
            }
        };
        if t.tokens == 0 {
            continue;
        }
        let model = t.model.clone().unwrap_or_else(|| "generic".into());
        let est = HeuristicTokenizer::for_model(&model).count(&t.text);
        let err = (est as f64 - t.tokens as f64) / t.tokens as f64;
        samples.push(Sample {
            provider: provider_of(&model),
            err,
        });
    }

    if samples.is_empty() {
        eprintln!("no usable ground-truth rows in {}", path.display());
        std::process::exit(1);
    }

    println!("\n{}", "=".repeat(60));
    println!(
        "Token-estimation accuracy — {} ({} rows)",
        path.display(),
        samples.len()
    );
    println!("{}", "=".repeat(60));
    println!(
        "  {:<12}{:>7}{:>9}{:>9}{:>9}{:>8}{:>8}",
        "provider", "n", "median", "p95", "max", "≤5%", "≤10%"
    );

    let mut groups: Vec<&str> = samples.iter().map(|s| s.provider.as_str()).collect();
    groups.sort_unstable();
    groups.dedup();
    for g in groups.iter().chain(std::iter::once(&"ALL")) {
        let rows: Vec<f64> = samples
            .iter()
            .filter(|s| *g == "ALL" || s.provider == *g)
            .map(|s| s.err)
            .collect();
        report(g, &rows);
    }
    println!(
        "{}\n  Estimator biases high by design (never under-report saturation).",
        "=".repeat(60)
    );
    println!();
}

fn provider_of(model: &str) -> String {
    let m = model.to_ascii_lowercase();
    if m.contains("gpt") || m.starts_with("o1") || m.starts_with("o3") || m.contains("openai") {
        "openai".into()
    } else if m.contains("claude") || m.contains("anthropic") {
        "anthropic".into()
    } else {
        "generic".into()
    }
}

fn report(name: &str, errs: &[f64]) {
    let mut abs: Vec<f64> = errs.iter().map(|e| e.abs()).collect();
    abs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = abs.len();
    let pct = |q: f64| abs[((n as f64 - 1.0) * q).round() as usize];
    let within = |thr: f64| 100.0 * abs.iter().filter(|e| **e <= thr).count() as f64 / n as f64;
    println!(
        "  {:<12}{:>7}{:>8.1}%{:>8.1}%{:>8.1}%{:>7.0}%{:>7.0}%",
        name,
        n,
        100.0 * pct(0.5),
        100.0 * pct(0.95),
        100.0 * abs[n - 1],
        within(0.05),
        within(0.10),
    );
}
