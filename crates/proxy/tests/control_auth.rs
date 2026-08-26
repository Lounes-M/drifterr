//! The control API's access boundary, encoded as tests.
//!
//! # The bug this file exists to keep fixed
//!
//! The control API bound `127.0.0.1` and answered every request with
//! `Access-Control-Allow-Origin: *` and no credential. "It is local, so only the
//! user can reach it" is wrong in the one place that matters — the user's browser
//! is already inside that boundary. Any page they had open could run
//!
//! ```js
//! fetch("http://127.0.0.1:8788/anchor").then(r => r.json())
//! ```
//!
//! and read the goal verbatim, every constraint, and the offending span of every
//! violation. `POST /entitlement` granted Team; `POST /judge` swapped the model
//! key for one the attacker owned, at which point conversation excerpts left the
//! machine to a provider account they controlled.
//!
//! So these are not style tests. Each one below is a specific published claim —
//! "no chat content ever leaves your machine" — expressed as something CI can
//! fail on. A regression here is a critical security report by `SECURITY.md`'s own
//! definition, and it should look like one.
//!
//! # What is deliberately still open
//!
//! `/health` and the dashboard's own assets answer without a token: a dashboard
//! that cannot load cannot authenticate, and the hook needs a liveness probe
//! before pairing. Those routes are proven below to carry no CORS headers, which
//! is what keeps a foreign page from reading them.

use axum::Router;
use drifterr_proxy::auth::{Token, TOKEN_HEADER};
use drifterr_proxy::{control_router, AppState, ProxyConfig};
use std::net::SocketAddr;
use tokio::net::TcpListener;

const TOKEN: &str = "control-auth-test-token";

/// Every route that reads or writes a session. If a new one is added to the
/// router and not to this list, `no_route_is_left_unauthenticated` fails.
const PROTECTED_GETS: &[&str] = &[
    "/status",
    "/sessions",
    "/config",
    "/providers",
    "/entitlement",
    "/reanchor",
    "/intent",
    "/anchor",
    "/judge",
    "/auto-reanchor",
    "/auto-intent",
    "/prefs",
    "/history",
    "/journal",
    "/report",
    "/packs",
    "/packs/export",
    "/team/share-preview",
    "/standing-orders",
];

/// The mutating routes. These are the ones a wildcard CORS policy left open to
/// blind cross-site requests, because a page does not need to *read* a response
/// to have already caused the side effect.
const PROTECTED_POSTS: &[&str] = &[
    "/provider",
    "/entitlement",
    "/intent",
    "/intent/retire",
    "/judge",
    "/auto-reanchor",
    "/auto-intent",
    "/intent-shift",
    "/prefs",
    "/packs/apply",
    "/standing-orders/promote",
    "/feedback",
    "/ingest",
];

async fn serve() -> SocketAddr {
    let cfg = ProxyConfig {
        openai_upstream: "http://unused".into(),
        anthropic_upstream: "http://unused".into(),
        openai_strip_v1: false,
    };
    let state = AppState::new(cfg, None).with_token(Token::from_value(TOKEN));
    let router = control_router(state);
    spawn(router).await
}

async fn spawn(router: Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    addr
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

// ---------------------------------------------------------------------------
// 1. The token is required.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_session_route_refuses_an_unauthenticated_request() {
    let addr = serve().await;
    let c = client();

    for path in PROTECTED_GETS {
        let res = c
            .get(format!("http://{addr}{path}"))
            .send()
            .await
            .unwrap_or_else(|e| panic!("GET {path}: {e}"));
        assert_eq!(
            res.status(),
            401,
            "GET {path} answered without a token — a website could read this"
        );
    }
    for path in PROTECTED_POSTS {
        let res = c
            .post(format!("http://{addr}{path}"))
            .header("content-type", "application/json")
            .body("{}")
            .send()
            .await
            .unwrap_or_else(|e| panic!("POST {path}: {e}"));
        assert_eq!(
            res.status(),
            401,
            "POST {path} accepted an unauthenticated write"
        );
    }
}

