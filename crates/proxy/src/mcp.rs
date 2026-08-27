//! `drifterr-proxy mcp` — an MCP server exposing the anchor and the rule checker.
//!
//! # Why this is the 10× move
//!
//! Everything else in Drifterr is *detection*: the agent writes something, and afterwards
//! we say it broke a rule. The tokens are spent, the code is written, and the user's job
//! is now to undo it. Even the `UserPromptSubmit` hook only restates the rules once the
//! session is already drifting.
//!
//! An MCP server inverts that. The agent gets tools it can call *while working*:
//!
//! * `drifterr_anchor` — "what did the user actually ask for, and what rules apply?"
//! * `drifterr_check` — "before I hand this back, does it break any of them?"
//!
//! A capable agent that can ask both questions does not need to be caught, because the
//! violation never happens. That is a different product from a monitor, and a strictly
//! better one: prevention costs one cheap local call, while detection costs a wasted turn
//! plus the user's attention.
//!
//! # Why the checker is a tool and not a lecture
//!
//! `drifterr_check` returns the *same* deterministic verdict as the live engine and CI —
//! same rules, same evidence, same spans. So an agent that self-checks gets exactly the
//! answer the human would have been shown, which makes the three paths impossible to
//! disagree. It is also honest about what it cannot verify: a rule the engine can't check
//! is reported as unverified rather than silently passed, so an agent is never told
//! "clean" about something nobody looked at.
//!
//! # Transport
//!
//! Line-delimited JSON-RPC 2.0 over stdio, which is what MCP clients speak. Deliberately
//! hand-rolled: the protocol surface used here is four methods, and adding an SDK to the
//! crate that must stay free of network dependencies (see `tests/egress.rs`) is a poor
//! trade. Unknown methods get a proper JSON-RPC error rather than a panic, and a malformed
//! line is skipped — a crashed MCP server takes the agent's tools down with it.

use drifterr_engine::baseline::{Baseline, Checkable, Constraint};
use serde_json::{json, Value};

/// MCP protocol revision this server implements.
const PROTOCOL_VERSION: &str = "2024-11-05";

/// Cap on content handed to `drifterr_check`, so a runaway agent cannot pin the process.
const MAX_CHECK_BYTES: usize = 512 * 1024;

/// How long to wait on the local control API before answering from what we have. A tool
/// call blocks the agent, so a stopped Drifterr must not become a stalled turn.
const TIMEOUT: std::time::Duration = std::time::Duration::from_millis(600);

/// Handle one JSON-RPC request, returning the response to write (or `None` for a
/// notification, which by spec gets no reply).
///
/// `anchor` supplies the current goal/constraints — injected rather than fetched so this is
/// testable without a running control API.
pub fn handle(line: &str, anchor: &Anchor) -> Option<String> {
    let req: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        // A malformed line must not kill the server: the agent would silently lose every
        // tool, with no indication why.
        Err(_) => return None,
    };
    // Notifications (no id) are acknowledged by silence, per JSON-RPC.
    let id = req.get("id").cloned()?;
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");

    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "drifterr", "version": env!("CARGO_PKG_VERSION") }
        })),
        "tools/list" => Ok(json!({ "tools": tool_specs() })),
        "tools/call" => call_tool(req.get("params"), anchor),
        // `ping` is part of the spec and cheap to honour.
        "ping" => Ok(json!({})),
        other => Err(format!("unknown method '{other}'")),
    };

    Some(match result {
        Ok(v) => json!({ "jsonrpc": "2.0", "id": id, "result": v }).to_string(),
        // -32601 is JSON-RPC's "method not found"; close enough for every error here and
        // better than inventing codes an MCP client won't recognise.
        Err(msg) => json!({
            "jsonrpc": "2.0", "id": id,
            "error": { "code": -32601, "message": msg }
        })
        .to_string(),
    })
}

/// The session's current intent, as the tools report it.
///
/// Holds the engine's own [`Constraint`] values rather than their labels. That is the
/// whole reason `drifterr_check` can promise the same verdict as the live engine: it
/// evaluates the same rule objects, instead of re-deriving rules from prose and quietly
/// checking a subset.
#[derive(Debug, Clone, Default)]
pub struct Anchor {
    pub goal: String,
    /// Active constraints, carrying both the user-facing text and the rule.
    pub constraints: Vec<Constraint>,
    /// Approaches the user explicitly rejected — the thing an agent is most likely to
    /// reintroduce a dozen turns later, having forgotten.
    pub rejected: Vec<String>,
}

