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

/// Characters per token, for estimating tool-payload size. Matches the heuristic the
/// tokenizer applies to prose; tool payloads are mostly JSON and code, which tokenize a
/// little denser, so this errs slightly low rather than alarming.
const CHARS_PER_TOKEN: usize = 4;

/// Parse one Claude Code JSONL session into a [`Conversation`], or `None` if it
/// contains no usable turns.
pub fn parse_session(content: &str, file_stem: &str) -> Option<Conversation> {
    let mut model: Option<String> = None;
    let mut session_id: Option<String> = None;
    let mut tool_call_count = 0usize;
    let mut raw: Vec<(Role, String)> = Vec::new();
    // Characters of tool payload across the session. Occupies the context window but is
    // never fed to the engine as text — see `Extracted::tool_chars`.
    let mut tool_chars = 0usize;

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
        let got = content_val.map(extract).unwrap_or_default();
        tool_call_count += got.tools;
        // Accumulate the payload even when the message carries no prose — a turn that is
        // *only* a tool result still fills the window, and skipping it here is what made
        // the estimate ~19× too low on tool-heavy sessions.
        tool_chars += got.tool_chars;
        if got.text.trim().is_empty() {
            continue;
        }
        raw.push((role, got.text));
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

    // Tool payloads occupy the window too. Estimated with the same ~4-chars-per-token
    // rule the tokenizer uses for prose — coarse, but far closer than the zero this
    // previously contributed.
    let accumulated = used + tool_chars / CHARS_PER_TOKEN;
    let window = context_window(&model);

    // A transcript holding more conversation than the model's window can only mean the
    // context was compacted at some point, and nothing in the file marks where. So the
    // sum stops being occupancy and becomes "how much was ever said" — see
    // `ContextState::occupancy_known`. Report the honest lower bound and flag it, rather
    // than a 100% that would fire the hard signal on a healthy session.
    let occupancy_known = accumulated <= window;
    let used_tokens = accumulated.min(window);

    Some(Conversation {
        session_id,
        context: ContextState {
            window_size: window,
            used_tokens,
            // Still an estimate, and still says so. The transcript's `usage` records look
            // like they would make this exact but are cumulative billing counters — see
            // `usage_input_sum` for the measurements. Reserved for the proxy.
            exact: false,
            occupancy_known,
            tool_call_count,
        },
        model,
        turns,
        source: Source::File,
    })
}

/// Sum of a `message.usage` record's input components — **not** context occupancy.
///
/// # A documented negative result: this cannot make the file channel exact
///
/// Saturation is the hard signal most worth trusting, and on this channel it is an
/// *estimate*. Making it exact looks trivially possible, because Claude Code writes the
/// provider's `usage` onto assistant messages with `input_tokens`,
/// `cache_read_input_tokens`, `cache_creation_input_tokens` and `output_tokens`. Summing
/// the input components looks exactly like "the prompt the model was given".
///
/// **It is not, and using it as such produces a confidently wrong number.** Measured
/// against a real 178-turn transcript on a 200k-window model:
///
/// * the sum grows monotonically to **679,390** — 3.4× the entire context window;
/// * **676 of 851** usage records exceed the window;
/// * a single record reports `cache_creation_input_tokens: 665,834`, which cannot
///   describe one request to a 200k-window model.
///
/// These are **cumulative billing counters**, accumulated across every request in an
/// assistant turn's tool-use loop (that session made 1,167 tool calls). They are the
/// right numbers for costing a session and the wrong ones for measuring occupancy, and
/// the per-request prompt size cannot be recovered from them: `cache_read` blends an
/// unknown number of re-reads of an unknown prompt.
///
/// Shipping this would have reported ~100% saturation on a healthy session — a **false
/// RED on a hard signal**, which `CLAUDE.md` names as the one unforgivable failure. So
/// the channel keeps estimating and keeps saying so, and `ContextState.exact` stays
/// reserved for the proxy, where the real `messages` array is visible.
///
/// This function is retained, unused by the parser, purely so the finding has a home and
/// a test (`usage_counters_are_cumulative_not_occupancy`). Delete both together if
/// Claude Code ever starts writing per-request usage.
#[cfg(test)]
fn usage_input_sum(usage: &Value) -> Option<usize> {
    let field = |k: &str| usage.get(k).and_then(Value::as_u64).unwrap_or(0) as usize;
    let input = field("input_tokens");
    let cache_read = field("cache_read_input_tokens");
    let cache_creation = field("cache_creation_input_tokens");
    if input == 0 && cache_read == 0 && cache_creation == 0 {
        return None;
    }
    Some(input + cache_read + cache_creation + field("output_tokens"))
}