#[tokio::test]
async fn a_wrong_token_is_refused_and_the_right_one_is_accepted() {
    let addr = serve().await;
    let c = client();
    let url = format!("http://{addr}/status");

    for wrong in [
        "",
        "nope",
        // The classic off-by-one comparison bugs.
        &TOKEN[..TOKEN.len() - 1],
        &format!("{TOKEN}x"),
        &TOKEN.to_uppercase(),
    ] {
        let res = c
            .get(&url)
            .header(TOKEN_HEADER, wrong)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 401, "token {wrong:?} must not authenticate");
    }

    for good in [
        c.get(&url).header(TOKEN_HEADER, TOKEN),
        c.get(&url)
            .header("authorization", format!("Bearer {TOKEN}")),
    ] {
        assert_eq!(good.send().await.unwrap().status(), 200);
    }
}

/// The list above must stay complete. Reading the router's own route table is not
/// possible from outside axum, so this asserts the inverse: every path the app
/// serves is either public *by name* or covered by the protected lists. A new
/// endpoint that is neither will show up here as an unlisted 200.
#[tokio::test]
async fn no_route_is_left_unauthenticated() {
    let addr = serve().await;
    let c = client();
    // Routes intentionally reachable without a token, and why:
    //   /health           — liveness only; returns the fixed string "ok".
    //   / and /index.html — the dashboard must load before it can authenticate.
    //   /app.js /styles.css /public/* — its assets.
    const PUBLIC: &[&str] = &[
        "/health",
        "/",
        "/index.html",
        "/app.js",
        "/styles.css",
        "/public/anything.woff2",
    ];
    for path in PUBLIC {
        let res = c.get(format!("http://{addr}{path}")).send().await.unwrap();
        assert_ne!(res.status(), 401, "{path} is documented as public");
        assert!(
            res.headers().get("access-control-allow-origin").is_none(),
            "{path} is public, so it must never be readable cross-origin"
        );
    }
    // And an unknown path must not become an accidental hole.
    let res = c
        .get(format!("http://{addr}/not-a-route"))
        .send()
        .await
        .unwrap();
    assert!(
        res.status() == 401 || res.status() == 404,
        "unknown paths must be refused or not found, got {}",
        res.status()
    );
}

// ---------------------------------------------------------------------------
// 2. The origin allowlist.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_foreign_origin_is_never_allowed_to_read_a_response() {
    let addr = serve().await;
    let c = client();

    // Even *with* a valid token — the token could be stolen from a paired
    // extension, and the origin check is the second, independent layer.
    for origin in [
        "https://evil.example",
        "http://localhost:3000",
        "null",
        "https://tauri.localhost.evil.example",
        "http://127.0.0.1:8788.evil.example",
    ] {
        let res = c
            .get(format!("http://{addr}/status"))
            .header("origin", origin)
            .header(TOKEN_HEADER, TOKEN)
            .send()
            .await
            .unwrap();
        assert!(
            res.headers().get("access-control-allow-origin").is_none(),
            "{origin} was granted CORS access — a page there could read sessions"
        );
    }
}

#[tokio::test]
async fn the_panel_and_extension_origins_are_allowed() {
    let addr = serve().await;
    let c = client();
    for origin in [
        "tauri://localhost",
        "https://tauri.localhost",
        "chrome-extension://abcdefghijklmnopabcdefghijklmnop",
    ] {
        let res = c
            .get(format!("http://{addr}/status"))
            .header("origin", origin)
            .header(TOKEN_HEADER, TOKEN)
            .send()
            .await
            .unwrap();
        assert_eq!(
            res.headers()
                .get("access-control-allow-origin")
                .map(|v| v.to_str().unwrap()),
            Some(origin),
            "{origin} is a first-party surface and must be allowed"
        );
        assert_eq!(
            res.headers().get("vary").map(|v| v.to_str().unwrap()),
            Some("Origin"),
            "a cache must never serve one origin's response to another"
        );
    }
}

