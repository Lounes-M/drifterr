//! Drifterr proxy binary.
//!
//! Point your tool at the proxy port and read state from the control port:
//!
//! ```text
//! OPENAI_BASE_URL=http://localhost:8787/v1   # OpenAI-style tools
//! ANTHROPIC_BASE_URL=http://localhost:8787   # Anthropic-style tools
//! curl http://localhost:8788/status          # live drift status (JSON)
//! ```
//!
//! Configuration via environment (a `.env` file in the working directory is
//! auto-loaded; see `.env.example`):
//! * `DRIFTERR_PROXY_ADDR`     (default `127.0.0.1:8787`)
//! * `DRIFTERR_CONTROL_ADDR`   (default `127.0.0.1:8788`)
//! * `DRIFTERR_DB`             SQLite path (default: in-memory, not persisted)
//! * `OPENAI_UPSTREAM`         (default `https://openrouter.ai/api` — OpenRouter)
//! * `ANTHROPIC_UPSTREAM`      (default `https://api.anthropic.com`)

use drifterr_proxy::{serve, AppState, ProxyConfig};
use drifterr_store::Store;
use std::net::SocketAddr;

/// Default OpenAI-compatible upstream. Drifterr standardizes on OpenRouter for
/// all agent calls; it speaks the OpenAI schema, so `/v1/chat/completions`
/// relays straight through. Override with `OPENAI_UPSTREAM` for plain OpenAI or
/// a local server.
const DEFAULT_OPENAI_UPSTREAM: &str = "https://openrouter.ai/api";
const DEFAULT_ANTHROPIC_UPSTREAM: &str = "https://api.anthropic.com";

#[tokio::main]
async fn main() {
    // Load a local .env if present (no-op otherwise), so config persists.
    dotenvy::dotenv().ok();

    let proxy_addr: SocketAddr = env_or("DRIFTERR_PROXY_ADDR", "127.0.0.1:8787")
        .parse()
        .expect("invalid DRIFTERR_PROXY_ADDR");
    let control_addr: SocketAddr = env_or("DRIFTERR_CONTROL_ADDR", "127.0.0.1:8788")
        .parse()
        .expect("invalid DRIFTERR_CONTROL_ADDR");

    let cfg = ProxyConfig {
        openai_upstream: env_or("OPENAI_UPSTREAM", DEFAULT_OPENAI_UPSTREAM),
        anthropic_upstream: env_or("ANTHROPIC_UPSTREAM", DEFAULT_ANTHROPIC_UPSTREAM),
    };

    let store = match std::env::var("DRIFTERR_DB") {
        Ok(path) if !path.is_empty() => match Store::open(&path) {
            Ok(s) => {
                eprintln!("drifterr: persisting to {path}");
                Some(s)
            }
            Err(e) => {
                eprintln!("drifterr: could not open {path}: {e} — running without persistence");
                None
            }
        },
        _ => None,
    };

    eprintln!("drifterr proxy    → {proxy_addr}  (point your tool here)");
    eprintln!("drifterr control  → http://{control_addr}/  (dashboard + /status)");
    eprintln!("drifterr upstream → {}", cfg.openai_upstream);

    let state = AppState::new(cfg, store);

    // Optional file channel: watch a directory of Claude Code sessions and feed
    // the SAME engine via the normalized format. Held in `_watcher` so it lives
    // for the program's lifetime.
    let _watcher: Option<drifterr_adapters::RecommendedWatcher> =
        std::env::var("DRIFTERR_WATCH_DIR")
            .ok()
            .filter(|d| !d.is_empty())
            .and_then(|dir| start_file_channel(&dir, state.clone()));

    if let Err(e) = serve(proxy_addr, control_addr, state).await {
        eprintln!("drifterr: server error: {e}");
        std::process::exit(1);
    }
}

/// Start watching `dir` for Claude Code sessions, ingesting each into the shared
/// engine state. Returns the live watcher (keep it alive).
fn start_file_channel(dir: &str, state: AppState) -> Option<drifterr_adapters::RecommendedWatcher> {
    use std::path::Path;
    let path = Path::new(dir);

    // Initial scan so existing sessions show up immediately.
    for (_, conv) in drifterr_adapters::claude_code::scan_dir(path) {
        if let Ok(mut core) = state.core.lock() {
            core.record_conversation(&conv);
        }
    }

    let ingest_state = state.clone();
    let ingest = move |file: std::path::PathBuf| {
        let Ok(content) = std::fs::read_to_string(&file) else {
            return;
        };
        let stem = file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("session");
        if let Some(conv) = drifterr_adapters::claude_code::parse_session(&content, stem) {
            if let Ok(mut core) = ingest_state.core.lock() {
                core.record_conversation(&conv);
            }
        }
    };

    match drifterr_adapters::watch_dir(path, ingest) {
        Ok(w) => {
            eprintln!("drifterr files    → watching {dir}  (Claude Code sessions)");
            Some(w)
        }
        Err(e) => {
            eprintln!("drifterr: could not watch {dir}: {e}");
            None
        }
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}
