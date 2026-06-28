//! Per-provider parsing of the two API schemas we relay: OpenAI-style
//! (`/chat/completions`) and Anthropic-style (`/messages`).
//!
//! The proxy reads the *request* to recover the conversation so far, and reads
//! the *response* (streaming SSE or a single JSON body) to recover the new
//! assistant turn plus exact token usage. Both schemas normalize into the same
//! [`ParsedRequest`] / [`ParsedResponse`], so the rest of the proxy is
//! schema-agnostic — mirroring how the engine is channel-agnostic.
//!
//! All parsing is best-effort and never panics: a body we cannot understand
//! yields empty results, and the relay still happens. We must never break a
//! user's request because our detection could not read it.

use drifterr_engine::conversation::{Role, Turn};
use drifterr_tokenizer::{HeuristicTokenizer, Tokenizer};
use serde_json::Value;

/// Which API schema a path speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    OpenAI,
    Anthropic,
}

impl Provider {
    /// Infer the schema from the request path. Anthropic uses `/messages`;
    /// everything else is treated as OpenAI-style (`/chat/completions`,
    /// OpenRouter, local servers, …), which is the dominant convention.
    pub fn from_path(path: &str) -> Provider {
        if path.contains("/messages") {
            Provider::Anthropic
        } else {
            Provider::OpenAI
        }
    }
}

/// The conversation history recovered from a request body.
#[derive(Debug, Clone)]
pub struct ParsedRequest {
    pub model: String,
    /// Prior turns (user / assistant / tool / system→user). Does NOT include the
    /// response we are about to receive.
    pub turns: Vec<Turn>,
    pub stream: bool,
    pub tool_call_count: usize,
}

/// The new assistant turn recovered from a response body.
#[derive(Debug, Clone, Default)]
pub struct ParsedResponse {
    pub assistant_text: String,
    /// Exact prompt tokens reported by the provider, if present.
    pub input_tokens: Option<usize>,
    /// Exact completion tokens reported by the provider, if present.
    pub output_tokens: Option<usize>,
}

impl ParsedResponse {
    /// Whether the provider gave us exact token usage for both sides.
    pub fn has_exact_usage(&self) -> bool {
        self.input_tokens.is_some() && self.output_tokens.is_some()
    }
}

/// Concatenate the textual content of a message's `content` field, which may be
/// a plain string or an array of typed blocks (multimodal / tool blocks). Only
/// text is kept; images and other parts are ignored for detection purposes.
fn extract_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Map a wire role to the normalized [`Role`]. System messages carry user
/// instructions for our purposes (baseline), so they fold into `User`.
fn map_role(role: &str) -> Role {
    match role {
        "assistant" => Role::Assistant,
        "tool" | "function" => Role::Tool,
        _ => Role::User, // "user", "system", anything unknown
    }
}

/// Parse a request body into the normalized conversation history.
pub fn parse_request(provider: Provider, body: &[u8]) -> ParsedRequest {
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let model = v
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let stream = v.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let tok = HeuristicTokenizer::for_model(&model);

    let mut turns = Vec::new();
    let mut tool_call_count = 0usize;

    // Anthropic carries the system prompt at the top level, not in `messages`.
    if provider == Provider::Anthropic {
        if let Some(system) = v.get("system") {
            let text = extract_text(system);
            if !text.is_empty() {
                push_turn(&mut turns, Role::User, text, &tok);
            }
        }
    }

    if let Some(messages) = v.get("messages").and_then(Value::as_array) {
        for m in messages {
            let role = map_role(m.get("role").and_then(Value::as_str).unwrap_or("user"));
            let text = m.get("content").map(extract_text).unwrap_or_default();
            // Count tool calls: OpenAI assistant messages carry a `tool_calls`
            // array; tool-role messages are themselves tool results.
            if let Some(calls) = m.get("tool_calls").and_then(Value::as_array) {
                tool_call_count += calls.len();
            }
            if role == Role::Tool {
                tool_call_count += 1;
            }
            push_turn(&mut turns, role, text, &tok);
        }
    }

    ParsedRequest {
        model,
        turns,
        stream,
        tool_call_count,
    }
}

fn push_turn(turns: &mut Vec<Turn>, role: Role, content: String, tok: &HeuristicTokenizer) {
    let index = turns.len();
    let tokens = tok.count(&content);
    turns.push(Turn {
        index,
        role,
        content,
        tokens,
        timestamp: now_millis(),
    });
}

