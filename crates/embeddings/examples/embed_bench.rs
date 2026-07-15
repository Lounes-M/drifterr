//! Embedder benchmark — the numbers you need before bundling the semantic model:
//! **model size, cold-load time, per-embed latency, output dim, memory**, plus a
//! quick goal-vs-drift separation check.
//!
//! Works with either embedder. Without the feature it benches the lexical
//! `BagEmbedder` (a useful zero-cost baseline); with `--features onnx` and
//! `DRIFTERR_EMBED_MODEL` pointing at a model directory it benches the semantic
//! ONNX model (and stats the on-disk model file):
//!
//!   cargo run -p drifterr-embeddings --example embed_bench
//!   DRIFTERR_EMBED_MODEL=/path/to/bge cargo run -p drifterr-embeddings \
//!     --features onnx --example embed_bench
//!
//! For goal-drift *recall* on the annotated set (not just separation), run the
//! detection eval with the semantic embedder wired in:
//!   DRIFTERR_EMBED_MODEL=/path/to/bge \
//!     cargo run -p drifterr-engine --features onnx --example eval -- eval/

use std::time::Instant;

use drifterr_embeddings::{cosine, default_embedder, Embedder};

/// Representative short "turns" — the length profile the soft signals see.
const CORPUS: &[&str] = &[
    "Reduce cold-start latency of our AWS Lambda functions.",
    "Enable provisioned concurrency and move heavy imports below the handler.",
    "Let's refactor the auth module into strict TypeScript with no JS files.",
    "Here is the refactored authentication module, split into services.",
    "Actually, let's redesign the landing page hero and pick a new palette.",
    "Add a CSV export endpoint on the server, covered by integration tests.",
    "The build is failing on Windows with an LNK2038 runtime-library mismatch.",
    "We should cache the tokenizer so repeated embeds don't re-parse the vocab.",
];

fn read_rss_kb() -> Option<u64> {
    // Linux-only best effort (statm is in pages). Elsewhere → None.
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let resident_pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    let page_kb = 4; // 4 KiB pages on all our targets.
    Some(resident_pages * page_kb)
}

fn fmt_kb(kb: u64) -> String {
    if kb >= 1024 {
        format!("{:.1} MB", kb as f64 / 1024.0)
    } else {
        format!("{kb} KB")
    }
}

fn main() {
    println!("\n=== drifterr embedder benchmark ===");

    // Which embedder is active, and (for ONNX) the on-disk model size.
    let model_dir = std::env::var("DRIFTERR_EMBED_MODEL").unwrap_or_default();
    let onnx_feature = cfg!(feature = "onnx");
    println!("onnx feature compiled: {onnx_feature}");
    if model_dir.trim().is_empty() {
        println!("DRIFTERR_EMBED_MODEL: (unset) → lexical BagEmbedder");
    } else {
        println!("DRIFTERR_EMBED_MODEL: {model_dir}");
        let model_path = std::path::Path::new(model_dir.trim()).join("model.onnx");
        match std::fs::metadata(&model_path) {
            Ok(m) => println!("model.onnx size:      {}", fmt_kb(m.len() / 1024)),
            Err(_) => println!("model.onnx size:      (not found — will fall back to lexical)"),
        }
    }

    let rss_before = read_rss_kb();

    // Cold load (this is where the ONNX runtime + weights are read).
    let t0 = Instant::now();
    let embedder: Box<dyn Embedder> = default_embedder();
    let load_ms = t0.elapsed().as_secs_f64() * 1e3;
    println!("cold load:            {load_ms:.1} ms");

    // Dimension + a warm-up pass (JIT graph optimization, allocator warmup).
    let dim = embedder.embed(CORPUS[0]).len();
    for s in CORPUS {
        let _ = embedder.embed(s);
    }
    println!("output dimension:     {dim}");

    // Latency: many iterations over the corpus; report mean + p50 + throughput.
    let iters = 50usize;
    let mut samples: Vec<f64> = Vec::with_capacity(iters * CORPUS.len());
    let t1 = Instant::now();
    for _ in 0..iters {
        for s in CORPUS {
            let t = Instant::now();
            let _ = embedder.embed(s);
            samples.push(t.elapsed().as_secs_f64() * 1e3);
        }
    }
    let total_s = t1.elapsed().as_secs_f64();
    samples.sort_by(|a, b| a.total_cmp(b));
    let n = samples.len();
    let mean = samples.iter().sum::<f64>() / n as f64;
    let p50 = samples[n / 2];
    let p95 = samples[(n as f64 * 0.95) as usize];
    println!(
        "embed latency:        mean {mean:.3} ms   p50 {p50:.3} ms   p95 {p95:.3} ms   ({:.0} embeds/s)",
        n as f64 / total_s
    );

    if let (Some(before), Some(after)) = (rss_before, read_rss_kb()) {
        println!(
            "resident memory:      {} → {}  (Δ {})",
            fmt_kb(before),
            fmt_kb(after),
            fmt_kb(after.saturating_sub(before))
        );
    } else {
        println!("resident memory:      n/a (Linux-only measurement)");
    }

    // Quick quality check: the goal should separate an on-topic turn from a
    // drift turn. Lexical scores this ~0.05; a semantic model ~0.25+.
    let g = embedder.embed(CORPUS[0]);
    let ontopic = cosine(&g, &embedder.embed(CORPUS[1]));
    let drift = cosine(&g, &embedder.embed(CORPUS[4]));
    println!(
        "goal separation:      on-topic {ontopic:.3}  ·  drift {drift:.3}  ·  gap {:+.3}",
        ontopic - drift
    );
    println!();
}
