//! `drifterr-proxy hook` — the Claude Code integration for automatic re-anchoring.
//!
//! # Why a hook rather than injection
//!
//! On the proxy channel Drifterr can rewrite an outgoing request, so auto-re-anchor
//! is just a matter of prepending the preamble. The Claude Code channel is a *file
//! watcher*: it reads transcripts after the fact and has no request to modify. So the
//! flagship zero-config path could only ever offer a manual copy-paste, while the
//! site said "one click re-anchors the thread".
//!
//! Claude Code's own hook mechanism closes that gap, and closes it better than
//! injection would. A `UserPromptSubmit` hook runs *before* the model sees the turn,
//! so the constraint reminder arrives **before** the reply that would have broken it
//! rather than after. That is a different and much more valuable thing than detection:
//! prevention.
//!
//! # Install
//!
//! `drifterr-proxy hook --install` prints the settings block. Or by hand, in
//! `~/.claude/settings.json`:
//!
//! ```json
//! {
//!   "hooks": {
//!     "UserPromptSubmit": [
//!       { "hooks": [{ "type": "command", "command": "drifterr-proxy hook" }] }
//!     ]
//!   }
//! }
//! ```
//!
//! # The rule this module must never break
//!
//! **It must never break the user's prompt.** Same principle as the proxy's
//! byte-for-byte passthrough: a monitoring tool that can stop you working is worse
//! than no monitoring tool. So every failure path — Drifterr not running, a timeout,
//! malformed input, a poisoned lock — exits 0 with no output, and the turn proceeds
//! untouched. There is no error condition that justifies blocking a keystroke.

use serde::Deserialize;
use std::time::Duration;

/// How long to wait on the local control API. Generous enough for a cold localhost
/// request, short enough that nobody notices — a hook sits directly in the path of a
/// keypress, so a slow Drifterr must not become a slow prompt.
const TIMEOUT: Duration = Duration::from_millis(600);

/// The subset of Claude Code's `UserPromptSubmit` payload we use.
#[derive(Debug, Default, Deserialize)]
pub struct HookInput {
    /// The session this prompt belongs to — matches `sessionId` in the transcript,
    /// which is what the file-watch channel keys sessions on.
    #[serde(default, rename = "session_id")]
    pub session_id: String,
    #[serde(default)]
    pub cwd: String,
}