/// Extract `(text, tool_block_count)` from a message `content` value, which may
/// be a plain string or an array of typed blocks (text / tool_use / tool_result).
fn extract(content: &Value) -> Extracted {
    match content {
        Value::String(s) => Extracted {
            text: s.clone(),
            tools: 0,
            tool_chars: 0,
        },
        Value::Array(blocks) => {
            let mut text_parts = Vec::new();
            let mut tools = 0;
            let mut tool_chars = 0usize;
            for b in blocks {
                match b.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(t) = b.get("text").and_then(Value::as_str) {
                            text_parts.push(t.to_string());
                        }
                    }
                    Some("tool_use") | Some("tool_result") => {
                        tools += 1;
                        // Measure the payload, don't just count the block. A tool
                        // result is often the largest thing in a turn (a file read, a
                        // test log), and it occupies the context window exactly like
                        // prose does — see `Extracted::tool_chars`.
                        tool_chars += tool_payload_len(b);
                    }
                    _ => {}
                }
            }
            Extracted {
                text: text_parts.join("\n"),
                tools,
                tool_chars,
            }
        }
        _ => Extracted::default(),
    }
}

/// One message's content, split into what the engine reads and what merely occupies the
/// window.
#[derive(Default)]
struct Extracted {
    /// Prose the engine analyses for drift.
    text: String,
    /// Number of tool_use / tool_result blocks.
    tools: usize,
    /// Characters of tool payload.
    ///
    /// # Why this is counted separately
    ///
    /// Tool payloads were previously discarded: `extract` counted the blocks and threw
    /// the content away, so the saturation estimate only ever saw prose. In an agentic
    /// session that is where nearly all the tokens are — a file read, a diff, a test log
    /// — and the measured effect on a real 178-turn transcript with 1,167 tool calls was
    /// an estimate **~19× too low**, reporting a nearly-full context as almost empty.
    ///
    /// Saturation is a hard signal allowed to drive RED, so under-counting it this badly
    /// meant the most useful signal on the default channel was effectively switched off.
    ///
    /// They are kept out of `text` on purpose: the engine's constraint rules must not
    /// match on a tool result. A file read that happens to contain `console.log` is not
    /// the assistant writing `console.log`, and conflating the two would manufacture
    /// violations out of the agent merely *looking* at code.
    tool_chars: usize,
}

