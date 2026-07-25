//! Claude Code session adapter.
//!
//! Claude Code writes each session to a JSONL file (one JSON event per line),
//! typically under `~/.claude/projects/<project>/<session-id>.jsonl`. This adapter
//! reconstructs the normalized [`Conversation`] from such a file — best-effort
//! and tolerant of schema drift: unknown lines are skipped, never panicking.
//!
//! Tokens are **estimated** (the file never carries the wire payload), so
//! `exact = false` — we never claim proxy-grade precision here.

use drifterr_engine::conversation::{ContextState, Conversation, Role, Source, Turn};
use drifterr_tokenizer::{context_window, HeuristicTokenizer, Tokenizer};
use serde_json::Value;
use std::path::Path;

/// Parse one Claude Code JSONL session into a [`Conversation`], or `None` if it
/// contains no usable turns.
pub fn parse_session(content: &str, file_stem: &str) -> Option<Conversation> {
    let mut model: Option<String> = None;
    let mut session_id: Option<String> = None;
    let mut tool_call_count = 0usize;
    let mut raw: Vec<(Role, String)> = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };

        if session_id.is_none() {
            if let Some(s) = v.get("sessionId").and_then(Value::as_str) {
                session_id = Some(s.to_string());
            }
        }

        let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
        let role = match kind {
            "user" => Role::User,
            "assistant" => Role::Assistant,
            // Claude Code also writes "summary", "system", tool events, etc.
            _ => continue,
        };

        // Capture the model from any assistant message.
        if model.is_none() {
            if let Some(m) = v.pointer("/message/model").and_then(Value::as_str) {
                model = Some(m.to_string());
            }
        }

        // content lives at message.content (string or array of blocks); fall back
        // to a top-level content field.
        let content_val = v.pointer("/message/content").or_else(|| v.get("content"));
        let (text, tools) = content_val.map(extract).unwrap_or_default();
        tool_call_count += tools;
        if text.trim().is_empty() {
            continue;
        }
        raw.push((role, text));
    }

    if raw.is_empty() {
        return None;
    }

    let model = model.unwrap_or_else(|| "claude-sonnet-4".to_string());
    let session_id = session_id.unwrap_or_else(|| format!("file-{file_stem}"));
    let tok = HeuristicTokenizer::for_model(&model);

    let mut turns = Vec::with_capacity(raw.len());
    let mut used = 0usize;
    for (index, (role, content)) in raw.into_iter().enumerate() {
        let tokens = tok.count(&content);
        used += tokens;
        turns.push(Turn {
            index,
            role,
            content,
            tokens,
            timestamp: 0,
        });
    }

    Some(Conversation {
        session_id,
        context: ContextState {
            window_size: context_window(&model),
            used_tokens: used,
            exact: false,
            tool_call_count,
        },
        model,
        turns,
        source: Source::File,
    })
}

/// Extract `(text, tool_block_count)` from a message `content` value, which may
/// be a plain string or an array of typed blocks (text / tool_use / tool_result).
fn extract(content: &Value) -> (String, usize) {
    match content {
        Value::String(s) => (s.clone(), 0),
        Value::Array(blocks) => {
            let mut text_parts = Vec::new();
            let mut tools = 0;
            for b in blocks {
                match b.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(t) = b.get("text").and_then(Value::as_str) {
                            text_parts.push(t.to_string());
                        }
                    }
                    Some("tool_use") | Some("tool_result") => tools += 1,
                    _ => {}
                }
            }
            (text_parts.join("\n"), tools)
        }
        _ => (String::new(), 0),
    }
}

/// Parse every `*.jsonl` session file in `dir` (non-recursive). Returns the
/// path + reconstructed conversation for each file that yields turns.
pub fn scan_dir(dir: &Path) -> Vec<(std::path::PathBuf, Conversation)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&path) {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("session");
            if let Some(conv) = parse_session(&content, stem) {
                out.push((path, conv));
            }
        }
    }
    out
}