/// What the hook decided to do, so the caller can render it and tests can assert it.
#[derive(Debug, PartialEq)]
pub enum Decision {
    /// Inject this text as additional context for the turn.
    Inject(String),
    /// Say nothing. Carries the reason, for `--explain`.
    Quiet(&'static str),
}

/// Parse the hook payload. Anything unrecognizable yields defaults rather than an
/// error: a schema change in Claude Code must degrade to "do nothing", never to a
/// failed prompt.
pub fn parse_input(stdin: &str) -> HookInput {
    serde_json::from_str(stdin).unwrap_or_default()
}

/// Decide what to inject, given the control API's `/status` and `/reanchor` replies.
///
/// Split out from the IO so the policy is testable without a running server.
///
/// Policy, and the reasoning for each part:
///
/// * **Only when RED.** An amber is advisory — the engine itself is not committing to
///   a problem, so spending the user's context window on a reminder would be noise.
/// * **Only with the `autoReanchor` entitlement.** The pricing page sells automatic
///   injection as the Pro capability, so the hook honours the same gate the proxy
///   channel does. Free still gets the manual snapshot, and every install has 14 days
///   of Pro to try it.
/// * **Only if there is a preamble.** No baseline, nothing to restate.
pub fn decide(status: &serde_json::Value, preamble: Option<&str>) -> Decision {
    let ent_ok = status
        .get("entitlement")
        .and_then(|e| e.get("autoReanchor"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if !ent_ok {
        return Decision::Quiet("automatic re-anchor is a Pro capability");
    }
    let state = status
        .get("current")
        .and_then(|c| c.get("state"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("green");
    if state != "red" {
        return Decision::Quiet("session is not drifting");
    }
    match preamble.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => Decision::Inject(p.to_string()),
        None => Decision::Quiet("no baseline to restate"),
    }
}

/// Render a decision as the JSON Claude Code expects on stdout.
///
/// An empty string means "say nothing", which is the correct output for every quiet
/// path including all the failure paths.
pub fn render(decision: &Decision) -> String {
    match decision {
        Decision::Inject(text) => serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "UserPromptSubmit",
                "additionalContext": format!(
                    "[Drifterr] This session is drifting from what you asked for. \
                     Re-anchoring to your stated intent:\n\n{text}"
                )
            }
        })
        .to_string(),
        Decision::Quiet(_) => String::new(),
    }
}

/// Run the hook end to end: read the decision from the local control API.
///
/// Returns the string to print on stdout — empty when nothing should be injected.
/// Never returns an error: every failure is a quiet path.
pub async fn run(api_base: &str, input: &HookInput, explain: bool) -> String {
    let client = match reqwest::Client::builder()
        .timeout(TIMEOUT)
        // Talk to localhost directly; an ambient HTTP(S)_PROXY belongs to the shell.
        .no_proxy()
        .build()
    {
        Ok(c) => c,
        Err(_) => return String::new(),
    };

    // The control API is authenticated (see `auth`), and the hook is a separate
    // short-lived process, so it reads the token from the shared state dir. No
    // token means Drifterr is not running — which is already a quiet path, so it
    // costs nothing to take it early rather than after two refused requests.
    let Some(token) = crate::auth::read_token() else {
        if explain {
            eprintln!("drifterr hook: no control token found — is Drifterr running?");
        }
        return String::new();
    };

    let q = if input.session_id.is_empty() {
        String::new()
    } else {
        format!("?session={}", urlencode(&input.session_id))
    };

    let status: serde_json::Value = match client
        .get(format!("{api_base}/status"))
        .header(crate::auth::TOKEN_HEADER, &token)
        .send()
        .await
    {
        Ok(r) => r.json().await.unwrap_or(serde_json::Value::Null),
        // Drifterr isn't running. Not an error worth reporting into someone's prompt.
        Err(_) => {
            if explain {
                eprintln!("drifterr hook: control API unreachable at {api_base} — staying quiet");
            }
            return String::new();
        }
    };

    // Fetching the re-anchor also opens the "did it hold?" verification window, so
    // automatic re-anchors are measured exactly like manual ones.
    let preamble: Option<String> = match client
        .get(format!("{api_base}/reanchor{q}"))
        .header(crate::auth::TOKEN_HEADER, &token)
        .send()
        .await
    {
        Ok(r) => r.json::<serde_json::Value>().await.ok().and_then(|v| {
            v.get("preamble")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        }),
        Err(_) => None,
    };

    let decision = decide(&status, preamble.as_deref());
    if explain {
        match &decision {
            Decision::Inject(_) => eprintln!("drifterr hook: injecting the re-anchor preamble"),
            Decision::Quiet(why) => eprintln!("drifterr hook: quiet — {why}"),
        }
    }
    render(&decision)
}

/// Minimal percent-encoding for a session id in a query string. Session ids are
/// UUID-ish, but a hand-set id could contain anything and this must not build a
/// malformed URL.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The settings snippet to paste into `~/.claude/settings.json`.
pub fn install_snippet(exe: &str) -> String {
    format!(
        r#"{{
  "hooks": {{
    "UserPromptSubmit": [
      {{
        "hooks": [
          {{ "type": "command", "command": "{exe} hook" }}
        ]
      }}
    ]
  }}
}}"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn status(state: &str, auto: bool) -> serde_json::Value {
        json!({
            "current": { "state": state },
            "entitlement": { "autoReanchor": auto }
        })
    }

    #[test]
    fn injects_only_when_red_and_entitled() {
        let d = decide(&status("red", true), Some("Goal: ship billing"));
        assert_eq!(d, Decision::Inject("Goal: ship billing".to_string()));
    }

    #[test]
    fn amber_is_advisory_and_costs_no_context() {
        // The engine itself isn't committing to a problem at amber, so spending the
        // user's context window on a reminder would be noise.
        assert!(matches!(
            decide(&status("amber", true), Some("Goal: x")),
            Decision::Quiet(_)
        ));
        assert!(matches!(
            decide(&status("green", true), Some("Goal: x")),
            Decision::Quiet(_)
        ));
    }

    #[test]
    fn respects_the_pro_gate_the_pricing_page_advertises() {
        let d = decide(&status("red", false), Some("Goal: x"));
        assert_eq!(
            d,
            Decision::Quiet("automatic re-anchor is a Pro capability")
        );
    }

    #[test]
    fn stays_quiet_without_a_baseline() {
        assert!(matches!(
            decide(&status("red", true), None),
            Decision::Quiet(_)
        ));
        assert!(matches!(
            decide(&status("red", true), Some("   ")),
            Decision::Quiet(_)
        ));
    }

    #[test]
    fn a_malformed_status_never_injects() {
        // Missing fields, wrong types, outright garbage — all must degrade to quiet
        // rather than panic or inject something meaningless.
        for v in [
            json!({}),
            json!(null),
            json!({ "current": "not an object" }),
            json!({ "entitlement": { "autoReanchor": "yes" } }),
            json!({ "current": { "state": 42 }, "entitlement": { "autoReanchor": true } }),
        ] {
            assert!(
                matches!(decide(&v, Some("Goal: x")), Decision::Quiet(_)),
                "should stay quiet for {v}"
            );
        }
    }

    #[test]
    fn quiet_renders_as_no_output_at_all() {
        // Claude Code treats stdout as context, so a quiet decision must print
        // literally nothing — not "{}" and not a newline.
        assert_eq!(render(&Decision::Quiet("whatever")), "");
    }

    #[test]
    fn injection_is_labelled_and_carries_the_preamble() {
        let out = render(&Decision::Inject("Goal: ship billing".into()));
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let ctx = v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        // The model must be able to tell this came from tooling, not the user.
        assert!(ctx.contains("[Drifterr]"));
        assert!(ctx.contains("Goal: ship billing"));
        assert_eq!(
            v["hookSpecificOutput"]["hookEventName"], "UserPromptSubmit",
            "must name the event Claude Code dispatched"
        );
    }

    #[test]
    fn input_parsing_degrades_instead_of_failing() {
        let ok = parse_input(r#"{"session_id":"abc-123","cwd":"/tmp/x"}"#);
        assert_eq!(ok.session_id, "abc-123");
        assert_eq!(ok.cwd, "/tmp/x");
        // A schema change upstream must not turn into a failed prompt.
        assert_eq!(parse_input("not json").session_id, "");
        assert_eq!(parse_input("").session_id, "");
        assert_eq!(parse_input("{}").session_id, "");
        // Unknown fields are ignored, not rejected.
        assert_eq!(
            parse_input(r#"{"session_id":"s","brand_new_field":1}"#).session_id,
            "s"
        );
    }

    #[test]
    fn session_ids_are_encoded_for_the_query_string() {
        assert_eq!(urlencode("abc-123_x.y~z"), "abc-123_x.y~z");
        assert_eq!(urlencode("a b&c=d"), "a%20b%26c%3Dd");
        assert_eq!(urlencode("café"), "caf%C3%A9");
    }

    #[test]
    fn install_snippet_is_valid_json_naming_the_right_event() {
        let s = install_snippet("/usr/local/bin/drifterr-proxy");
        let v: serde_json::Value = serde_json::from_str(&s).expect("snippet must be valid JSON");
        assert_eq!(
            v["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"],
            "/usr/local/bin/drifterr-proxy hook"
        );
    }
}
