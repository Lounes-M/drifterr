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
//! Configuration via environment:
//! * `DRIFTERR_PROXY_ADDR`     (default `127.0.0.1:8787`)
//! * `DRIFTERR_CONTROL_ADDR`   (default `127.0.0.1:8788`)
//! * `DRIFTERR_DB`             SQLite path (default: in-memory, not persisted)
//! * `OPENAI_UPSTREAM`         (default `https://api.openai.com`)
//! * `ANTHROPIC_UPSTREAM`      (default `https://api.anthropic.com`)

use drifterr_proxy::{serve, AppState, ProxyConfig};
use drifterr_store::Store;
use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    let proxy_addr: SocketAddr = env_or("DRIFTERR_PROXY_ADDR", "127.0.0.1:8787")
        .parse()
        .expect("invalid DRIFTERR_PROXY_ADDR");
    let control_addr: SocketAddr = env_or("DRIFTERR_CONTROL_ADDR", "127.0.0.1:8788")
        .parse()
        .expect("invalid DRIFTERR_CONTROL_ADDR");

    let cfg = ProxyConfig {
        openai_upstream: env_or("OPENAI_UPSTREAM", "https://api.openai.com"),
        anthropic_upstream: env_or("ANTHROPIC_UPSTREAM", "https://api.anthropic.com"),
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

    let state = AppState::new(cfg, store);

    eprintln!("drifterr proxy   → {proxy_addr}  (point your tool here)");
    eprintln!("drifterr control → http://{control_addr}/status");

    if let Err(e) = serve(proxy_addr, control_addr, state).await {
        eprintln!("drifterr: server error: {e}");
        std::process::exit(1);
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}
