//! The judge — short, binary model calls for the fuzzy checks the deterministic
//! engine can't make on its own (decision coherence, fuzzy constraints).
//!
//! It is **pluggable** and **fail-safe**:
//!
//! * [`Judge::OpenRouter`] asks the user's own provider (OpenRouter by default —
//!   Drifterr standardizes on it) a single yes/no question with a short reason.
//! * [`Judge::Stub`] answers from simple substring rules — for deterministic
//!   tests, with no network.
//! * [`Judge::Disabled`] always answers "no" — the product runs fully without a
//!   judge (the hard + soft signals still work).
//!
//! Fail-safe means: any error, timeout, or unparseable reply yields **no
//! violation**. A judge that cries wolf is worse than one that stays quiet, and
//! it must never break detection.

use serde::Serialize;

pub mod decision;

/// A judge's answer to a binary question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JudgeAnswer {
    pub yes: bool,
    pub reason: String,
}

impl JudgeAnswer {
    fn no() -> Self {
        Self {
            yes: false,
            reason: String::new(),
        }
    }
}

/// A pluggable judge backend.
pub enum Judge {
    /// No judge — always answers "no".
    Disabled,
    /// Deterministic test double: answers "yes" if the context contains any of
    /// the configured substrings (case-insensitive).
    Stub(StubJudge),
    /// Real model calls via an OpenAI-compatible endpoint (OpenRouter default).
    OpenRouter(OpenRouterJudge),
}

impl Judge {
    /// Build from the environment: enabled (OpenRouter) when an API key is
    /// present, otherwise disabled. Keys checked: `OPENROUTER_API_KEY`, then
    /// `OPENAI_API_KEY`. Model via `DRIFTERR_JUDGE_MODEL`, base via
    /// `DRIFTERR_JUDGE_BASE`.
    pub fn from_env() -> Self {
        let key = std::env::var("OPENROUTER_API_KEY")
            .ok()
            .filter(|k| !k.is_empty())
            .or_else(|| {
                std::env::var("OPENAI_API_KEY")
                    .ok()
                    .filter(|k| !k.is_empty())
            });
        match key {
            Some(api_key) => Judge::OpenRouter(OpenRouterJudge::new(
                std::env::var("DRIFTERR_JUDGE_BASE")
                    .unwrap_or_else(|_| "https://openrouter.ai/api".to_string()),
                std::env::var("DRIFTERR_JUDGE_MODEL")
                    .unwrap_or_else(|_| "openai/gpt-4o-mini".to_string()),
                api_key,
            )),
            None => Judge::Disabled,
        }
    }

    pub fn enabled(&self) -> bool {
        !matches!(self, Judge::Disabled)
    }

    /// A short label for diagnostics / the settings view.
    pub fn label(&self) -> String {
        match self {
            Judge::Disabled => "disabled".to_string(),
            Judge::Stub(_) => "stub".to_string(),
            Judge::OpenRouter(j) => j.model.clone(),
        }
    }

    /// Ask a yes/no question about `context`. Never panics; any failure → "no".
    pub async fn check(&self, question: &str, context: &str) -> JudgeAnswer {
        match self {
            Judge::Disabled => JudgeAnswer::no(),
            Judge::Stub(s) => s.check(question, context),
            Judge::OpenRouter(j) => j.check(question, context).await,
        }
    }
}

/// Deterministic test double.
pub struct StubJudge {
    pub yes_if_contains: Vec<String>,
}

impl StubJudge {
    pub fn new(yes_if_contains: &[&str]) -> Self {
        Self {
            yes_if_contains: yes_if_contains.iter().map(|s| s.to_lowercase()).collect(),
        }
    }
    fn check(&self, _question: &str, context: &str) -> JudgeAnswer {
        let lc = context.to_lowercase();
        let hit = self.yes_if_contains.iter().find(|s| lc.contains(*s));
        match hit {
            Some(s) => JudgeAnswer {
                yes: true,
                reason: format!("matched \"{s}\""),
            },
            None => JudgeAnswer::no(),
        }
    }
}