/// Length of a tool block's payload, in characters.
///
/// Serializes the input/content field rather than guessing at its shape, since a
/// `tool_use` input is arbitrary JSON and a `tool_result` content may be a string or an
/// array of blocks.
fn tool_payload_len(block: &Value) -> usize {
    let payload = block
        .get("input")
        .or_else(|| block.get("content"))
        .or_else(|| block.get("output"));
    match payload {
        Some(Value::String(s)) => s.len(),
        Some(v) => serde_json::to_string(v).map(|s| s.len()).unwrap_or(0),
        None => 0,
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

    /// A documented negative result, pinned so nobody re-attempts it and ships the bug.
    ///
    /// Summing a transcript's usage input components looks exactly like "the prompt the
    /// model was given". It is not — they are cumulative billing counters accumulated
    /// across every request in a tool-use loop, and treating them as occupancy reports a
    /// healthy session as 100% full: a false RED on a hard signal.
    #[test]
    fn usage_counters_are_cumulative_not_occupancy() {
        // Shape and magnitudes taken from a real 178-turn transcript on a 200k-window
        // model. 676808 cached "prompt" tokens cannot describe one request.
        let late = serde_json::json!({
            "input_tokens": 2,
            "cache_read_input_tokens": 676808,
            "cache_creation_input_tokens": 658,
            "output_tokens": 1018
        });
        let sum = usage_input_sum(&late).unwrap();
        assert!(
            sum > 3 * 200_000,
            "the counters exceed the window several times over ({sum}) — they are not \
             occupancy, and anything treating them as such is wrong"
        );

        // And the parser must NOT be using them: exactness stays reserved for the proxy.
        let jsonl = concat!(
            r#"{"type":"user","sessionId":"u-1","message":{"role":"user","content":"hi"}}"#,
            "\n",
            r#"{"type":"assistant","message":{"role":"assistant","model":"claude-opus-4-x","content":[{"type":"text","text":"ok"}],"usage":{"input_tokens":2,"cache_read_input_tokens":676808,"cache_creation_input_tokens":658,"output_tokens":1018}}}"#
        );
        let c = parse_session(jsonl, "x").unwrap();
        assert!(
            !c.context.exact,
            "the file channel must keep saying 'estimated'"
        );
        assert!(
            c.saturation_ratio() < 1.0,
            "a short session must not read as a full context: {}",
            c.saturation_ratio()
        );
    }

    #[test]
    fn tool_payloads_count_toward_the_context_estimate() {
        // The bug this fixes: tool blocks were counted but their content discarded, so
        // in an agentic session — where nearly all the tokens are file reads and test
        // logs — the estimate saw only prose.
        let big = "x".repeat(8000);
        let jsonl = format!(
            concat!(
                r#"{{"type":"user","sessionId":"t-1","message":{{"role":"user","content":"read the file"}}}}"#,
                "\n",
                r#"{{"type":"assistant","message":{{"role":"assistant","model":"m","content":[{{"type":"text","text":"ok"}},{{"type":"tool_use","name":"Read","input":{{"file":"a.ts"}}}}]}}}}"#,
                "\n",
                r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","content":"{}"}}]}}}}"#
            ),
            big
        );
        let c = parse_session(&jsonl, "x").unwrap();
        assert_eq!(c.context.tool_call_count, 2);
        // ~8000 chars of tool payload ⇒ ~2000 tokens, which must dominate the handful of
        // prose tokens rather than being dropped.
        assert!(
            c.context.used_tokens > 1500,
            "tool payload must be counted, got {}",
            c.context.used_tokens
        );
    }

    #[test]
    fn a_long_transcript_reports_unknown_occupancy_not_a_full_window() {
        // The whole point of the investigation behind this module. A transcript longer
        // than the window means the context was compacted and the file cannot say where,
        // so the sum stops being occupancy. Reporting it as ~100% would fire the hard
        // signal on a healthy session; reporting text-only (the old behaviour) reported
        // 8% for a context that was actually full. Neither is honest.
        let chunk = "z".repeat(40_000);
        let mut jsonl = String::from(
            r#"{"type":"user","sessionId":"long-1","message":{"role":"user","content":"go"}}"#,
        );
        // ~30 × 40k chars of tool payload ⇒ ~300k tokens, well past a 200k window.
        for _ in 0..30 {
            jsonl.push('\n');
            jsonl.push_str(&format!(
                r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","content":"{chunk}"}}]}}}}"#
            ));
        }
        let c = parse_session(&jsonl, "x").unwrap();
        assert!(
            !c.context.occupancy_known,
            "a transcript past the window must admit occupancy is unknown"
        );
        assert_eq!(
            c.context.used_tokens, c.context.window_size,
            "reported as a lower bound at the window, never above it"
        );

        // And the hard signal must abstain from RED on it.
        let ev = drifterr_engine::signals::saturation::evaluate(&c);
        assert_ne!(
            ev.state,
            drifterr_engine::signals::State::Red,
            "unknown occupancy must not drive RED: {}",
            ev.evidence.detail
        );
        assert!(
            ev.evidence.detail.contains("unknown"),
            "and it must say why: {}",
            ev.evidence.detail
        );
    }

    #[test]
    fn a_short_transcript_keeps_known_occupancy() {
        let c = parse_session(SAMPLE, "x").unwrap();
        assert!(
            c.context.occupancy_known,
            "a session that fits in the window measures the right thing"
        );
    }

    #[test]
    fn a_tool_only_turn_still_fills_the_window() {
        // A message with no prose used to contribute nothing at all.
        let big = "y".repeat(4000);
        let jsonl = format!(
            concat!(
                r#"{{"type":"user","sessionId":"t-2","message":{{"role":"user","content":"go"}}}}"#,
                "\n",
                r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","content":"{}"}}]}}}}"#
            ),
            big
        );
        let c = parse_session(&jsonl, "x").unwrap();
        assert!(
            c.context.used_tokens > 900,
            "a prose-free tool turn must still occupy the window, got {}",
            c.context.used_tokens
        );
    }

    #[test]
    fn tool_payloads_never_reach_the_engine_as_text() {
        // Critical for precision: a file read containing `console.log` is the agent
        // *looking* at code, not writing it. Feeding tool results to the constraint
        // rules would manufacture violations out of reading.
        let jsonl = concat!(
            r#"{"type":"user","sessionId":"t-3","message":{"role":"user","content":"no console.log please"}}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"file.ts:12: console.log('debug')"}]}}"#
        );
        let c = parse_session(jsonl, "x").unwrap();
        for t in &c.turns {
            assert!(
                !t.content.contains("console.log('debug')"),
                "tool payload leaked into an analysed turn: {}",
                t.content
            );
        }
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
