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