/// The project directory a session was run from, if the transcript records one.
///
/// Claude Code writes a `cwd` on its message events. That is the only reliable
/// pointer from a session file back to the repo it belongs to, which is what lets
/// Drifterr find *that* project's rules file (`CLAUDE.md`, `.cursor/rules`) rather
/// than guessing from the app's own working directory — the desktop app is
/// launched from Applications, so its cwd means nothing.
///
/// Best-effort like the rest of this adapter: unknown or malformed lines are
/// skipped, and a missing `cwd` simply means "no project known".
pub fn session_cwd(content: &str) -> Option<String> {
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(cwd) = v.get("cwd").and_then(Value::as_str) {
            if !cwd.is_empty() {
                return Some(cwd.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
{"type":"summary","summary":"ignore me"}
{"type":"user","sessionId":"abc-123","message":{"role":"user","content":"Refactor auth in TS, no JS"},"timestamp":"t1"}
{"type":"assistant","message":{"role":"assistant","model":"claude-opus-4-x","content":[{"type":"text","text":"Sure, creating auth.ts"}]},"timestamp":"t2"}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","name":"edit","input":{}}]}}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"ok"}]}}
not json at all
"#;

    #[test]
    fn parses_turns_model_and_session() {
        let c = parse_session(SAMPLE, "file-stem").unwrap();
        assert_eq!(c.session_id, "abc-123");
        assert_eq!(c.model, "claude-opus-4-x");
        assert_eq!(c.source, Source::File);
        assert!(!c.context.exact, "file channel is always estimated");
        assert_eq!(c.context.window_size, 200_000);
        // user + assistant(text) = 2 textual turns; tool-only blocks contribute
        // no text turn but bump the tool count.
        assert_eq!(c.turns.len(), 2);
        assert_eq!(c.turns[0].role, Role::User);
        assert_eq!(c.turns[1].role, Role::Assistant);
        assert_eq!(c.context.tool_call_count, 2);
        assert!(c.context.used_tokens > 0);
    }

    #[test]
    fn session_id_falls_back_to_stem() {
        let jsonl = r#"{"type":"user","message":{"role":"user","content":"hi there"}}"#;
        let c = parse_session(jsonl, "my-session").unwrap();
        assert_eq!(c.session_id, "file-my-session");
    }

    #[test]
    fn empty_or_garbage_yields_none() {
        assert!(parse_session("", "x").is_none());
        assert!(parse_session("garbage\n{not json}", "x").is_none());
    }

    #[test]
    fn scan_dir_reads_jsonl_files() {
        let dir = std::env::temp_dir().join(format!("drifterr-scan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("a.jsonl"),
            r#"{"type":"user","message":{"role":"user","content":"first session"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("b.jsonl"),
            r#"{"type":"user","message":{"role":"user","content":"second session"}}"#,
        )
        .unwrap();
        std::fs::write(dir.join("ignore.txt"), "not a session").unwrap();

        let found = scan_dir(&dir);
        assert_eq!(found.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_cwd_finds_the_project_directory() {
        let jsonl = concat!(
            r#"{"type":"summary","summary":"whatever"}"#,
            "\n",
            r#"{"type":"user","cwd":"/Users/x/code/drifterr","message":{"role":"user","content":"hi"}}"#,
            "\n"
        );
        assert_eq!(
            session_cwd(jsonl).as_deref(),
            Some("/Users/x/code/drifterr")
        );
    }

    #[test]
    fn session_cwd_is_none_when_absent_or_unusable() {
        assert!(session_cwd("").is_none());
        assert!(session_cwd(r#"{"type":"user","message":{"content":"hi"}}"#).is_none());
        // Malformed lines are skipped, never fatal.
        assert!(session_cwd("{not json}\ngarbage").is_none());
        // An empty cwd is not a project.
        assert!(session_cwd(r#"{"type":"user","cwd":""}"#).is_none());
    }
}