impl Anchor {
    /// Read the anchor from a serialized [`Baseline`] (the control API's `/anchor`).
    ///
    /// Malformed or missing fields degrade to empty rather than failing: an agent that
    /// gets "nothing is pinned" is told so honestly by [`render_anchor`], which is far
    /// better than a tool that errors.
    pub fn from_baseline_json(v: &Value) -> Self {
        match serde_json::from_value::<Baseline>(v.clone()) {
            Ok(b) => Self::from_baseline(&b),
            Err(_) => Anchor::default(),
        }
    }

    /// As [`from_baseline_json`](Self::from_baseline_json), from the baseline itself.
    pub fn from_baseline(b: &Baseline) -> Self {
        Anchor {
            goal: b.goal.trim().to_string(),
            // Retired constraints are no longer in force; reporting them would send an
            // agent chasing a rule the user has already dropped.
            constraints: b.constraints.iter().filter(|c| c.active).cloned().collect(),
            rejected: b
                .decisions
                .iter()
                .filter(|d| d.rejected)
                .map(|d| d.text.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect(),
        }
    }

    /// Build an anchor from what a user would type, for tests and for the `--check`
    /// smoke path. Uses the same [`Baseline::declare`] the intent editor does, so a
    /// phrase becomes a deterministic or a judge constraint by exactly the same rule.
    pub fn from_texts(goal: &str, constraints: &[&str], rejected: &[&str]) -> Self {
        let mut b = Baseline {
            goal: goal.to_string(),
            constraints: Vec::new(),
            decisions: rejected
                .iter()
                .map(|t| drifterr_engine::baseline::Decision {
                    text: (*t).to_string(),
                    rejected: true,
                })
                .collect(),
        };
        let phrases: Vec<String> = constraints.iter().map(|c| (*c).to_string()).collect();
        b.declare(goal, &phrases);
        Self::from_baseline(&b)
    }
}

/// Whether a request will actually read the anchor, so the stdio loop only pays for a
/// control-API round trip on a real tool call — `initialize` and `tools/list` are answered
/// from constants and must stay instant.
pub fn needs_anchor(line: &str) -> bool {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|v| {
            v.get("method")
                .and_then(Value::as_str)
                .map(|m| m == "tools/call")
        })
        .unwrap_or(false)
}

/// Read the live anchor from the local control API.
///
/// Every failure path yields an empty anchor, which the tools report as "nothing is
/// pinned" rather than as "you have no constraints".
pub async fn fetch_anchor(api_base: &str) -> Anchor {
    let client = match reqwest::Client::builder()
        .timeout(TIMEOUT)
        // Localhost only; an ambient HTTP(S)_PROXY belongs to the user's shell.
        .no_proxy()
        .build()
    {
        Ok(c) => c,
        Err(_) => return Anchor::default(),
    };
    // Authenticated like every other control-API consumer. A missing token means
    // Drifterr is not running; the agent then gets an empty anchor, which the tool
    // reports honestly as "no intent recorded" rather than as "no rules apply".
    let token = crate::auth::read_token().unwrap_or_default();
    match client
        .get(format!("{api_base}/anchor"))
        .header(crate::auth::TOKEN_HEADER, &token)
        .send()
        .await
    {
        Ok(r) => r
            .json::<Value>()
            .await
            .map(|v| Anchor::from_baseline_json(&v))
            .unwrap_or_default(),
        Err(_) => Anchor::default(),
    }
}

fn tool_specs() -> Value {
    json!([
        {
            "name": "drifterr_anchor",
            "description":
                "Get the user's stated goal, the constraints in force, and approaches they \
                 explicitly rejected. Call this before starting substantial work, and again \
                 if you are unsure whether something is in scope. This is the user's own \
                 statement of intent, not a guess.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "drifterr_check",
            "description":
                "Check work against the user's constraints BEFORE returning it. Pass a diff, \
                 a patch, or the code you are about to hand back. Returns each violated rule \
                 with the exact offending line. Deterministic and local — no model call. \
                 Rules that cannot be verified automatically are reported as unverified, not \
                 as passing.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "content": {
                        "type": "string",
                        "description":
                            "The work to check. Wrap code in a fenced block so code-scoped \
                             rules apply to it — prose mentioning a filename is not the same \
                             as touching that file."
                    }
                },
                "required": ["content"],
                "additionalProperties": false
            }
        }
    ])
}