/// Real judge over an OpenAI-compatible chat endpoint.
pub struct OpenRouterJudge {
    client: reqwest::Client,
    base_url: String,
    pub model: String,
    api_key: String,
}

#[derive(Serialize)]
struct ChatBody<'a> {
    model: &'a str,
    messages: Vec<ChatMsg<'a>>,
    temperature: f32,
    max_tokens: u32,
}
#[derive(Serialize)]
struct ChatMsg<'a> {
    role: &'a str,
    content: &'a str,
}

const SYSTEM: &str = "You are a strict reviewer. Answer ONLY with a JSON object \
of the form {\"yes\": <true|false>, \"reason\": \"<short>\"}. Say yes only when \
you are confident; when unsure, say no.";

impl OpenRouterJudge {
    pub fn new(base_url: String, model: String, api_key: String) -> Self {
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap_or_default();
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            model,
            api_key,
        }
    }

    async fn check(&self, question: &str, context: &str) -> JudgeAnswer {
        let user = format!("{question}\n\n---\n{context}");
        let body = ChatBody {
            model: &self.model,
            messages: vec![
                ChatMsg {
                    role: "system",
                    content: SYSTEM,
                },
                ChatMsg {
                    role: "user",
                    content: &user,
                },
            ],
            temperature: 0.0,
            max_tokens: 200,
        };
        let url = format!("{}/v1/chat/completions", self.base_url);
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await;
        let Ok(resp) = resp else {
            return JudgeAnswer::no();
        };
        let Ok(v) = resp.json::<serde_json::Value>().await else {
            return JudgeAnswer::no();
        };
        let content = v
            .pointer("/choices/0/message/content")
            .and_then(|c| c.as_str())
            .unwrap_or("");
        parse_judge_content(content)
    }
}

/// Parse a judge reply into an answer. Tries strict JSON first, then falls back
/// to scanning for an affirmative token. Anything ambiguous → "no" (fail-safe).
pub fn parse_judge_content(content: &str) -> JudgeAnswer {
    // Strict JSON anywhere in the content.
    if let Some(start) = content.find('{') {
        if let Some(end) = content.rfind('}') {
            if end > start {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content[start..=end]) {
                    if let Some(yes) = v.get("yes").and_then(|y| y.as_bool()) {
                        let reason = v
                            .get("reason")
                            .and_then(|r| r.as_str())
                            .unwrap_or("")
                            .to_string();
                        return JudgeAnswer { yes, reason };
                    }
                }
            }
        }
    }
    // Lenient fallback: a leading yes/true.
    let lc = content.trim().to_lowercase();
    let yes = lc.starts_with("yes") || lc.starts_with("true") || lc.contains("\"yes\": true");
    JudgeAnswer {
        yes,
        reason: if yes {
            content.trim().chars().take(120).collect()
        } else {
            String::new()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn disabled_always_no() {
        let j = Judge::Disabled;
        assert!(!j.check("violates?", "anything").await.yes);
        assert!(!j.enabled());
    }

    #[tokio::test]
    async fn stub_matches_substring() {
        let j = Judge::Stub(StubJudge::new(&["bcrypt"]));
        assert!(j.check("reintroduced?", "let's hash with bcrypt").await.yes);
        assert!(!j.check("reintroduced?", "use argon2 only").await.yes);
    }

    #[test]
    fn parse_strict_json() {
        let a = parse_judge_content(r#"{"yes": true, "reason": "uses .js"}"#);
        assert!(a.yes);
        assert_eq!(a.reason, "uses .js");
        let b = parse_judge_content(r#"Sure: {"yes": false, "reason": "fine"}"#);
        assert!(!b.yes);
    }

    #[test]
    fn parse_fallback_and_failsafe() {
        assert!(parse_judge_content("Yes, it does.").yes);
        assert!(!parse_judge_content("No.").yes);
        // Garbage / ambiguous → no.
        assert!(!parse_judge_content("hmm, hard to tell").yes);
        assert!(!parse_judge_content("").yes);
    }
}
