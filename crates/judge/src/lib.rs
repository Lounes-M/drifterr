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

pub mod constraint;
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

    /// Given a list of constraint texts, decide which the `message` violates —
    /// in a **single** batched call. Returns one `bool` per input constraint,
    /// aligned by index. Fail-safe: disabled judge, network error, or an
    /// unparseable reply all yield "nothing violated" (all `false`). Never RED:
    /// the caller emits AMBER, because a fuzzy call must under-claim.
    pub async fn violations(&self, constraints: &[String], message: &str) -> Vec<bool> {
        if constraints.is_empty() {
            return Vec::new();
        }
        match self {
            Judge::Disabled => vec![false; constraints.len()],
            Judge::Stub(s) => s.violations(constraints, message),
            Judge::OpenRouter(j) => j.violations(constraints, message).await,
        }
    }

    /// Extract short, checkable constraint statements the user placed on the
    /// assistant's work from a single user message (one call). Fail-safe:
    /// disabled/error/unparseable → empty. The caller gates this behind a cheap
    /// local cue so we don't spend a call on every turn.
    pub async fn extract_constraints(&self, user_message: &str) -> Vec<String> {
        match self {
            Judge::Disabled => Vec::new(),
            Judge::Stub(s) => s.extract_constraints(),
            Judge::OpenRouter(j) => j.extract_constraints(user_message).await,
        }
    }
}

/// Deterministic test double.
pub struct StubJudge {
    pub yes_if_contains: Vec<String>,
    /// Constraints this stub "extracts" from any user message — for testing the
    /// extraction path without a network.
    pub extracts: Vec<String>,
}

