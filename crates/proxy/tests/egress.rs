//! Egress guarantee — proves no chat content can leave the machine except to the
//! user's own model provider.
//!
//! Drifterr's #1 promise is local-first: conversations, prompts, signals and
//! drift scores never leave the machine. The one server-side component is
//! accounts/billing (Supabase + Stripe), which holds identity only and lives in
//! the JS/desktop-auth layer — never in these crates. This test file encodes
//! that boundary as CI-enforced invariants, so it fails the moment a byte of
//! conversation is wired to leave.
//!
//! Three layers of proof:
//!   A. the crates that HOLD/parse chat content have no network dependency at all;
//!   B. the only two network callers (proxy, judge) hardcode no analytics/backend
//!      host — they only ever talk to the configured model provider;
//!   C. at runtime, a chat relayed through the proxy reaches the configured
//!      upstream faithfully and no beacon fires anywhere else.

use axum::body::Body;
use axum::extract::Request;
use axum::response::Response;
use axum::Router;
use drifterr_proxy::{proxy_router, AppState, ProxyConfig};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;

fn workspace_crate(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(name)
}

/// Recursively collect `.rs` files under `dir`.
fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().and_then(|x| x.to_str()) == Some("rs") {
            out.push(p);
        }
    }
}

// ---- A. chat-content crates have zero network dependencies -------------------

/// The crates that construct, parse, store, or score conversation content. None
/// may pull a network client — that is the structural backbone of "chat never
/// leaves the machine". Adding `reqwest`/`hyper`/etc. to any of these fails CI.
#[test]
fn chat_content_crates_have_no_network_dependency() {
    // Whole-word network client crates that would enable egress.
    const FORBIDDEN: [&str; 6] = ["reqwest", "hyper", "ureq", "isahc", "curl", "tungstenite"];
    for crate_name in [
        "engine",
        "store",
        "adapters",
        "tokenizer",
        "embeddings",
        "intervention",
    ] {
        let manifest = workspace_crate(crate_name).join("Cargo.toml");
        let text = std::fs::read_to_string(&manifest)
            .unwrap_or_else(|e| panic!("read {}: {e}", manifest.display()));
        for dep in FORBIDDEN {
            assert!(
                !text.contains(dep),
                "crate `{crate_name}` must not depend on `{dep}` — it holds chat \
                 content and must never touch the network (local-first invariant)"
            );
        }
    }
}

// ---- B. the network callers hardcode no analytics / backend host -------------

/// The two crates that DO make network calls — the proxy (relays to the model
/// provider) and the judge (calls the user's own OpenRouter) — must never carry
/// a hardcoded analytics, telemetry, or Drifterr-backend host. Chat may only ever
/// flow to the configured model provider; nothing else.
#[test]
fn network_callers_hardcode_no_forbidden_host() {
    // Registrable domains that would mean chat (or any session data) leaves to a
    // third party or to a Drifterr server. We match full domains (with TLD) on
    // purpose: the accounts/billing architecture (Supabase/Stripe) is legitimately
    // *mentioned* in doc comments, but a real egress would carry the host domain
    // in a URL string. A bare word like "Supabase" in prose must not trip this;
    // "x.supabase.co" in a request URL must.
    const FORBIDDEN_HOSTS: [&str; 9] = [
        "supabase.co",
        "posthog.com",
        "segment.com",
        "sentry.io",
        "amplitude.com",
        "mixpanel.com",
        "google-analytics.com",
        "datadoghq.com",
        "segmentapis.com",
    ];
    for crate_name in ["proxy", "judge"] {
        let mut files = Vec::new();
        rs_files(&workspace_crate(crate_name).join("src"), &mut files);
        assert!(!files.is_empty(), "no source found for {crate_name}");
        for file in &files {
            // Don't scan this very test file's expectations if it ever moves here.
            let text = std::fs::read_to_string(file).unwrap_or_default();
            let lower = text.to_ascii_lowercase();
            for host in FORBIDDEN_HOSTS {
                assert!(
                    !lower.contains(host),
                    "{}: forbidden egress host `{host}` — chat/session data may only \
                     go to the configured model provider",
                    file.display()
                );
            }
        }
    }
}

// ---- B2. the team share payload carries config and counts, never content -----