fn call_tool(params: Option<&Value>, anchor: &Anchor) -> Result<Value, String> {
    let params = params.ok_or("tools/call requires params")?;
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or("tools/call requires a tool name")?;
    let args = params.get("arguments");

    match name {
        "drifterr_anchor" => Ok(text_result(&render_anchor(anchor))),
        "drifterr_check" => {
            let content = args
                .and_then(|a| a.get("content"))
                .and_then(Value::as_str)
                .ok_or("drifterr_check requires a 'content' string")?;
            if content.len() > MAX_CHECK_BYTES {
                return Err(format!(
                    "content is {} bytes (max {MAX_CHECK_BYTES}); check a smaller diff",
                    content.len()
                ));
            }
            Ok(text_result(&render_check(content, anchor)))
        }
        other => Err(format!("unknown tool '{other}'")),
    }
}

/// MCP tool results are content blocks, not bare strings.
fn text_result(text: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": text }] })
}

fn render_anchor(anchor: &Anchor) -> String {
    if anchor.goal.trim().is_empty() && anchor.constraints.is_empty() {
        // Say so plainly. An agent told "no constraints" when Drifterr simply isn't
        // tracking anything would reasonably assume it had a free hand.
        return "No intent is currently pinned in Drifterr, so there are no constraints to \
                report. Ask the user what the goal and hard rules are, rather than assuming \
                there are none."
            .to_string();
    }
    let mut out = String::from("The user's stated intent for this session:\n");
    if !anchor.goal.trim().is_empty() {
        out.push_str(&format!("\nGoal: {}\n", anchor.goal.trim()));
    }
    if !anchor.constraints.is_empty() {
        out.push_str("\nConstraints in force — do not break these:\n");
        for c in &anchor.constraints {
            // Mark which ones drifterr_check can actually verify. An agent that knows
            // a rule is unenforced is the only one that will take care over it itself.
            let checked = match c.checkable {
                Checkable::Deterministic => "",
                Checkable::Judge => "  (not machine-checkable — you must honour this one)",
            };
            out.push_str(&format!("- {}{checked}\n", c.text.trim()));
        }
    }
    if !anchor.rejected.is_empty() {
        out.push_str("\nApproaches the user explicitly REJECTED — do not reintroduce:\n");
        for r in &anchor.rejected {
            out.push_str(&format!("- {}\n", r.trim()));
        }
    }
    out.push_str(
        "\nBefore returning substantial work, call drifterr_check on it to confirm none of \
         these are broken.\n",
    );
    out
}

fn render_check(content: &str, anchor: &Anchor) -> String {
    // The engine's own constraints, handed to the same checker the CI path uses. Not
    // re-parsed from their labels: that round trip silently drops every parameterized
    // rule, and an agent told "1 rule checked, clean" about two rules is worse off than
    // one told nothing.
    let report = crate::check::check_with(content, anchor.constraints.clone(), Vec::new());

    let mut out = String::new();
    if report.checked == 0 {
        out.push_str(
            "No automatically verifiable rules are pinned, so nothing was checked. This is \
             NOT a pass — re-read the constraints from drifterr_anchor and satisfy them \
             yourself.\n",
        );
    } else if report.ok() {
        out.push_str(&format!(
            "PASS — {} rule(s) checked, no violations found.\n",
            report.checked
        ));
    } else {
        out.push_str(&format!(
            "VIOLATION — {} of {} rule(s) broken. Fix these before returning the work:\n\n",
            report.violations.len(),
            report.checked
        ));
        for v in &report.violations {
            out.push_str(&format!("- {}", v.evidence.detail));
            if let Some(span) = v.evidence.span.as_deref() {
                out.push_str(&format!("\n  offending text: {span}"));
            }
            out.push('\n');
        }
    }
    if !report.skipped.is_empty() {
        out.push_str(&format!(
            "\n{} rule(s) could not be checked automatically and were NOT verified — \
             satisfy them yourself:\n",
            report.skipped.len()
        ));
        for s in &report.skipped {
            out.push_str(&format!("- {s}\n"));
        }
    }
    out
}