/// Parse a response body. `content_type` decides streaming (SSE) vs single JSON.
pub fn parse_response(provider: Provider, content_type: &str, body: &[u8]) -> ParsedResponse {
    if content_type.contains("text/event-stream") {
        parse_sse(provider, body)
    } else {
        parse_json_response(provider, body)
    }
}

/// Parse a non-streaming JSON response.
fn parse_json_response(provider: Provider, body: &[u8]) -> ParsedResponse {
    let v: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return ParsedResponse::default(),
    };
    match provider {
        Provider::OpenAI => ParsedResponse {
            assistant_text: v
                .pointer("/choices/0/message/content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            input_tokens: usize_at(&v, "/usage/prompt_tokens"),
            output_tokens: usize_at(&v, "/usage/completion_tokens"),
        },
        Provider::Anthropic => {
            let text = v
                .get("content")
                .and_then(Value::as_array)
                .map(|blocks| {
                    blocks
                        .iter()
                        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                        .filter_map(|b| b.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default();
            ParsedResponse {
                assistant_text: text,
                input_tokens: usize_at(&v, "/usage/input_tokens"),
                output_tokens: usize_at(&v, "/usage/output_tokens"),
            }
        }
    }
}

/// Parse a streaming SSE body by accumulating the per-chunk deltas. We parse the
/// *buffered copy* produced by the tee — never the live stream — so this runs
/// off the client's response path.
fn parse_sse(provider: Provider, body: &[u8]) -> ParsedResponse {
    let text = String::from_utf8_lossy(body);
    let mut out = ParsedResponse::default();

    for line in text.lines() {
        let line = line.trim_start();
        let Some(payload) = line.strip_prefix("data:") else {
            continue;
        };
        let payload = payload.trim();
        if payload.is_empty() || payload == "[DONE]" {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(payload) else {
            continue;
        };

        match provider {
            Provider::OpenAI => {
                if let Some(delta) = v
                    .pointer("/choices/0/delta/content")
                    .and_then(Value::as_str)
                {
                    out.assistant_text.push_str(delta);
                }
                // Usage arrives in a trailing chunk when stream_options asks for
                // it; capture whenever present.
                if let Some(n) = usize_at(&v, "/usage/prompt_tokens") {
                    out.input_tokens = Some(n);
                }
                if let Some(n) = usize_at(&v, "/usage/completion_tokens") {
                    out.output_tokens = Some(n);
                }
            }
            Provider::Anthropic => match v.get("type").and_then(Value::as_str) {
                Some("message_start") => {
                    out.input_tokens = usize_at(&v, "/message/usage/input_tokens");
                }
                Some("content_block_delta") => {
                    if let Some(t) = v.pointer("/delta/text").and_then(Value::as_str) {
                        out.assistant_text.push_str(t);
                    }
                }
                Some("message_delta") => {
                    if let Some(n) = usize_at(&v, "/usage/output_tokens") {
                        out.output_tokens = Some(n);
                    }
                }
                _ => {}
            },
        }
    }
    out
}

fn usize_at(v: &Value, pointer: &str) -> Option<usize> {
    v.pointer(pointer)
        .and_then(Value::as_u64)
        .map(|n| n as usize)
}

/// Marker so we never inject the re-anchor preamble twice into one request.
pub const REANCHOR_MARKER: &str = "[Drifterr re-anchor]";

/// Inject the re-anchor `preamble` into an outgoing request body (opt-in
/// auto-re-anchor). For OpenAI it's prepended as a leading `system` message; for
/// Anthropic it's prepended to the top-level `system`. Returns the modified body,
/// or `None` if the body can't be parsed or the preamble is already present
/// (idempotent). Best-effort — a `None` means "relay the original unchanged".
pub fn inject_preamble(provider: Provider, body: &[u8], preamble: &str) -> Option<Vec<u8>> {
    let mut v: Value = serde_json::from_slice(body).ok()?;
    if !v.is_object() {
        return None;
    }
    if body_contains_marker(&v) {
        return None;
    }
    match provider {
        Provider::OpenAI => {
            let msgs = v.get_mut("messages")?.as_array_mut()?;
            msgs.insert(
                0,
                serde_json::json!({ "role": "system", "content": preamble }),
            );
        }
        Provider::Anthropic => match v.get_mut("system") {
            Some(Value::String(s)) => *s = format!("{preamble}\n\n{s}"),
            Some(Value::Array(arr)) => {
                arr.insert(0, serde_json::json!({ "type": "text", "text": preamble }))
            }
            _ => {
                v["system"] = Value::String(preamble.to_string());
            }
        },
    }
    serde_json::to_vec(&v).ok()
}

fn body_contains_marker(v: &Value) -> bool {
    serde_json::to_string(v)
        .map(|s| s.contains(REANCHOR_MARKER))
        .unwrap_or(false)
}

/// Wall-clock milliseconds. Unlike workflow scripts, a real binary may read the
/// clock; turns carry a true timestamp.
fn now_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inject_preamble_openai_and_idempotent() {
        let body = br#"{"model":"gpt-4o","messages":[{"role":"user","content":"hi"}]}"#;
        let out =
            inject_preamble(Provider::OpenAI, body, "[Drifterr re-anchor] keep to TS").unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        let msgs = v["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "system");
        assert!(msgs[0]["content"].as_str().unwrap().contains("re-anchor"));
        // Second injection is a no-op (marker already present).
        assert!(inject_preamble(Provider::OpenAI, &out, "[Drifterr re-anchor] x").is_none());
    }

    #[test]
    fn inject_preamble_anthropic_prepends_system() {
        let body = br#"{"model":"claude-opus-4-x","system":"be nice","messages":[{"role":"user","content":"hi"}]}"#;
        let out = inject_preamble(Provider::Anthropic, body, "[Drifterr re-anchor] no JS").unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        let sys = v["system"].as_str().unwrap();
        assert!(sys.starts_with("[Drifterr re-anchor]"));
        assert!(sys.contains("be nice"));
    }

    #[test]
    fn inject_preamble_handles_garbage() {
        assert!(inject_preamble(Provider::OpenAI, b"not json", "x").is_none());
    }

    #[test]
    fn openai_request_history_and_stream_flag() {
        let body = br#"{"model":"gpt-4o","stream":true,"messages":[
            {"role":"system","content":"be concise"},
            {"role":"user","content":"refactor in TS, no JS"},
            {"role":"assistant","content":"ok"}
        ]}"#;
        let r = parse_request(Provider::OpenAI, body);
        assert_eq!(r.model, "gpt-4o");
        assert!(r.stream);
        assert_eq!(r.turns.len(), 3);
        assert_eq!(r.turns[0].role, Role::User); // system folded to user
        assert_eq!(r.turns[2].role, Role::Assistant);
    }

    #[test]
    fn anthropic_request_lifts_system_prompt() {
        let body = br#"{"model":"claude-opus-4-x","system":"no comments",
            "messages":[{"role":"user","content":"write slugify"}]}"#;
        let r = parse_request(Provider::Anthropic, body);
        assert_eq!(r.turns.len(), 2);
        assert_eq!(r.turns[0].content, "no comments");
    }

    #[test]
    fn openai_sse_accumulates_text_and_usage() {
        let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"Hello \"}}]}\n\n\
                   data: {\"choices\":[{\"delta\":{\"content\":\"world\"}}]}\n\n\
                   data: {\"choices\":[{\"delta\":{}}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":2}}\n\n\
                   data: [DONE]\n\n";
        let r = parse_response(Provider::OpenAI, "text/event-stream", sse.as_bytes());
        assert_eq!(r.assistant_text, "Hello world");
        assert_eq!(r.input_tokens, Some(10));
        assert_eq!(r.output_tokens, Some(2));
        assert!(r.has_exact_usage());
    }

    #[test]
    fn anthropic_sse_accumulates_text_and_usage() {
        let sse = "event: message_start\n\
                   data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":12}}}\n\n\
                   event: content_block_delta\n\
                   data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"auth.js\"}}\n\n\
                   event: message_delta\n\
                   data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":5}}\n\n";
        let r = parse_response(Provider::Anthropic, "text/event-stream", sse.as_bytes());
        assert_eq!(r.assistant_text, "auth.js");
        assert_eq!(r.input_tokens, Some(12));
        assert_eq!(r.output_tokens, Some(5));
    }

    #[test]
    fn openai_json_response() {
        let body = br#"{"choices":[{"message":{"content":"app.js created"}}],
            "usage":{"prompt_tokens":20,"completion_tokens":4}}"#;
        let r = parse_response(Provider::OpenAI, "application/json", body);
        assert_eq!(r.assistant_text, "app.js created");
        assert_eq!(r.input_tokens, Some(20));
    }

    #[test]
    fn garbage_never_panics() {
        let r = parse_response(Provider::OpenAI, "text/event-stream", b"not sse at all");
        assert!(r.assistant_text.is_empty());
        let r2 = parse_request(Provider::OpenAI, b"}{ broken");
        assert_eq!(r2.model, "unknown");
    }
}
