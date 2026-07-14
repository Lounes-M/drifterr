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
//! * `DRIFTERR_PROVIDER`       preset (default openrouter): openai, anthropic, gemini, groq, mistral, deepseek, xai, together
//! * `OPENAI_UPSTREAM`         explicit OpenAI-schema URL (overrides the preset)
//! * `ANTHROPIC_UPSTREAM`      (default `https://api.anthropic.com`)

use drifterr_proxy::{serve, AppState, ProxyConfig};
use drifterr_store::Store;
use std::net::SocketAddr;

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

    // Select the upstream provider from the environment (DRIFTERR_PROVIDER
    // preset, or explicit OPENAI_UPSTREAM / ANTHROPIC_UPSTREAM URLs).
    let cfg = ProxyConfig::from_env();

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
    eprintln!(
        "drifterr upstream → {} ({})",
        cfg.openai_upstream,
        cfg.provider_label()
    );

    let state = AppState::new(cfg, store);
    if state.auto_reanchor_on() {
        eprintln!("drifterr re-anchor → ON (injects the preamble into drifting requests)");
    }

    // File channel: watch a directory of Claude Code sessions and feed the SAME
    // engine via the normalized format. Zero-config by default (~/.claude/projects);
    // DRIFTERR_WATCH_DIR overrides the location. Held in `_watcher` so it lives for
    // the program's lifetime.
    let watch_dir = std::env::var("DRIFTERR_WATCH_DIR")
        .ok()
        .filter(|d| !d.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(drifterr_proxy::default_claude_projects_dir);
    let _watcher = watch_dir.filter(|p| p.is_dir()).and_then(|dir| {
        let w = drifterr_proxy::watch_claude_sessions(&dir, state.clone());
        if w.is_some() {
            state.set_watching_files(true);
            eprintln!(
                "drifterr files    → watching {}  (Claude Code sessions)",
                dir.display()
            );
        }
        w
    });

    if let Err(e) = serve(proxy_addr, control_addr, state).await {
        eprintln!("drifterr: server error: {e}");
        std::process::exit(1);
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}
