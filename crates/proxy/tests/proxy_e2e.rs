//! End-to-end proxy tests: a mock upstream, the real proxy relaying over TCP,
//! and the control API observing detection.
//!
//! The headline assertion is **passthrough integrity** — the bytes the client
//! receives must equal the bytes the upstream sent, exactly. That is the brief's
//! #1 hard point (never break SSE streaming). On top of that we assert that
//! detection fired correctly, off the response path, for both API schemas.

use axum::body::{Body, Bytes};
use axum::extract::Request;
use axum::response::Response;
use axum::Router;
use drifterr_proxy::{control_router, proxy_router, AppState, ProxyConfig};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;

const AUTH: &str = "Bearer test-key";

// An OpenAI-style SSE stream whose deltas spell "Sure, let's create auth.js",
// followed by a usage chunk and the terminator. Split into several frames to
// exercise real multi-chunk streaming.
fn openai_sse_chunks() -> Vec<String> {
    vec![
        "data: {\"choices\":[{\"delta\":{\"content\":\"Sure, \"}}]}\n\n".into(),
        "data: {\"choices\":[{\"delta\":{\"content\":\"let's create \"}}]}\n\n".into(),
        "data: {\"choices\":[{\"delta\":{\"content\":\"auth.js\"}}]}\n\n".into(),
        "data: {\"choices\":[{\"delta\":{}}],\"usage\":{\"prompt_tokens\":150,\"completion_tokens\":12}}\n\n".into(),
        "data: [DONE]\n\n".into(),
    ]
}

fn anthropic_sse_chunks() -> Vec<String> {
    vec![
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":190000}}}\n\n".into(),
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Working on it\"}}\n\n".into(),
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":500}}\n\n".into(),
    ]
}

/// Mock upstream: validates the forwarded auth header and returns an SSE stream
/// chosen by path (Anthropic `/messages` vs OpenAI `/chat/completions`).
async fn mock_handler(req: Request) -> Response {
    let path = req.uri().path().to_string();
    let auth = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if auth != AUTH {
        return Response::builder()
            .status(401)
            .body(Body::from("missing auth"))
            .unwrap();
    }
    let chunks = if path.contains("/messages") {
        anthropic_sse_chunks()
    } else {
        openai_sse_chunks()
    };
    let stream = futures_util::stream::iter(
        chunks
            .into_iter()
            .map(|s| Ok::<_, std::convert::Infallible>(Bytes::from(s))),
    );
    Response::builder()
        .status(200)
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap()
}

async fn spawn(router: Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    addr
}