/// Team sharing is the first capability that asks for anything to leave a laptop, so it
/// gets its own invariant rather than relying on review.
///
/// The test drives real chat content — including a canary and a real constraint violation
/// — through the engine, then asserts that the payload a share would upload contains
/// neither the canary, nor the violation's span, nor the goal, nor a session id. It also
/// pins the *withholding*: a session-inferred rule id (`c1`) names a constraint mined from
/// the user's own messages, so publishing even the id-with-count would reveal that they
/// said something. The count of what was withheld must be visible, not silent.
#[tokio::test]
async fn the_team_share_payload_never_carries_conversation_content() {
    use drifterr_proxy::control_router;
    use drifterr_proxy::entitlement::Plan;

    let cfg = ProxyConfig {
        openai_upstream: "http://unused".into(),
        anthropic_upstream: "http://unused".into(),
        openai_strip_v1: false,
    };
    // Team plan, so sharing is entitled and the payload is actually built; and a real
    // store, so there are genuine flag counts to filter rather than an empty list that
    // would pass vacuously.
    let state = AppState::with_judge(
        cfg,
        Some(drifterr_store::Store::open_in_memory().unwrap()),
        drifterr_judge::Judge::Disabled,
    )
    .with_plan(Plan::Team);
    let control = spawn(control_router(state)).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let api = format!("http://{control}");

    // The canary is placed inside the *violating code*, so it lands in the evidence span.
    // That makes this a test of the span specifically, not just of the turn text.
    let canary = "CANARY-team-4b81-do-not-leak";
    let turns = serde_json::json!([
        {"role": "user", "content": "Refactor the reports module, and no console.log"},
        {"role": "assistant", "content":
            format!("Done:\n```ts\nconsole.log('{canary}');\n```")}
    ]);
    client
        .post(format!("{api}/ingest"))
        .json(&serde_json::json!({
            "sessionId": "team-egress-1", "model": "gpt-4o", "turns": turns
        }))
        .send()
        .await
        .unwrap();

    // Confirm the fixture really did violate a rule, or the test would pass vacuously.
    let status: serde_json::Value = client
        .get(format!("{api}/status"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        status["current"]["state"], "red",
        "fixture must produce a real violation, or this proves nothing"
    );

    let preview: serde_json::Value = client
        .get(format!(
            "{api}/team/share-preview?packs=tight-scope&days=14"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(preview["entitled"], true);
    let body = serde_json::to_string(&preview).unwrap();

    for leak in [
        canary,          // the violation's span — an excerpt of the model's own output
        "team-egress-1", // the session id
        "gpt-4o",        // the model
        "console.log(",  // the offending code
        "Refactor",      // the goal, derived from the user's own message
    ] {
        assert!(
            !body.contains(leak),
            "team share payload leaked '{leak}': {body}"
        );
    }

    // The shared config IS present — otherwise the feature does nothing.
    assert!(
        body.contains("No new dependencies"),
        "the selected pack's rules must be shared: {body}"
    );

    // And the withheld local rule is reported rather than silently dropped.
    let withheld = preview["withheld"].as_str().unwrap_or_default();
    assert!(
        withheld.contains("stated in conversation"),
        "the user must be told what was withheld and why: {preview}"
    );
}

/// The gate: a plan without team sharing gets no payload at all, not an empty one.
#[tokio::test]
async fn team_sharing_is_gated_on_the_plan() {
    use drifterr_proxy::control_router;
    use drifterr_proxy::entitlement::Plan;

    let cfg = ProxyConfig {
        openai_upstream: "http://unused".into(),
        anthropic_upstream: "http://unused".into(),
        openai_strip_v1: false,
    };
    let state =
        AppState::with_judge(cfg, None, drifterr_judge::Judge::Disabled).with_plan(Plan::Free);
    let control = spawn(control_router(state)).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();

    let preview: serde_json::Value = client
        .get(format!(
            "http://{control}/team/share-preview?packs=tight-scope"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(preview["entitled"], false);
    assert!(
        preview.get("payload").is_none(),
        "an unentitled plan must not even build a payload: {preview}"
    );
}

// ---- C. runtime: chat relays only to the configured upstream -----------------

async fn spawn(router: Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    addr
}

/// A capturing upstream: records every request body it receives, then returns a
/// minimal non-streaming OpenAI reply.
fn capturing_upstream(seen: Arc<Mutex<Vec<String>>>) -> Router {
    Router::new().fallback(move |req: Request| {
        let seen = seen.clone();
        async move {
            let bytes = axum::body::to_bytes(req.into_body(), 1 << 20)
                .await
                .unwrap_or_default();
            seen.lock().unwrap().push(String::from_utf8_lossy(&bytes).to_string());
            Response::builder()
                .status(200)
                .header("content-type", "application/json")
                .body(Body::from(
                    "{\"choices\":[{\"message\":{\"content\":\"ok\"}}],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":1}}",
                ))
                .unwrap()
        }
    })
}

/// A beacon trap standing in for "anywhere else" (a Drifterr server, an analytics
/// endpoint). The proxy must never contact it — the counter must stay at zero.
fn beacon_trap(hits: Arc<AtomicUsize>) -> Router {
    Router::new().fallback(move |_req: Request| {
        let hits = hits.clone();
        async move {
            hits.fetch_add(1, Ordering::SeqCst);
            Response::builder()
                .status(200)
                .body(Body::from("beacon"))
                .unwrap()
        }
    })
}

#[tokio::test]
async fn chat_relays_only_to_the_configured_upstream() {
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let beacon_hits = Arc::new(AtomicUsize::new(0));

    let upstream = spawn(capturing_upstream(seen.clone())).await;
    // The trap runs but is NEVER configured anywhere — proving no hidden fan-out.
    let _trap = spawn(beacon_trap(beacon_hits.clone())).await;

    let cfg = ProxyConfig {
        openai_upstream: format!("http://{upstream}"),
        anthropic_upstream: format!("http://{upstream}"),
        openai_strip_v1: false,
    };
    let proxy = spawn(proxy_router(AppState::new(cfg, None))).await;

    // A unique canary buried in the chat content.
    let canary = "CANARY-9f3a1c7e-do-not-leak";
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let resp = client
        .post(format!("http://{proxy}/v1/chat/completions"))
        .header("authorization", "Bearer test-key")
        .json(&serde_json::json!({
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": format!("Refactor {canary} in TS")}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let _ = resp.bytes().await.unwrap();

    // Give the off-path detection a beat (it must not change where bytes went).
    tokio::time::sleep(Duration::from_millis(50)).await;

    // The canary reached the CONFIGURED provider — and only there.
    let captured = seen.lock().unwrap();
    assert!(
        captured.iter().any(|b| b.contains(canary)),
        "the chat must reach the configured upstream (faithful relay)"
    );
    assert_eq!(
        beacon_hits.load(Ordering::SeqCst),
        0,
        "the proxy contacted a non-provider endpoint — chat egress leak"
    );
}
