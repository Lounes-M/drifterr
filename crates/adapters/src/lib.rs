//! Channel adapters that normalize external sources into the engine's
//! [`Conversation`](drifterr_engine::conversation::Conversation) shape.
//!
//! Today: the Claude Code file watcher. Each adapter only *produces* the
//! normalized format — it never reaches into the engine — so the engine stays
//! channel-agnostic.
//!
//! [`rules_import`] is the other direction of the same idea: it supplies the
//! *baseline* from a file the user already maintains (`CLAUDE.md`,
//! `.cursor/rules`), keeping the filesystem knowledge here and out of the engine.

pub mod claude_code;
pub mod rules_import;

use notify::{RecursiveMode, Watcher};
use std::path::{Path, PathBuf};

/// The live watcher handle callers must keep alive. Re-exported so downstream
/// crates don't need a direct `notify` dependency.
pub use notify::RecommendedWatcher;

/// Watch `dir` for changes to `*.jsonl` session files. On each change, the
/// changed path is handed to `on_change` (run on the watcher's own thread).
///
/// Returns the live watcher — **keep it alive** for as long as you want events;
/// dropping it stops watching. Thin glue over [`notify`]; the parsing it feeds
/// (`claude_code`) is what carries the test coverage.
pub fn watch_dir<F>(dir: &Path, on_change: F) -> notify::Result<RecommendedWatcher>
where
    F: Fn(PathBuf) + Send + 'static,
{
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(event) = res {
            for path in event.paths {
                if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                    on_change(path);
                }
            }
        }
    })?;
    watcher.watch(dir, RecursiveMode::Recursive)?;
    Ok(watcher)
}