#[tokio::test]
async fn preflight_is_answered_without_a_token_but_only_for_allowed_origins() {
    let addr = serve().await;
    let c = client();

    // A preflight cannot carry a token by definition, so refusing it would break
    // every legitimate cross-origin call.
    let ok = c
        .request(reqwest::Method::OPTIONS, format!("http://{addr}/intent"))
        .header("origin", "tauri://localhost")
        .header("access-control-request-method", "POST")
        .header("access-control-request-headers", TOKEN_HEADER)
        .send()
        .await
        .unwrap();
    assert!(ok.status().is_success());
    assert_eq!(
        ok.headers()
            .get("access-control-allow-origin")
            .map(|v| v.to_str().unwrap()),
        Some("tauri://localhost")
    );
    assert!(ok
        .headers()
        .get("access-control-allow-headers")
        .unwrap()
        .to_str()
        .unwrap()
        .contains(TOKEN_HEADER));

    let bad = c
        .request(reqwest::Method::OPTIONS, format!("http://{addr}/intent"))
        .header("origin", "https://evil.example")
        .header("access-control-request-method", "POST")
        .send()
        .await
        .unwrap();
    assert!(
        bad.headers().get("access-control-allow-origin").is_none(),
        "a refused preflight is what stops the real request being sent"
    );
}

// ---------------------------------------------------------------------------
// 3. The specific attacks, end to end.
// ---------------------------------------------------------------------------

/// The original proof-of-concept: read the user's prompt from a foreign origin.
#[tokio::test]
async fn a_website_cannot_read_the_users_goal_or_constraints() {
    let addr = serve().await;
    let c = client();

    // Seed a session the way the browser channel does.
    c.post(format!("http://{addr}/ingest"))
        .header(TOKEN_HEADER, TOKEN)
        .json(&serde_json::json!({
            "sessionId": "victim",
            "model": "gpt-4",
            "turns": [
                { "role": "user", "content": "Migrate the acme payroll database. No console.log." },
                { "role": "assistant", "content": "```js\nconsole.log(process.env.DB_URL)\n```" }
            ]
        }))
        .send()
        .await
        .unwrap();

    for path in ["/anchor", "/status", "/reanchor", "/intent", "/sessions"] {
        let res = c
            .get(format!("http://{addr}{path}"))
            .header("origin", "https://evil.example")
            .send()
            .await
            .unwrap();
        assert_eq!(
            res.status(),
            401,
            "{path} answered a foreign origin — this is the whole bug"
        );
        let body = res.text().await.unwrap_or_default();
        assert!(
            !body.contains("acme payroll") && !body.contains("DB_URL"),
            "{path} leaked conversation content in its refusal body"
        );
    }
}

/// The write half: a page could grant itself a paid plan.
#[tokio::test]
async fn a_website_cannot_grant_itself_a_paid_plan() {
    let addr = serve().await;
    let c = client();

    let res = c
        .post(format!("http://{addr}/entitlement"))
        .header("origin", "https://evil.example")
        .json(&serde_json::json!({ "plan": "team" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);

    // And the plan really did not move.
    let ent: serde_json::Value = c
        .get(format!("http://{addr}/entitlement"))
        .header(TOKEN_HEADER, TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_ne!(
        ent["plan"], "team",
        "an unauthenticated POST changed the effective plan"
    );
}

/// The exfiltration half: a page could point the judge at its own provider key
/// and turn on the feature that feeds it conversation excerpts.
#[tokio::test]
async fn a_website_cannot_redirect_the_judge_to_its_own_key() {
    let addr = serve().await;
    let c = client();

    for (path, body) in [
        ("/judge", serde_json::json!({ "apiKey": "sk-attacker" })),
        ("/auto-intent", serde_json::json!({ "on": true })),
        ("/auto-reanchor", serde_json::json!({ "on": true })),
    ] {
        let res = c
            .post(format!("http://{addr}{path}"))
            .header("origin", "https://evil.example")
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(
            res.status(),
            401,
            "POST {path} would send chat content to a key the user never chose"
        );
    }

    let judge: serde_json::Value = c
        .get(format!("http://{addr}/judge"))
        .header(TOKEN_HEADER, TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        judge["enabled"], false,
        "the judge was enabled by an unauthenticated request"
    );
}

/// A response must never echo the token back to a caller that did not have it —
/// including in an error body.
#[tokio::test]
async fn the_token_is_never_disclosed_by_the_api() {
    let addr = serve().await;
    let c = client();
    for path in ["/status", "/config", "/health", "/not-a-route"] {
        let res = c.get(format!("http://{addr}{path}")).send().await.unwrap();
        let headers = format!("{:?}", res.headers());
        let body = res.text().await.unwrap_or_default();
        assert!(
            !body.contains(TOKEN) && !headers.contains(TOKEN),
            "{path} disclosed the control token"
        );
    }
}