impl StubJudge {
    pub fn new(yes_if_contains: &[&str]) -> Self {
        Self {
            yes_if_contains: yes_if_contains.iter().map(|s| s.to_lowercase()).collect(),
            extracts: Vec::new(),
        }
    }
    /// Builder: make this stub return `extracts` from `extract_constraints`.
    pub fn with_extracts(mut self, extracts: &[&str]) -> Self {
        self.extracts = extracts.iter().map(|s| s.to_string()).collect();
        self
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
    /// A constraint "looks violated" when the message contains any configured
    /// substring — enough to drive deterministic tests of the batched path.
    fn violations(&self, constraints: &[String], message: &str) -> Vec<bool> {
        let lc = message.to_lowercase();
        let hit = self.yes_if_contains.iter().any(|s| lc.contains(s));
        vec![hit; constraints.len()]
    }
    fn extract_constraints(&self) -> Vec<String> {
        self.extracts.clone()
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

const VIOLATIONS_SYSTEM: &str = "You are a strict reviewer. You are given a \
numbered list of constraints and one assistant message. Return ONLY a JSON \
array of the 1-based indices of the constraints the message CLEARLY violates, \
e.g. [1,3]. If none are violated, return []. Flag a constraint only when you \
are confident; when unsure, leave it out.";

const EXTRACT_SYSTEM: &str = "You extract the explicit constraints a user \
placed on an AI assistant's work — durable rules the output must obey (style, \
format, tone, scope, technology, or things to avoid). Return ONLY a JSON array \
of short, self-contained constraint strings, each phrased as a checkable \
imperative, e.g. [\"Do not use external libraries\", \"Keep answers under 200 \
words\"]. Extract only genuine, lasting constraints; ignore the task request \
itself and one-off questions. If there are none, return [].";

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

    /// One chat completion. Returns the message content, or `None` on any
    /// transport/parse error — the single fail-safe seam every judge call flows
    /// through.
    async fn complete(&self, system: &str, user: &str, max_tokens: u32) -> Option<String> {
        let body = ChatBody {
            model: &self.model,
            messages: vec![
                ChatMsg {
                    role: "system",
                    content: system,
                },
                ChatMsg {
                    role: "user",
                    content: user,
                },
            ],
            temperature: 0.0,
            max_tokens,
        };
        let url = format!("{}/v1/chat/completions", self.base_url);
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .ok()?;
        let v = resp.json::<serde_json::Value>().await.ok()?;
        v.pointer("/choices/0/message/content")
            .and_then(|c| c.as_str())
            .map(|s| s.to_string())
    }

    async fn check(&self, question: &str, context: &str) -> JudgeAnswer {
        let user = format!("{question}\n\n---\n{context}");
        match self.complete(SYSTEM, &user, 200).await {
            Some(content) => parse_judge_content(&content),
            None => JudgeAnswer::no(),
        }
    }

    async fn violations(&self, constraints: &[String], message: &str) -> Vec<bool> {
        let mut list = String::new();
        for (i, c) in constraints.iter().enumerate() {
            list.push_str(&format!("{}. {}\n", i + 1, c));
        }
        let user = format!("Constraints:\n{list}\n---\nAssistant message:\n{message}");
        let mut out = vec![false; constraints.len()];
        let Some(content) = self.complete(VIOLATIONS_SYSTEM, &user, 100).await else {
            return out;
        };
        for idx in parse_index_array(&content) {
            if (1..=constraints.len()).contains(&idx) {
                out[idx - 1] = true;
            }
        }
        out
    }

    async fn extract_constraints(&self, user_message: &str) -> Vec<String> {
        match self.complete(EXTRACT_SYSTEM, user_message, 300).await {
            Some(content) => parse_string_array(&content),
            None => Vec::new(),
        }
    }
}

/// Parse a JSON array of positive integers embedded anywhere in `content`
/// (1-based constraint indices). Anything unparseable → empty (fail-safe).
pub fn parse_index_array(content: &str) -> Vec<usize> {
    let (Some(start), Some(end)) = (content.find('['), content.rfind(']')) else {
        return Vec::new();
    };
    if end <= start {
        return Vec::new();
    }
    serde_json::from_str::<Vec<i64>>(&content[start..=end])
        .map(|v| {
            v.into_iter()
                .filter(|n| *n > 0)
                .map(|n| n as usize)
                .collect()
        })
        .unwrap_or_default()
}

/// Parse a JSON array of non-empty strings embedded anywhere in `content`.
/// Anything unparseable → empty (fail-safe).
pub fn parse_string_array(content: &str) -> Vec<String> {
    let (Some(start), Some(end)) = (content.find('['), content.rfind(']')) else {
        return Vec::new();
    };
    if end <= start {
        return Vec::new();
    }
    serde_json::from_str::<Vec<String>>(&content[start..=end])
        .map(|v| {
            v.into_iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
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

    #[test]
    fn parse_index_array_variants() {
        assert_eq!(parse_index_array("[1,3]"), vec![1, 3]);
        assert_eq!(parse_index_array("Violated: [2] only"), vec![2]);
        assert_eq!(parse_index_array("[]"), Vec::<usize>::new());
        // Non-positive / garbage → dropped or empty (fail-safe).
        assert_eq!(parse_index_array("[0, 2, -1]"), vec![2]);
        assert_eq!(parse_index_array("nope"), Vec::<usize>::new());
    }

    #[test]
    fn parse_string_array_variants() {
        assert_eq!(
            parse_string_array(r#"["no libs", "  under 200 words  "]"#),
            vec!["no libs".to_string(), "under 200 words".to_string()]
        );
        assert_eq!(parse_string_array("[]"), Vec::<String>::new());
        // Empty strings filtered; unparseable → empty.
        assert_eq!(
            parse_string_array(r#"["keep", "  "]"#),
            vec!["keep".to_string()]
        );
        assert_eq!(parse_string_array("garbage"), Vec::<String>::new());
    }

    #[tokio::test]
    async fn disabled_violations_all_false_and_no_extracts() {
        let j = Judge::Disabled;
        assert_eq!(
            j.violations(&["a".into(), "b".into()], "anything").await,
            vec![false, false]
        );
        assert!(j.extract_constraints("do X, and never Y").await.is_empty());
        // Empty input short-circuits to empty (no call).
        assert!(j.violations(&[], "msg").await.is_empty());
    }

    #[tokio::test]
    async fn stub_violations_and_extracts() {
        let j = Judge::Stub(StubJudge::new(&["bcrypt"]).with_extracts(&["No comments"]));
        assert_eq!(
            j.violations(&["c1".into()], "let's use bcrypt").await,
            vec![true]
        );
        assert_eq!(
            j.violations(&["c1".into()], "use argon2 only").await,
            vec![false]
        );
        assert_eq!(
            j.extract_constraints("whatever").await,
            vec!["No comments".to_string()]
        );
    }
}