/// The settings snippet registering this server with Claude Code.
pub fn install_snippet(exe: &str) -> String {
    format!(
        r#"{{
  "mcpServers": {{
    "drifterr": {{
      "command": "{exe}",
      "args": ["mcp"]
    }}
  }}
}}"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor() -> Anchor {
        Anchor::from_texts(
            "Add a CSV export to the reports page",
            &["No new dependencies", "Don't touch package.json"],
            &["use lodash"],
        )
    }

    fn call(tool: &str, args: Value) -> String {
        let req = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": tool, "arguments": args }
        });
        let resp = handle(&req.to_string(), &anchor()).expect("a request gets a response");
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert!(v.get("error").is_none(), "unexpected error: {v}");
        v["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn initialize_advertises_tools() {
        let req = json!({"jsonrpc":"2.0","id":1,"method":"initialize"});
        let v: Value = serde_json::from_str(&handle(&req.to_string(), &anchor()).unwrap()).unwrap();
        assert_eq!(v["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert!(v["result"]["capabilities"]["tools"].is_object());
        assert_eq!(v["result"]["serverInfo"]["name"], "drifterr");
    }

    #[test]
    fn tools_list_describes_both_tools_with_schemas() {
        let req = json!({"jsonrpc":"2.0","id":2,"method":"tools/list"});
        let v: Value = serde_json::from_str(&handle(&req.to_string(), &anchor()).unwrap()).unwrap();
        let tools = v["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"drifterr_anchor"));
        assert!(names.contains(&"drifterr_check"));
        // The check tool must declare its required argument, or clients cannot call it.
        let check = tools
            .iter()
            .find(|t| t["name"] == "drifterr_check")
            .unwrap();
        assert_eq!(check["inputSchema"]["required"][0], "content");
    }

    #[test]
    fn anchor_tool_reports_goal_constraints_and_rejections() {
        let text = call("drifterr_anchor", json!({}));
        assert!(text.contains("CSV export"));
        assert!(text.contains("No new dependencies"));
        // Rejected decisions are the thing an agent most often reintroduces.
        assert!(text.contains("REJECTED"));
        assert!(text.contains("use lodash"));
        // And it should tell the agent to self-check.
        assert!(text.contains("drifterr_check"));
    }

    #[test]
    fn an_empty_anchor_says_so_rather_than_implying_a_free_hand() {
        let req = json!({
            "jsonrpc":"2.0","id":1,"method":"tools/call",
            "params": {"name":"drifterr_anchor","arguments":{}}
        });
        let resp = handle(&req.to_string(), &Anchor::default()).unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("No intent is currently pinned"));
        assert!(
            text.contains("rather than assuming there are none"),
            "must not read as 'you have no constraints': {text}"
        );
    }

    #[test]
    fn check_tool_passes_clean_work() {
        let text = call(
            "drifterr_check",
            json!({"content": "```diff\n--- a/src/app.ts\n+++ b/src/app.ts\n+const x = 1;\n```"}),
        );
        assert!(text.starts_with("PASS"), "{text}");
    }

    #[test]
    fn check_tool_reports_the_violated_rule_and_the_offending_line() {
        let text = call(
            "drifterr_check",
            json!({"content": "```bash\nnpm install csv-stringify\n```"}),
        );
        assert!(text.starts_with("VIOLATION"), "{text}");
        assert!(text.contains("offending text"), "{text}");
        assert!(text.contains("npm install csv-stringify"), "{text}");
    }

    #[test]
    fn check_tool_never_reports_nothing_checked_as_a_pass() {
        // The dangerous case: an agent told "clean" about work nobody looked at would
        // proceed with false confidence.
        let empty = Anchor::from_texts(
            "do a thing",
            &["Always prefer composition over inheritance"],
            &[],
        );
        let req = json!({
            "jsonrpc":"2.0","id":1,"method":"tools/call",
            "params": {"name":"drifterr_check","arguments":{"content":"anything"}}
        });
        let v: Value = serde_json::from_str(&handle(&req.to_string(), &empty).unwrap()).unwrap();
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("NOT a pass"), "{text}");
    }

    #[test]
    fn unverifiable_rules_are_flagged_as_not_verified() {
        let mixed = Anchor::from_texts(
            "g",
            &[
                "No new dependencies",
                "Always prefer composition over inheritance",
            ],
            &[],
        );
        let req = json!({
            "jsonrpc":"2.0","id":1,"method":"tools/call",
            "params": {"name":"drifterr_check","arguments":{"content":"```ts\nconst a=1;\n```"}}
        });
        let v: Value = serde_json::from_str(&handle(&req.to_string(), &mixed).unwrap()).unwrap();
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("PASS"), "{text}");
        assert!(text.contains("NOT verified"), "{text}");
        assert!(text.contains("composition"), "{text}");
    }

    #[test]
    fn oversized_content_is_refused_rather_than_processed() {
        let big = "x".repeat(MAX_CHECK_BYTES + 1);
        let req = json!({
            "jsonrpc":"2.0","id":9,"method":"tools/call",
            "params": {"name":"drifterr_check","arguments":{"content": big}}
        });
        let v: Value = serde_json::from_str(&handle(&req.to_string(), &anchor()).unwrap()).unwrap();
        assert_eq!(v["error"]["code"], -32601);
        assert!(v["error"]["message"].as_str().unwrap().contains("bytes"));
    }

    #[test]
    fn protocol_errors_are_returned_not_panicked() {
        // Every one of these used to be a plausible panic. A crashed MCP server takes the
        // agent's whole toolset down with no indication why.
        for line in [
            r#"{"jsonrpc":"2.0","id":1,"method":"nonsense"}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call"}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{}}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"nope"}}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"drifterr_check","arguments":{}}}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"drifterr_check","arguments":{"content":42}}}"#,
        ] {
            let resp = handle(line, &anchor()).unwrap_or_else(|| panic!("no response to {line}"));
            let v: Value = serde_json::from_str(&resp).unwrap();
            assert!(
                v.get("error").is_some(),
                "expected an error for {line}: {v}"
            );
            assert_eq!(
                v["id"], 1,
                "the id must be echoed so the client can match it"
            );
        }
    }

    #[test]
    fn malformed_and_notification_lines_get_no_reply_and_no_crash() {
        assert!(handle("not json", &anchor()).is_none());
        assert!(handle("", &anchor()).is_none());
        assert!(handle("[]", &anchor()).is_none());
        // A notification (no id) must be acknowledged by silence, per JSON-RPC.
        assert!(handle(
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            &anchor()
        )
        .is_none());
    }

    #[test]
    fn the_anchor_is_read_from_the_serialized_baseline() {
        // Straight off `GET /anchor`, rules included — which is the point: the checker
        // gets the engine's rule objects, not a re-parse of their labels.
        let v = json!({
            "goal": "  Add a CSV export  ",
            "constraints": [
                {
                    "id": "c1", "text": "Don't touch package.json", "type": "tech",
                    "checkable": "deterministic", "active": true,
                    "rule": { "kind": "forbid_pattern", "pattern": "(?m)^(?:diff --git )[ab]/package\\.json\\b" }
                }
            ],
            "decisions": [
                { "text": "use lodash", "rejected": true },
                { "text": "stream the rows", "rejected": false }
            ]
        });
        let a = Anchor::from_baseline_json(&v);
        assert_eq!(a.goal, "Add a CSV export");
        assert_eq!(a.constraints.len(), 1);
        assert!(
            a.constraints[0].rule.is_some(),
            "the rule must survive the wire, or the checker silently checks less"
        );
        // Only *rejected* decisions are reported; an accepted one is not a prohibition.
        assert_eq!(a.rejected, ["use lodash"]);
    }

    #[test]
    fn retired_constraints_are_not_reported_as_in_force() {
        // The user explicitly dropped it. Restating it would send the agent chasing a rule
        // that no longer exists — the same mistake in the opposite direction.
        let v = json!({
            "goal": "g",
            "constraints": [
                { "id": "c1", "text": "No new dependencies", "type": "tech",
                  "checkable": "judge", "active": true },
                { "id": "c2", "text": "No console.log", "type": "format",
                  "checkable": "judge", "active": false }
            ]
        });
        let a = Anchor::from_baseline_json(&v);
        assert_eq!(a.constraints.len(), 1);
        assert_eq!(a.constraints[0].text, "No new dependencies");
    }

    #[test]
    fn a_malformed_anchor_reply_yields_an_empty_anchor_not_a_panic() {
        for v in [
            json!({}),
            json!(null),
            json!({ "goal": 42, "constraints": "nope" }),
            json!({ "goal": "g", "constraints": [{ "nonsense": true }] }),
        ] {
            let a = Anchor::from_baseline_json(&v);
            assert!(a.constraints.is_empty(), "for {v}");
        }
    }

    #[test]
    fn unverifiable_constraints_are_labelled_as_such_in_the_anchor() {
        // An agent that knows a rule is unenforced is the only one that will take care
        // over it itself. Reporting it indistinguishably from a checked rule invites the
        // agent to assume drifterr_check has it covered.
        let a = Anchor::from_texts("g", &["Always prefer composition over inheritance"], &[]);
        let text = render_anchor(&a);
        assert!(text.contains("not machine-checkable"), "{text}");
    }

    #[test]
    fn only_tool_calls_pay_for_the_control_api_round_trip() {
        assert!(needs_anchor(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call"}"#
        ));
        assert!(!needs_anchor(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#
        ));
        assert!(!needs_anchor(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#
        ));
        assert!(!needs_anchor("garbage"));
    }

    #[test]
    fn install_snippet_is_valid_json_for_claude_code() {
        let s = install_snippet("/usr/local/bin/drifterr-proxy");
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(
            v["mcpServers"]["drifterr"]["command"],
            "/usr/local/bin/drifterr-proxy"
        );
        assert_eq!(v["mcpServers"]["drifterr"]["args"][0], "mcp");
    }
}
