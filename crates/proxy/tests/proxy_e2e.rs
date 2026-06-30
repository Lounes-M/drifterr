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

    // Free (default): drift map locked (history withheld) and sessions capped at 1.
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
    assert!(
        v["current"]["history"].as_array().unwrap().is_empty(),
        "Free must not expose drift-map history"
    );
    assert_eq!(
        v["sessions"].as_array().unwrap().len(),
        1,
        "Free caps tracked sessions at 1"
    );
    assert_eq!(
        v["sessionsLocked"], 1,
        "the other session is reported as locked"
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
    assert_eq!(
        v["sessions"].as_array().unwrap().len(),
        2,
        "Pro lifts the session cap"
    );
    assert_eq!(v["sessionsLocked"], 0);
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