/// Bring up: mock upstream + proxy + control API, all on ephemeral ports.
/// Returns (proxy_addr, control_addr).
async fn bring_up() -> (SocketAddr, SocketAddr) {
    let mock_addr = spawn(Router::new().fallback(mock_handler)).await;
    let base = format!("http://{mock_addr}");
    let cfg = ProxyConfig {
        openai_upstream: base.clone(),
        anthropic_upstream: base,
        openai_strip_v1: false,
    };
    let state = AppState::new(cfg, None);
    let proxy_addr = spawn(proxy_router(state.clone())).await;
    let control_addr = spawn(control_router(state)).await;
    (proxy_addr, control_addr)
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

/// Poll the control API until a session status appears, then return it.
async fn wait_for_status(client: &reqwest::Client, control: SocketAddr) -> serde_json::Value {
    let url = format!("http://{control}/status");
    for _ in 0..150 {
        let v: serde_json::Value = client.get(&url).send().await.unwrap().json().await.unwrap();
        if !v["current"].is_null() {
            return v["current"].clone();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("status never appeared");
}

#[tokio::test]
async fn openai_streaming_passthrough_is_byte_exact_and_detects_violation() {
    let (proxy, control) = bring_up().await;
    let client = client();

    let req_body = serde_json::json!({
        "model": "gpt-4o",
        "stream": true,
        "messages": [{"role": "user", "content": "refactor auth in TS, no JS"}]
    });
    let resp = client
        .post(format!("http://{proxy}/v1/chat/completions"))
        .header("authorization", AUTH)
        .json(&req_body)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("content-type").unwrap(),
        "text/event-stream",
        "content-type must be relayed unchanged"
    );

    // PASSTHROUGH INTEGRITY: client bytes == upstream bytes, exactly.
    let received = resp.bytes().await.unwrap();
    let expected: Bytes = Bytes::from(openai_sse_chunks().concat());
    assert_eq!(received, expected, "streamed body was altered by the proxy");

    // DETECTION (off the response path): the .js violation turns the widget red.
    let status = wait_for_status(&client, control).await;
    assert_eq!(status["state"], "red");
    assert_eq!(status["triggering"]["signal"], "constraint");
    assert_eq!(status["triggering"]["constraintId"], "c1");
    assert_eq!(status["triggering"]["span"], ".js");
    assert_eq!(status["exact"], true, "usage present ⇒ exact saturation");
}

#[tokio::test]
async fn anthropic_streaming_detects_saturation_red() {
    let (proxy, control) = bring_up().await;
    let client = client();

    // 190000 input + 500 output over a 200k window ⇒ ~95% ⇒ saturation RED,
    // with no constraint involved.
    let req_body = serde_json::json!({
        "model": "claude-opus-4-x",
        "stream": true,
        "messages": [{"role": "user", "content": "keep going"}]
    });
    let resp = client
        .post(format!("http://{proxy}/v1/messages"))
        .header("authorization", AUTH)
        .json(&req_body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let received = resp.bytes().await.unwrap();
    let expected: Bytes = Bytes::from(anthropic_sse_chunks().concat());
    assert_eq!(received, expected);

    let status = wait_for_status(&client, control).await;
    assert_eq!(status["triggering"]["signal"], "saturation");
    assert!(status["saturationPct"].as_u64().unwrap() >= 90);
    assert_eq!(status["exact"], true);
}

/// Mock upstream whose reply reintroduces "bcrypt" (for the judge test).
async fn mock_bcrypt(req: Request) -> Response {
    if req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        != Some(AUTH)
    {
        return Response::builder()
            .status(401)
            .body(Body::from("no auth"))
            .unwrap();
    }
    let chunks = vec![
        "data: {\"choices\":[{\"delta\":{\"content\":\"Sure, let's hash the passwords with bcrypt now.\"}}]}\n\n".to_string(),
        "data: {\"choices\":[{\"delta\":{}}],\"usage\":{\"prompt_tokens\":60,\"completion_tokens\":9}}\n\n".to_string(),
        "data: [DONE]\n\n".to_string(),
    ];
    let stream = futures_util::stream::iter(
        chunks
            .into_iter()
            .map(|s| Ok::<_, std::convert::Infallible>(Bytes::from(s))),
    );
    Response::builder()
        .status(200)
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap()
}

/// Mock upstream whose reply is informal ("lol") — for the fuzzy-constraint test.
async fn mock_lol(req: Request) -> Response {
    if req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        != Some(AUTH)
    {
        return Response::builder()
            .status(401)
            .body(Body::from("no auth"))
            .unwrap();
    }
    let chunks = vec![
        "data: {\"choices\":[{\"delta\":{\"content\":\"lol yeah sure whatever you want\"}}]}\n\n".to_string(),
        "data: {\"choices\":[{\"delta\":{}}],\"usage\":{\"prompt_tokens\":40,\"completion_tokens\":7}}\n\n".to_string(),
        "data: [DONE]\n\n".to_string(),
    ];
    let stream = futures_util::stream::iter(
        chunks
            .into_iter()
            .map(|s| Ok::<_, std::convert::Infallible>(Bytes::from(s))),
    );
    Response::builder()
        .status(200)
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap()
}

/// End-to-end proof of the fuzzy-constraint pipeline: a user states a rule in
/// prose ("must keep the tone formal") → the cue gate fires → the judge extracts
/// it as a judge-checkable constraint → the judge finds the informal reply
/// violates it → an AMBER `constraint` signal is attached off the response path.
#[tokio::test]
async fn judge_extracts_and_flags_fuzzy_constraint() {
    use drifterr_judge::{Judge, StubJudge};

    let mock = spawn(Router::new().fallback(mock_lol)).await;
    let base = format!("http://{mock}");
    let cfg = ProxyConfig {
        openai_upstream: base.clone(),
        anthropic_upstream: base,
        openai_strip_v1: false,
    };
    // Stub judge extracts one fuzzy constraint and flags any reply with "lol".
    let judge = Judge::Stub(StubJudge::new(&["lol"]).with_extracts(&["Keep the tone formal"]));
    let state = AppState::with_judge(cfg, None, judge);
    let proxy = spawn(proxy_router(state.clone())).await;
    let control = spawn(control_router(state)).await;
    let client = client();

    client
        .post(format!("http://{proxy}/v1/chat/completions"))
        .header("authorization", AUTH)
        .json(&serde_json::json!({
            "model": "gpt-4o",
            "stream": true,
            "messages": [{"role": "user", "content": "Draft the client email. You must keep the tone formal."}]
        }))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();

    // Poll until the async judge phase attaches an AMBER constraint signal from
    // the extracted fuzzy constraint (constraint id, not a deterministic span).
    let url = format!("http://{control}/status");
    let mut found = None;
    for _ in 0..150 {
        let v: serde_json::Value = client.get(&url).send().await.unwrap().json().await.unwrap();
        let cur = &v["current"];
        if !cur.is_null() {
            if let Some(sigs) = cur["signals"].as_array() {
                if let Some(s) = sigs
                    .iter()
                    .find(|s| s["signal"] == "constraint" && s["state"] == "amber")
                {
                    found = Some((cur.clone(), s.clone()));
                    break;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let (cur, sig) = found.expect("fuzzy constraint signal should appear");
    assert_eq!(
        cur["state"], "amber",
        "fuzzy constraint lifts green → amber"
    );
    assert!(sig["detail"]
        .as_str()
        .unwrap()
        .contains("Keep the tone formal"));
}

#[tokio::test]
async fn judge_flags_reintroduced_rejected_decision() {
    use drifterr_judge::{Judge, StubJudge};

    let mock = spawn(Router::new().fallback(mock_bcrypt)).await;
    let base = format!("http://{mock}");
    let cfg = ProxyConfig {
        openai_upstream: base.clone(),
        anthropic_upstream: base,
        openai_strip_v1: false,
    };
    // Stub judge says "yes" when the context mentions bcrypt — no network.
    let state = AppState::with_judge(cfg, None, Judge::Stub(StubJudge::new(&["bcrypt"])));
    let proxy = spawn(proxy_router(state.clone())).await;
    let control = spawn(control_router(state)).await;
    let client = client();

    // The user explicitly rejected bcrypt; the (mocked) reply reintroduces it.
    client
        .post(format!("http://{proxy}/v1/chat/completions"))
        .header("authorization", AUTH)
        .json(&serde_json::json!({
            "model": "gpt-4o",
            "stream": true,
            "messages": [{"role": "user", "content": "Add password hashing. Please don't use bcrypt."}]
        }))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();

    // Poll until the async judge phase attaches the decision-coherence signal.
    let url = format!("http://{control}/status");
    let mut found = None;
    for _ in 0..150 {
        let v: serde_json::Value = client.get(&url).send().await.unwrap().json().await.unwrap();
        let cur = &v["current"];
        if !cur.is_null() {
            if let Some(sigs) = cur["signals"].as_array() {
                if let Some(s) = sigs.iter().find(|s| s["signal"] == "decision_coherence") {
                    found = Some((cur.clone(), s.clone()));
                    break;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let (cur, sig) = found.expect("decision-coherence signal should appear");
    assert_eq!(
        cur["state"], "amber",
        "judge soft signal lifts green → amber"
    );
    assert_eq!(sig["state"], "amber");
    assert!(sig["detail"].as_str().unwrap().contains("bcrypt"));
}

#[tokio::test]
async fn reanchor_returns_snapshot_after_a_session_exists() {
    let (proxy, control) = bring_up().await;
    let client = client();

    // Drive one turn so a session + baseline exist.
    client
        .post(format!("http://{proxy}/v1/chat/completions"))
        .header("authorization", AUTH)
        .json(&serde_json::json!({
            "model": "gpt-4o",
            "stream": true,
            "messages": [{"role": "user", "content": "refactor auth in TS, no JS"}]
        }))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    wait_for_status(&client, control).await;

    let r: serde_json::Value = client
        .get(format!("http://{control}/reanchor"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let snapshot = r["snapshot"].as_str().unwrap();
    assert!(snapshot.contains("# Re-anchor"));
    assert!(snapshot.contains("TypeScript only, no JS files"));
    assert!(r["preamble"]
        .as_str()
        .unwrap()
        .contains("Binding constraints"));
}

#[tokio::test]
async fn public_serves_ui_assets_and_blocks_traversal() {
    let (_proxy, control) = bring_up().await;
    let client = client();

    // The fonts folder's README exists in-repo, so /public/fonts/README.md
    // resolves through the static handler (default ui_dir = repo path in tests).
    let ok = client
        .get(format!("http://{control}/public/fonts/README.md"))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);

    // Path traversal is rejected.
    let bad = client
        .get(format!("http://{control}/public/../Cargo.toml"))
        .send()
        .await
        .unwrap();
    assert!(bad.status() == 400 || bad.status() == 404);
}

#[tokio::test]
async fn reanchor_404_when_no_session() {
    let (_proxy, control) = bring_up().await;
    let client = client();
    let resp = client
        .get(format!("http://{control}/reanchor"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn standing_order_recurs_promotes_and_reappears() {
    // A store-backed control API (standing orders are durable). No upstream
    // needed — /ingest doesn't relay.
    let state = AppState::new(
        ProxyConfig::default(),
        Some(drifterr_store::Store::open_in_memory().unwrap()),
    );
    let control = spawn(control_router(state)).await;
    let client = client();

    async fn ingest(
        client: &reqwest::Client,
        control: SocketAddr,
        session: &str,
        user: &str,
        reply: &str,
    ) {
        client
            .post(format!("http://{control}/ingest"))
            .json(&serde_json::json!({
                "sessionId": session,
                "model": "claude-opus-4-x",
                "turns": [{"role":"user","content":user},{"role":"assistant","content":reply}]
            }))
            .send()
            .await
            .unwrap();
    }

    // The same constraint stated in three separate sessions.
    for s in ["s1", "s2", "s3"] {
        ingest(
            &client,
            control,
            s,
            "Refactor in TS, no JS",
            "creating app.ts",
        )
        .await;
    }

    // It should now be a promotion candidate.
    let orders: serde_json::Value = client
        .get(format!("http://{control}/standing-orders"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let cand = orders
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["candidate"] == true)
        .expect("a candidate after 3 occurrences");
    assert!(cand["occurrences"].as_i64().unwrap() >= 3);
    let id = cand["id"].as_i64().unwrap();

    // Accept it.
    let promoted = client
        .post(format!("http://{control}/standing-orders/promote"))
        .json(&serde_json::json!({ "id": id }))
        .send()
        .await
        .unwrap();
    assert_eq!(promoted.status(), 200);

    // A brand-new session that NEVER states the rule still gets it applied:
    // an app.js reply now violates the remembered constraint.
    ingest(
        &client,
        control,
        "s-fresh",
        "do something unrelated",
        "creating app.js",
    )
    .await;
    let status = wait_for_status(&client, control).await;
    assert_eq!(status["state"], "red");
    assert_eq!(status["triggering"]["signal"], "constraint");
}

#[tokio::test]
async fn auto_reanchor_injects_preamble_when_drifting() {
    // Mock upstream that records every request body and always replies with a
    // .js violation (to drive the session RED).
    let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let cap = captured.clone();
    let mock = spawn(Router::new().fallback(move |req: Request| {
        let cap = cap.clone();
        async move {
            let (_p, body) = req.into_parts();
            let bytes = axum::body::to_bytes(body, 1 << 20).await.unwrap_or_default();
            cap.lock().unwrap().push(String::from_utf8_lossy(&bytes).to_string());
            let chunks = vec![
                "data: {\"choices\":[{\"delta\":{\"content\":\"Sure, creating auth.js\"}}]}\n\n".to_string(),
                "data: {\"choices\":[{\"delta\":{}}],\"usage\":{\"prompt_tokens\":50,\"completion_tokens\":6}}\n\n".to_string(),
                "data: [DONE]\n\n".to_string(),
            ];
            let stream = futures_util::stream::iter(
                chunks.into_iter().map(|s| Ok::<_, std::convert::Infallible>(Bytes::from(s))),
            );
            Response::builder()
                .status(200)
                .header("content-type", "text/event-stream")
                .body(Body::from_stream(stream))
                .unwrap()
        }
    }))
    .await;

    let base = format!("http://{mock}");
    let cfg = ProxyConfig {
        openai_upstream: base.clone(),
        anthropic_upstream: base,
        openai_strip_v1: false,
    };
    // Auto-re-anchor is a paid capability, so the session must be on a plan that
    // unlocks it (Pro) for the injection to fire.
    let state = AppState::with_judge(cfg, None, drifterr_judge::Judge::Disabled)
        .with_auto_reanchor(true)
        .with_plan(drifterr_proxy::entitlement::Plan::Pro);
    let proxy = spawn(proxy_router(state.clone())).await;
    let control = spawn(control_router(state)).await;
    let client = client();

    let body = serde_json::json!({
        "model": "gpt-4o",
        "stream": true,
        "messages": [{"role": "user", "content": "refactor in TS, no JS"}]
    });

    // Turn 1: session not yet known ⇒ no injection; the reply drives it RED.
    client
        .post(format!("http://{proxy}/v1/chat/completions"))
        .header("authorization", AUTH)
        .json(&body)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let status = wait_for_status(&client, control).await;
    assert_eq!(status["state"], "red");

    // Turn 2: same session (same opening) ⇒ proxy injects the preamble.
    let body2 = serde_json::json!({
        "model": "gpt-4o",
        "stream": true,
        "messages": [
            {"role": "user", "content": "refactor in TS, no JS"},
            {"role": "assistant", "content": "ok"},
            {"role": "user", "content": "continue"}
        ]
    });
    client
        .post(format!("http://{proxy}/v1/chat/completions"))
        .header("authorization", AUTH)
        .json(&body2)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();

    let bodies = captured.lock().unwrap().clone();
    assert_eq!(bodies.len(), 2);
    assert!(
        !bodies[0].contains("Drifterr re-anchor"),
        "turn 1 relayed unchanged"
    );
    assert!(
        bodies[1].contains("Drifterr re-anchor"),
        "turn 2 should carry the injected re-anchor preamble"
    );
}

#[tokio::test]
async fn browser_ingest_feeds_the_engine() {
    let (_proxy, control) = bring_up().await;
    let client = client();

    // The extension posts scraped turns; a .js reply violates the TS-only intent.
    let resp = client
        .post(format!("http://{control}/ingest"))
        .json(&serde_json::json!({
            "sessionId": "chat-1",
            "model": "claude-opus-4-x",
            "turns": [
                {"role": "user", "content": "Refactor in TS, no JS"},
                {"role": "assistant", "content": "Sure, creating auth.js"}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let status = wait_for_status(&client, control).await;
    assert_eq!(status["state"], "red");
    assert_eq!(status["triggering"]["signal"], "constraint");
    assert_eq!(
        status["exact"], false,
        "browser channel saturation is estimated"
    );
}

#[tokio::test]
async fn control_serves_dashboard_with_cors() {
    let (_proxy, control) = bring_up().await;
    let client = client();

    let page = client
        .get(format!("http://{control}/"))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 200);
    assert_eq!(
        page.headers().get("access-control-allow-origin").unwrap(),
        "*"
    );
    let html = page.text().await.unwrap();
    assert!(html.contains("id=\"panel\""), "dashboard HTML served");

    let js = client
        .get(format!("http://{control}/app.js"))
        .send()
        .await
        .unwrap();
    assert!(js
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("javascript"));
}

#[tokio::test]
async fn control_exposes_effective_config() {
    let (_proxy, control) = bring_up().await;
    let client = client();
    let cfg: serde_json::Value = client
        .get(format!("http://{control}/config"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // bring_up points both upstreams at the mock; persistence is off (None).
    assert!(cfg["openaiUpstream"]
        .as_str()
        .unwrap()
        .starts_with("http://"));
    assert_eq!(cfg["persisted"], false);
    assert!(!cfg["version"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn missing_auth_is_relayed_untouched() {
    // The proxy must not invent auth; a 401 from upstream reaches the client.
    let (proxy, _control) = bring_up().await;
    let client = client();
    let resp = client
        .post(format!("http://{proxy}/v1/chat/completions"))
        .json(&serde_json::json!({"model":"gpt-4o","messages":[]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn plan_gates_drift_map_and_sessions() {
    // No upstream needed — we drive detection via the /ingest channel directly.
    let cfg = ProxyConfig {
        openai_upstream: "http://unused".into(),
        anthropic_upstream: "http://unused".into(),
        openai_strip_v1: false,
    };
    let state = AppState::with_judge(cfg, None, drifterr_judge::Judge::Disabled);
    let control = spawn(control_router(state)).await;
    let client = client();
    let ingest = format!("http://{control}/ingest");
    let status = format!("http://{control}/status");
    let entitlement = format!("http://{control}/entitlement");

    let turns = serde_json::json!([
        {"role": "user", "content": "build a CSV export, server-side only"},
        {"role": "assistant", "content": "here is the plan for the export"}
    ]);
    // Two sessions, each ingested twice so the drift map has history.
    for _ in 0..2 {
        for sid in ["s1", "s2"] {
            client
                .post(&ingest)
                .json(&serde_json::json!({"sessionId": sid, "model": "gpt-4o", "turns": turns}))
                .send()
                .await
                .unwrap();
        }
    }

    // Free (default): the drift map is locked (history withheld), but the core
    // loop is not — every live session stays tracked and visible. Gating *depth*
    // rather than *access* is the point; a new user must never meet a session
    // wall before they've seen a detection.
    let v: serde_json::Value = client
        .get(&status)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["entitlement"]["plan"], "free");
    assert_eq!(v["entitlement"]["driftMap"], false);
    assert_eq!(
        v["entitlement"]["historyDays"], 7,
        "Free's real limit is retention, not concurrency"
    );
    assert!(
        v["current"]["history"].as_array().unwrap().is_empty(),
        "Free must not expose drift-map history"
    );
    assert_eq!(
        v["sessions"].as_array().unwrap().len(),
        2,
        "Free tracks every live session"
    );
    assert_eq!(
        v["sessionsLocked"], 0,
        "nothing is locked away on Free any more"
    );

    // Upgrade to Pro: drift map + all sessions unlock.
    client
        .post(&entitlement)
        .json(&serde_json::json!({"plan": "pro"}))
        .send()
        .await
        .unwrap();
    let v: serde_json::Value = client
        .get(&status)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["entitlement"]["plan"], "pro");
    assert_eq!(v["entitlement"]["driftMap"], true);
    assert!(
        !v["current"]["history"].as_array().unwrap().is_empty(),
        "Pro exposes drift-map history"
    );
    assert_eq!(v["sessions"].as_array().unwrap().len(), 2);
    assert_eq!(v["sessionsLocked"], 0);
    assert!(
        v["entitlement"]["historyDays"].is_null(),
        "Pro lifts the retention limit"
    );
}

/// The local first-run trial grants Pro capabilities with no account, and lapses
/// back to Free on its own once the window closes.
#[tokio::test]
async fn local_trial_grants_pro_then_lapses() {
    let cfg = ProxyConfig {
        openai_upstream: "http://unused".into(),
        anthropic_upstream: "http://unused".into(),
        openai_strip_v1: false,
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    const DAY: i64 = 86_400_000;

    // Fresh install, nobody signed in, trial started today.
    let state = AppState::with_judge(cfg.clone(), None, drifterr_judge::Judge::Disabled)
        .with_trial_started(Some(now));
    let control = spawn(control_router(state)).await;
    let client = client();
    let v: serde_json::Value = client
        .get(format!("http://{control}/entitlement"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["plan"], "trial", "a new install is on trial, signed out");
    assert_eq!(v["driftMap"], true);
    assert_eq!(v["autoReanchor"], true);
    assert_eq!(v["trialDaysLeft"], 14);

    // Same install, 20 days later: back to Free, no server involved.
    let lapsed = AppState::with_judge(cfg, None, drifterr_judge::Judge::Disabled)
        .with_trial_started(Some(now - 20 * DAY));
    let control = spawn(control_router(lapsed)).await;
    let v: serde_json::Value = client
        .get(format!("http://{control}/entitlement"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["plan"], "free");
    assert_eq!(v["driftMap"], false);
    assert_eq!(v["autoReanchor"], false);
    assert!(
        v["trialDaysLeft"].is_null() || v["trialDaysLeft"] == 0,
        "an expired trial must not advertise days left"
    );
}

#[tokio::test]
async fn provider_selector_switches_upstream() {
    let cfg = ProxyConfig {
        openai_upstream: "http://unused".into(),
        anthropic_upstream: "http://unused".into(),
        openai_strip_v1: false,
    };
    let state = AppState::with_judge(cfg, None, drifterr_judge::Judge::Disabled);
    let control = spawn(control_router(state)).await;
    let client = client();

    // The registry lists the major providers (incl. Gemini) + a current id.
    let v: serde_json::Value = client
        .get(format!("http://{control}/providers"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ids: Vec<&str> = v["providers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"openai") && ids.contains(&"gemini") && ids.contains(&"anthropic"));

    // Switch to OpenAI → /config reflects it live.
    client
        .post(format!("http://{control}/provider"))
        .json(&serde_json::json!({"id": "openai"}))
        .send()
        .await
        .unwrap();
    let c: serde_json::Value = client
        .get(format!("http://{control}/config"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(c["provider"], "OpenAI");
    assert!(c["openaiUpstream"]
        .as_str()
        .unwrap()
        .contains("api.openai.com"));

    // Unknown provider is rejected.
    let r = client
        .post(format!("http://{control}/provider"))
        .json(&serde_json::json!({"id": "nope"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
}

#[tokio::test]
async fn auto_intent_infers_goal_through_the_proxy() {
    use drifterr_judge::{InferredIntent, Judge, StubJudge};

    let mock = spawn(Router::new().fallback(mock_handler)).await;
    let base = format!("http://{mock}");
    let cfg = ProxyConfig {
        openai_upstream: base.clone(),
        anthropic_upstream: base,
        openai_strip_v1: false,
    };
    let judge = Judge::Stub(StubJudge::new(&[]).with_synth(InferredIntent {
        goal: "Refactor the auth module to argon2".into(),
        constraints: vec!["Keep the tests green".into()],
    }));
    let state = AppState::with_judge(cfg, None, judge).with_auto_intent(true);
    let proxy = spawn(proxy_router(state.clone())).await;
    let control = spawn(control_router(state)).await;
    let client = client();

    // Two turns of the SAME session (identical first user message = the anchor),
    // so Auto-intent's rate limiter reaches its ≥2-turn threshold and fires.
    for msgs in [
        serde_json::json!([{ "role": "user", "content": "start the auth refactor" }]),
        serde_json::json!([
            { "role": "user", "content": "start the auth refactor" },
            { "role": "assistant", "content": "ok" },
            { "role": "user", "content": "keep going" }
        ]),
    ] {
        client
            .post(format!("http://{proxy}/v1/chat/completions"))
            .header("authorization", AUTH)
            .json(&serde_json::json!({ "model": "gpt-4o", "stream": true, "messages": msgs }))
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
    }

    // The inferred goal should replace the weak first-message heuristic.
    let url = format!("http://{control}/intent");
    let mut goal = String::new();
    for _ in 0..150 {
        let v: serde_json::Value = client.get(&url).send().await.unwrap().json().await.unwrap();
        goal = v["goal"].as_str().unwrap_or("").to_string();
        if goal.contains("argon2") {
            // Constraints inferred too, as fuzzy (judge-checkable).
            let has = v["constraints"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .any(|c| c["text"].as_str() == Some("Keep the tests green"))
                })
                .unwrap_or(false);
            assert!(has, "inferred constraint is present");
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        goal.contains("argon2"),
        "Auto-intent set the inferred goal; got {goal:?}"
    );
}

#[tokio::test]
async fn judge_can_be_configured_at_runtime() {
    // Boot with the judge disabled (no env key).
    let state = AppState::with_judge(
        ProxyConfig::default(),
        None,
        drifterr_judge::Judge::Disabled,
    );
    let control = spawn(control_router(state)).await;
    let client = client();

    // Disabled by default.
    let j: serde_json::Value = client
        .get(format!("http://{control}/judge"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(j["enabled"], false);
    assert_eq!(j["label"], "disabled");

    // Provide a key + model → judge turns on, label is the model (key never echoed).
    let set: serde_json::Value = client
        .post(format!("http://{control}/judge"))
        .json(&serde_json::json!({"apiKey": "sk-or-test", "model": "openai/gpt-4o-mini"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(set["enabled"], true);
    assert_eq!(set["label"], "openai/gpt-4o-mini");
    assert!(set.get("apiKey").is_none(), "key is never returned");

    // /config reflects the live judge, not the boot value.
    let c: serde_json::Value = client
        .get(format!("http://{control}/config"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(c["judge"], "openai/gpt-4o-mini");

    // Empty key disables it again.
    let off: serde_json::Value = client
        .post(format!("http://{control}/judge"))
        .json(&serde_json::json!({"apiKey": ""}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(off["enabled"], false);
}

/// The MCP server against a live control API.
///
/// This is the prevention path, so what matters is that an agent asking "what are the
/// rules?" gets the *live* answer — the goal the user typed a moment ago and the approaches
/// they rejected mid-session — and that self-checking gives the same verdict the engine
/// would have raised afterwards. A stale or empty anchor here is worse than no MCP server,
/// because the agent would act on it confidently.
#[tokio::test]
async fn mcp_tools_read_the_live_anchor_and_agree_with_the_engine() {
    let cfg = ProxyConfig {
        openai_upstream: "http://unused".into(),
        anthropic_upstream: "http://unused".into(),
        openai_strip_v1: false,
    };
    let state = AppState::with_judge(cfg, None, drifterr_judge::Judge::Disabled);
    let control = spawn(control_router(state)).await;
    let client = client();
    let api = format!("http://{control}");

    // A session with a rejected decision stated in it, which is exactly the thing an agent
    // reintroduces a dozen turns later having forgotten.
    let turns = serde_json::json!([
        {"role": "user", "content": "Add a CSV export to the reports page. Don't touch package.json, and no new dependencies."},
        {"role": "assistant", "content": "Understood."},
        {"role": "user", "content": "No, we're not going to use lodash for this."}
    ]);
    client
        .post(format!("{api}/ingest"))
        .json(&serde_json::json!({"sessionId": "mcp-1", "model": "gpt-4o", "turns": turns}))
        .send()
        .await
        .unwrap();

    let anchor = drifterr_proxy::mcp::fetch_anchor(&api).await;
    assert!(
        !anchor.constraints.is_empty(),
        "the anchor must carry the constraints the user stated: {anchor:?}"
    );
    assert!(
        anchor
            .constraints
            .iter()
            .any(|c| c.text.to_lowercase().contains("package.json")),
        "constraints: {:?}",
        anchor.constraints
    );
    assert!(
        anchor
            .constraints
            .iter()
            .filter(|c| c.text.to_lowercase().contains("package.json"))
            .all(|c| c.rule.is_some()),
        "the rule must survive the control API, or the self-check silently checks less"
    );

    let ask = |method: &str, params: serde_json::Value| {
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string()
    };
    let text_of = |resp: String| -> String {
        let v: serde_json::Value = serde_json::from_str(&resp).unwrap();
        assert!(v.get("error").is_none(), "unexpected error: {v}");
        v["result"]["content"][0]["text"].as_str().unwrap().into()
    };

    // The anchor tool restates what the user asked for.
    let text = text_of(
        drifterr_proxy::mcp::handle(
            &ask(
                "tools/call",
                serde_json::json!({"name":"drifterr_anchor","arguments":{}}),
            ),
            &anchor,
        )
        .unwrap(),
    );
    assert!(text.contains("package.json"), "{text}");

    // And the checker refuses the work that would have broken it — the same verdict the
    // engine raises after the fact, one turn earlier.
    let text = text_of(
        drifterr_proxy::mcp::handle(
            &ask(
                "tools/call",
                serde_json::json!({
                    "name": "drifterr_check",
                    "arguments": {"content": "```diff\n--- a/package.json\n+++ b/package.json\n@@\n+  \"csv\": \"^1\"\n```"}
                }),
            ),
            &anchor,
        )
        .unwrap(),
    );
    assert!(text.starts_with("VIOLATION"), "{text}");
    assert!(text.contains("package.json"), "{text}");

    // With Drifterr not running, the tools must say nothing is pinned rather than report
    // an empty rule set as a clean bill of health.
    let none = drifterr_proxy::mcp::fetch_anchor("http://127.0.0.1:1").await;
    assert!(none.goal.is_empty() && none.constraints.is_empty());
    let text = text_of(
        drifterr_proxy::mcp::handle(
            &ask(
                "tools/call",
                serde_json::json!({"name":"drifterr_anchor","arguments":{}}),
            ),
            &none,
        )
        .unwrap(),
    );
    assert!(text.contains("No intent is currently pinned"), "{text}");
    let text = text_of(
        drifterr_proxy::mcp::handle(
            &ask(
                "tools/call",
                serde_json::json!({"name":"drifterr_check","arguments":{"content":"anything"}}),
            ),
            &none,
        )
        .unwrap(),
    );
    assert!(text.contains("NOT a pass"), "{text}");
}

/// The Claude Code hook against a live control API.
///
/// Covers the property that matters most: the hook sits in the path of a keypress, so
/// it must inject only when it genuinely should and stay silent in every other case,
/// including when Drifterr isn't running at all.
#[tokio::test]
async fn claude_code_hook_injects_only_on_a_drifting_pro_session() {
    use drifterr_proxy::hook::{decide, render, Decision, HookInput};

    let cfg = ProxyConfig {
        openai_upstream: "http://unused".into(),
        anthropic_upstream: "http://unused".into(),
        openai_strip_v1: false,
    };
    // Pro, so automatic injection is entitled.
    let state = AppState::with_judge(cfg, None, drifterr_judge::Judge::Disabled)
        .with_plan(drifterr_proxy::entitlement::Plan::Pro);
    let control = spawn(control_router(state)).await;
    let client = client();
    let api = format!("http://{control}");

    // Drive a constraint violation through the ingest channel so the session is RED.
    let turns = serde_json::json!([
        {"role": "user", "content": "Refactor the auth module in TypeScript, no JS"},
        {"role": "assistant", "content": "Sure, creating auth.js now"}
    ]);
    client
        .post(format!("{api}/ingest"))
        .json(&serde_json::json!({"sessionId": "hook-1", "model": "gpt-4o", "turns": turns}))
        .send()
        .await
        .unwrap();

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
        "fixture should be drifting"
    );

    // Read the id back rather than hard-coding it: the ingest channel namespaces the
    // ids it is given, and the hook should follow whatever /status reports.
    let sid = status["current"]["sessionId"].as_str().unwrap().to_string();
    let reanchor: serde_json::Value = client
        .get(format!("{api}/reanchor?session={sid}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let preamble = reanchor["preamble"].as_str().unwrap();
    assert!(!preamble.is_empty());

    // Red + entitled + a preamble ⇒ inject, and the output is the JSON Claude Code
    // expects with the preamble inside it.
    let d = decide(&status, Some(preamble));
    assert!(matches!(d, Decision::Inject(_)), "should inject: {d:?}");
    let out = render(&d);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "UserPromptSubmit");
    assert!(v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .contains("[Drifterr]"));

    // Fetching the re-anchor opened the verification window, so an automatic
    // re-anchor gets measured exactly like a manual one.
    let after: serde_json::Value = client
        .get(format!("{api}/status"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        after["current"]["reanchor"]["signal"], "constraint",
        "the hook's re-anchor must be tracked: {after}"
    );

    // With Drifterr not running at all, the hook produces nothing and cannot fail.
    let out = drifterr_proxy::hook::run(
        "http://127.0.0.1:1",
        &HookInput {
            session_id: sid.clone(),
            cwd: String::new(),
        },
        false,
    )
    .await;
    assert!(
        out.is_empty(),
        "an unreachable Drifterr must inject nothing"
    );
}

/// Rule packs end to end: list, apply, export, and round-trip through a rules file.
#[tokio::test]
async fn rule_packs_apply_and_export() {
    let cfg = ProxyConfig {
        openai_upstream: "http://unused".into(),
        anthropic_upstream: "http://unused".into(),
        openai_strip_v1: false,
    };
    let state = AppState::with_judge(cfg, None, drifterr_judge::Judge::Disabled);
    let control = spawn(control_router(state)).await;
    let client = client();
    let api = format!("http://{control}");

    // The catalogue states up front how much of each pack is actually enforceable.
    let packs: serde_json::Value = client
        .get(format!("{api}/packs"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let list = packs.as_array().unwrap();
    assert!(list.len() >= 3, "built-in packs are listed");
    for p in list {
        assert!(
            p["advisory"].as_array().unwrap().is_empty(),
            "a curated pack must be fully enforceable: {p}"
        );
        assert!(p["enforceable"].as_u64().unwrap() > 0);
    }

    // Applying a pack seeds the intent, so the rules govern the next session.
    let applied: serde_json::Value = client
        .post(format!("{api}/packs/apply"))
        .json(&serde_json::json!({"id": "typescript-strict"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(applied["applied"], 4);
    assert!(applied["advisory"].as_array().unwrap().is_empty());

    let intent: serde_json::Value = client
        .get(format!("{api}/intent"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let texts: Vec<String> = intent["constraints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["text"].as_str().unwrap_or("").to_string())
        .collect();
    assert!(
        texts.iter().any(|t| t.contains("any")),
        "pack rules reached the anchor: {texts:?}"
    );

    // An unknown pack id is a 404, not a silent no-op.
    let missing = client
        .post(format!("{api}/packs/apply"))
        .json(&serde_json::json!({"id": "no-such-pack"}))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);

    // An inline pack from outside is untrusted input and must be validated.
    let bad = client
        .post(format!("{api}/packs/apply"))
        .json(&serde_json::json!({"pack": {"drifterrPack": 99, "name": "Future", "rules": []}}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        bad.status(),
        400,
        "a newer schema is refused, not guessed at"
    );

    // Export renders markdown for a rules file — the cross-tool direction.
    let md = client
        .get(format!("{api}/packs/export?markdown=true&name=Mine"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        md.contains("drifterr:begin"),
        "managed markers present: {md}"
    );
    assert!(md.contains("## Mine"));
}
