//! # Drifterr proxy channel
//!
//! A local API proxy: the user points their tool at it, it relays every request
//! to the real provider **transparently**, and — off the response path — it
//! reconstructs each assistant turn and runs the engine. This is the only
//! channel that yields *exact* saturation, because it sees the real `messages`
//! array and the provider's reported token usage.
//!
//! ## The non-negotiable rule
//!
//! Never buffer the response before relaying it — that would break SSE
//! streaming, the product's #1 hard point. Instead the upstream byte stream is
//! forwarded to the client **unchanged**, while a cheap [`tee`](proxy_handler)
//! (cloned reference-counted `Bytes`) feeds a background task that does the
//! parsing and detection after the stream ends. Client-added latency ≈ 0.
//!
//! Two listeners run side by side:
//! * the **proxy** (catch-all) — the transparent relay;
//! * the **control API** — `GET /status` etc., consumed by the future menubar.
//!
//! Keeping them on separate ports means the status contract can never collide
//! with a proxied path.

pub mod dashboard;
pub mod entitlement;
pub mod provider;
pub mod state;

use axum::body::Body;
use axum::extract::{Path as AxPath, Query, Request, State};
use axum::http::{header, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bytes::Bytes;
use entitlement::{Entitlement, Plan};
use futures_util::StreamExt;
use provider::Provider;
use serde::Deserialize;
use serde::Serialize;
use state::{AppCore, SessionStatus};
use std::future::IntoFuture;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use tokio::net::TcpListener;

/// Largest request body we will read into memory before relaying (64 MiB).
const MAX_BODY: usize = 64 * 1024 * 1024;

/// Where to relay each provider's traffic. Defaults to the public endpoints;
/// override (e.g. to OpenRouter or a local server) for other backends.
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    pub openai_upstream: String,
    pub anthropic_upstream: String,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            openai_upstream: "https://api.openai.com".to_string(),
            anthropic_upstream: "https://api.anthropic.com".to_string(),
        }
    }
}

impl ProxyConfig {
    fn upstream_for(&self, provider: Provider) -> &str {
        match provider {
            Provider::OpenAI => &self.openai_upstream,
            Provider::Anthropic => &self.anthropic_upstream,
        }
    }
}

/// Effective configuration, exposed read-only at `GET /config` for the UI's
/// settings view and for diagnostics. (Editable judge/privacy settings arrive
/// with the judge milestone; this is the foundation they'll extend.)
#[derive(Debug, Clone, Serialize)]
pub struct ConfigMeta {
    pub version: &'static str,
    #[serde(rename = "openaiUpstream")]
    pub openai_upstream: String,
    #[serde(rename = "anthropicUpstream")]
    pub anthropic_upstream: String,
    /// Whether sessions are persisted to SQLite (vs in-memory only).
    pub persisted: bool,
    /// Judge backend label ("disabled", a model id, or "stub").
    pub judge: String,
    /// Whether opt-in auto-re-anchor is active.
    #[serde(rename = "autoReanchor")]
    pub auto_reanchor: bool,
}

/// Shared, cheaply-clonable application state.
#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<ProxyConfig>,
    pub client: reqwest::Client,
    pub core: Arc<Mutex<AppCore>>,
    pub meta: Arc<ConfigMeta>,
    pub judge: Arc<drifterr_judge::Judge>,
    /// Opt-in: inject the re-anchor preamble into outgoing requests when the
    /// session is drifting (RED). Off by default — it modifies user requests.
    pub auto_reanchor: bool,
    /// Current plan entitlement, set by the desktop app after login. Defaults to
    /// Free so the proxy works standalone. Gates paid capabilities (drift map,
    /// extra sessions, auto-re-anchor).
    pub entitlement: Arc<RwLock<Entitlement>>,
}

impl AppState {
    /// Build state with a config and an optional durable store. Judge and
    /// auto-re-anchor are configured from the environment.
    pub fn new(cfg: ProxyConfig, store: Option<drifterr_store::Store>) -> Self {
        let auto = matches!(
            std::env::var("DRIFTERR_AUTO_REANCHOR").as_deref(),
            Ok("1") | Ok("true") | Ok("yes")
        );
        Self::build(cfg, store, drifterr_judge::Judge::from_env(), auto)
    }

    /// Build state with an explicit judge (used by tests). Auto-re-anchor off.
    pub fn with_judge(
        cfg: ProxyConfig,
        store: Option<drifterr_store::Store>,
        judge: drifterr_judge::Judge,
    ) -> Self {
        Self::build(cfg, store, judge, false)
    }

    /// Enable/disable auto-re-anchor (used by tests).
    pub fn with_auto_reanchor(mut self, on: bool) -> Self {
        self.auto_reanchor = on;
        self
    }

    /// Set the active plan entitlement (used by tests and the app embedder).
    pub fn with_plan(self, plan: Plan) -> Self {
        if let Ok(mut w) = self.entitlement.write() {
            *w = Entitlement::for_plan(plan);
        }
        self
    }

    fn build(
        cfg: ProxyConfig,
        store: Option<drifterr_store::Store>,
        judge: drifterr_judge::Judge,
        auto_reanchor: bool,
    ) -> Self {
        // `.no_proxy()`: talk to providers directly. Any ambient HTTP(S)_PROXY
        // belongs to the surrounding shell, not to a localhost dev proxy.
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("reqwest client");
        let meta = ConfigMeta {
            version: env!("CARGO_PKG_VERSION"),
            openai_upstream: cfg.openai_upstream.clone(),
            anthropic_upstream: cfg.anthropic_upstream.clone(),
            persisted: store.is_some(),
            judge: judge.label(),
            auto_reanchor,
        };
        Self {
            cfg: Arc::new(cfg),
            client,
            core: Arc::new(Mutex::new(AppCore::new(store))),
            meta: Arc::new(meta),
            judge: Arc::new(judge),
            auto_reanchor,
            entitlement: Arc::new(RwLock::new(Entitlement::default())),
        }
    }

    /// Read the current entitlement (Free if the lock is poisoned).
    pub fn entitlement(&self) -> Entitlement {
        self.entitlement.read().map(|e| *e).unwrap_or_default()
    }
}

/// Open a durable SQLite store at `path`, or `None` on failure. Convenience for
/// embedders (e.g. the Tauri app) so they don't depend on `drifterr-store`.
pub fn open_store(path: &str) -> Option<drifterr_store::Store> {
    drifterr_store::Store::open(path).ok()
}

/// The transparent relay: a catch-all over every path and method.
pub fn proxy_router(state: AppState) -> Router {
    Router::new().fallback(proxy_handler).with_state(state)
}

/// The control API the UI reads, plus the built-in dashboard that serves the
/// menubar panel's assets. CORS is permissive: this listener is localhost-only,
/// and the browser-extension channel and any web dashboard need to reach it
/// cross-origin.
pub fn control_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(dashboard::index))
        .route("/index.html", get(dashboard::index))
        .route("/styles.css", get(dashboard::styles))
        .route("/app.js", get(dashboard::app_js))
        .route("/status", get(status_handler))
        .route("/sessions", get(sessions_handler))
        .route("/config", get(config_handler))
        .route(
            "/entitlement",
            get(entitlement_handler).post(set_entitlement_handler),
        )
        .route("/reanchor", get(reanchor_handler))
        .route("/standing-orders", get(standing_orders_handler))
        .route("/standing-orders/promote", post(promote_handler))
        .route("/ingest", post(ingest_handler))
        .route("/public/{*path}", get(public_handler))
        .route("/health", get(|| async { "ok" }))
        .layer(middleware::from_fn(add_cors))
        .with_state(state)
}

/// A conversation turn scraped from a page by the browser extension.
#[derive(Deserialize)]
struct IngestTurn {
    role: String,
    content: String,
}

/// The browser-extension channel payload (`POST /ingest`).
#[derive(Deserialize)]
struct IngestBody {
    #[serde(default, rename = "sessionId")]
    session_id: String,
    #[serde(default)]
    model: String,
    turns: Vec<IngestTurn>,
}

/// Ingest a conversation scraped by the browser extension and run detection —
/// the same engine path as the proxy and file channels.
async fn ingest_handler(State(app): State<AppState>, Json(body): Json<IngestBody>) -> Response {
    if body.turns.is_empty() {
        return (StatusCode::BAD_REQUEST, "no turns").into_response();
    }
    let model = if body.model.is_empty() {
        "unknown".to_string()
    } else {
        body.model
    };
    let turns: Vec<(drifterr_engine::conversation::Role, String)> = body
        .turns
        .into_iter()
        .map(|t| {
            use drifterr_engine::conversation::Role;
            let role = match t.role.as_str() {
                "assistant" => Role::Assistant,
                "tool" | "function" => Role::Tool,
                _ => Role::User,
            };
            (role, t.content)
        })
        .collect();

    let session_id = if body.session_id.is_empty() {
        // Stable id from the first user turn when the page gives us none.
        let anchor = turns
            .iter()
            .find(|(r, _)| *r == drifterr_engine::conversation::Role::User)
            .map(|(_, c)| c.as_str())
            .unwrap_or("default");
        format!("browser-{:016x}", fnv1a(anchor))
    } else {
        format!("browser-{}", body.session_id)
    };

    let conv = state::browser_conversation(session_id, model, turns);
    if let Ok(mut core) = app.core.lock() {
        core.record_conversation(&conv);
        return Json(core.current()).into_response();
    }
    (StatusCode::INTERNAL_SERVER_ERROR, "busy").into_response()
}

fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Directory the UI assets live in, used to serve `/public/*` (fonts, etc.).
/// Defaults to the in-repo path for dev; override with `DRIFTERR_UI_DIR`. In a
/// packaged Tauri build the webview serves these directly, so this is mainly for
/// the browser dashboard.
fn ui_dir() -> std::path::PathBuf {
    if let Ok(d) = std::env::var("DRIFTERR_UI_DIR") {
        return std::path::PathBuf::from(d);
    }
    std::path::PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../apps/desktop/ui"
    ))
}

fn content_type_for(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
}

/// Serve a file from `<ui_dir>/public/`. Read-only, with a path-traversal guard.
async fn public_handler(AxPath(path): AxPath<String>) -> Response {
    if path.contains("..") {
        return (StatusCode::BAD_REQUEST, "bad path").into_response();
    }
    let full = ui_dir().join("public").join(&path);
    match tokio::fs::read(&full).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, content_type_for(&path))], bytes).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

async fn config_handler(State(app): State<AppState>) -> Json<ConfigMeta> {
    Json((*app.meta).clone())
}

/// A standing order (recurring correction) for the control API.
#[derive(Serialize)]
struct StandingOrderView {
    id: i64,
    text: String,
    occurrences: i64,
    promoted: bool,
    /// Recurring enough to propose, not yet promoted.
    candidate: bool,
}

async fn standing_orders_handler(State(app): State<AppState>) -> Json<Vec<StandingOrderView>> {
    let orders = app
        .core
        .lock()
        .map(|c| c.standing_orders())
        .unwrap_or_default();
    Json(
        orders
            .into_iter()
            .map(|o| StandingOrderView {
                candidate: o.is_candidate(),
                id: o.id,
                text: o.text,
                occurrences: o.occurrences,
                promoted: o.promoted,
            })
            .collect(),
    )
}

#[derive(Deserialize)]
struct PromoteBody {
    id: i64,
}

/// Promote a standing order into a persistent rule (the user accepting it).
async fn promote_handler(State(app): State<AppState>, Json(body): Json<PromoteBody>) -> Response {
    let ok = app
        .core
        .lock()
        .map(|c| c.promote_standing_order(body.id))
        .unwrap_or(false);
    if ok {
        (StatusCode::OK, "promoted").into_response()
    } else {
        (StatusCode::NOT_FOUND, "unknown standing order").into_response()
    }
}

#[derive(Deserialize)]
struct ReanchorQuery {
    /// Optional session id; defaults to the most recently updated session.
    session: Option<String>,
}

/// Generate the re-anchor intervention (snapshot + preamble) for a session.
async fn reanchor_handler(State(app): State<AppState>, Query(q): Query<ReanchorQuery>) -> Response {
    let out = app
        .core
        .lock()
        .ok()
        .and_then(|core| core.reanchor(q.session.as_deref()));
    match out {
        Some(r) => Json(r).into_response(),
        None => (StatusCode::NOT_FOUND, "no active session to re-anchor").into_response(),
    }
}

/// Add permissive CORS headers and short-circuit preflight requests.
async fn add_cors(req: Request, next: Next) -> Response {
    let is_preflight = req.method() == Method::OPTIONS;
    let mut res = if is_preflight {
        Response::new(Body::empty())
    } else {
        next.run(req).await
    };
    let h = res.headers_mut();
    h.insert("access-control-allow-origin", "*".parse().unwrap());
    h.insert(
        "access-control-allow-methods",
        "GET, POST, OPTIONS".parse().unwrap(),
    );
    h.insert("access-control-allow-headers", "*".parse().unwrap());
    res
}

/// Run both listeners until shutdown. Convenience entry point for the binary.
pub async fn serve(
    proxy_addr: SocketAddr,
    control_addr: SocketAddr,
    state: AppState,
) -> std::io::Result<()> {
    let proxy = TcpListener::bind(proxy_addr).await?;
    let control = TcpListener::bind(control_addr).await?;
    let proxy_app = proxy_router(state.clone());
    let control_app = control_router(state);
    tokio::try_join!(
        axum::serve(proxy, proxy_app).into_future(),
        axum::serve(control, control_app).into_future(),
    )?;
    Ok(())
}

#[derive(Serialize)]
struct StatusResponse {
    current: Option<SessionStatus>,
    sessions: Vec<SessionStatus>,
    /// The active entitlement, so the UI can render locks + upgrade prompts.
    entitlement: Entitlement,
    /// How many tracked sessions are hidden by the plan's session cap.
    #[serde(rename = "sessionsLocked")]
    sessions_locked: usize,
}

async fn status_handler(State(app): State<AppState>) -> Json<StatusResponse> {
    let ent = app.entitlement();
    let (mut current, mut sessions) = match app.core.lock() {
        Ok(core) => (core.current(), core.all()),
        Err(_) => (None, Vec::new()),
    };

    // Gate the drift map: don't expose its history data unless the plan unlocks it.
    if !ent.drift_map {
        if let Some(c) = current.as_mut() {
            c.history.clear();
        }
        for s in sessions.iter_mut() {
            s.history.clear();
        }
    }

    // Gate concurrent sessions: keep the active one first, then fill to the cap;
    // the rest are surfaced only as a locked count.
    let mut sessions_locked = 0;
    if let Some(max) = ent.max_sessions {
        if sessions.len() > max {
            if let Some(cur) = current.as_ref() {
                sessions.sort_by_key(|s| s.session_id != cur.session_id);
            }
            sessions_locked = sessions.len() - max;
            sessions.truncate(max);
        }
    }

    Json(StatusResponse {
        current,
        sessions,
        entitlement: ent,
        sessions_locked,
    })
}

async fn entitlement_handler(State(app): State<AppState>) -> Json<Entitlement> {
    Json(app.entitlement())
}

/// Set the active plan (the desktop app calls this after `/me`). We derive the
/// capabilities from the plan id — never from client-sent flags.
#[derive(Deserialize)]
struct SetEntitlement {
    #[serde(default)]
    plan: String,
}

async fn set_entitlement_handler(
    State(app): State<AppState>,
    Json(body): Json<SetEntitlement>,
) -> Json<Entitlement> {
    let ent = Entitlement::for_plan(Plan::from_id(&body.plan));
    if let Ok(mut w) = app.entitlement.write() {
        *w = ent;
    }
    Json(ent)
}

async fn sessions_handler(State(app): State<AppState>) -> Json<Vec<SessionStatus>> {
    let sessions = app.core.lock().map(|c| c.all()).unwrap_or_default();
    Json(sessions)
}

/// The transparent relay handler with the streaming tee.
async fn proxy_handler(State(app): State<AppState>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());

    // We must read the whole *request* body — both to relay it and to recover
    // the conversation. (Requests are not the streaming concern; responses are.)
    let body_bytes = match axum::body::to_bytes(body, MAX_BODY).await {
        Ok(b) => b,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "request body too large"),
    };

    let provider = Provider::from_path(&path_and_query);
    let upstream_base = app.cfg.upstream_for(provider).trim_end_matches('/');
    let url = format!("{upstream_base}{path_and_query}");
    let parsed_req = provider::parse_request(provider, &body_bytes);

    // Opt-in auto-re-anchor: if this session is currently drifting (RED), inject
    // the re-anchor preamble into the outgoing request. Idempotent and best-
    // effort — on any doubt we relay the original bytes unchanged.
    let mut body_to_send = body_bytes.to_vec();
    if app.auto_reanchor && app.entitlement().auto_reanchor {
        let session_id = state::session_id_for(&parsed_req);
        let preamble = app
            .core
            .lock()
            .ok()
            .and_then(|core| core.auto_preamble(&session_id));
        if let Some(preamble) = preamble {
            if let Some(modified) = provider::inject_preamble(provider, &body_bytes, &preamble) {
                body_to_send = modified;
            }
        }
    }

    // Relay to the real provider.
    let upstream = app
        .client
        .request(parts.method.clone(), &url)
        .headers(forward_headers(&parts.headers))
        .body(body_to_send)
        .send()
        .await;
    let upstream = match upstream {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_GATEWAY, &format!("upstream error: {e}")),
    };

    let status = upstream.status();
    let resp_headers = upstream.headers().clone();
    let content_type = resp_headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    // The tee: each chunk is cloned (a refcount bump on `Bytes`) into a channel,
    // then yielded onward to the client untouched.
    let (tee_tx, mut tee_rx) = tokio::sync::mpsc::unbounded_channel::<Bytes>();
    let teed = upstream.bytes_stream().map(move |chunk| {
        if let Ok(bytes) = &chunk {
            let _ = tee_tx.send(bytes.clone());
        }
        chunk
    });

    // Detection runs entirely off the response path. When the client finishes
    // (or disconnects) the stream drops, closing the channel, and this task
    // finalizes with whatever was received.
    let app2 = app.clone();
    tokio::spawn(async move {
        let mut buf = Vec::new();
        while let Some(b) = tee_rx.recv().await {
            buf.extend_from_slice(&b);
        }
        let parsed_resp = provider::parse_response(provider, &content_type, &buf);
        if parsed_resp.assistant_text.is_empty() && !parsed_resp.has_exact_usage() {
            return; // nothing detectable in this response
        }
        let session_id = state::session_id_for(&parsed_req);

        // Base (deterministic + soft) signals — sync, under the lock.
        let decisions = {
            let Ok(mut core) = app2.core.lock() else {
                return;
            };
            core.record_turn(&session_id, &parsed_req, &parsed_resp);
            core.decisions_for(&session_id)
        };

        // Judge phase — async, off the lock. Signal 3 (decision coherence).
        if app2.judge.enabled() && !decisions.is_empty() && !parsed_resp.assistant_text.is_empty() {
            let last = drifterr_engine::conversation::Turn {
                index: parsed_req.turns.len(),
                role: drifterr_engine::conversation::Role::Assistant,
                content: parsed_resp.assistant_text.clone(),
                tokens: parsed_resp.output_tokens.unwrap_or(0),
                timestamp: 0,
            };
            let embedder = drifterr_embeddings::BagEmbedder::default();
            if let Some(event) = drifterr_judge::decision::decision_coherence(
                &last,
                &decisions,
                &embedder,
                &app2.judge,
            )
            .await
            {
                if let Ok(mut core) = app2.core.lock() {
                    core.apply_extra_events(&session_id, vec![event]);
                }
            }
        }
    });

    // Relay status + headers + the streaming body.
    let mut builder = Response::builder().status(status.as_u16());
    for (name, value) in resp_headers.iter() {
        let n = name.as_str();
        if is_hop_by_hop(n) || n == "content-length" {
            continue; // length is unknown for a stream; let the server frame it
        }
        builder = builder.header(name.as_str(), value.as_bytes());
    }
    builder.body(Body::from_stream(teed)).unwrap_or_else(|_| {
        error_response(StatusCode::INTERNAL_SERVER_ERROR, "response build error")
    })
}

/// Copy request headers for forwarding, dropping hop-by-hop headers, `host`
/// (reqwest sets it for the upstream), `content-length` (reqwest recomputes it),
/// and `accept-encoding` (so the upstream returns identity bytes we can parse —
/// the client still receives whatever the upstream sends).
fn forward_headers(src: &axum::http::HeaderMap) -> reqwest::header::HeaderMap {
    let mut out = reqwest::header::HeaderMap::new();
    for (name, value) in src.iter() {
        let n = name.as_str();
        if is_hop_by_hop(n) || n == "host" || n == "content-length" || n == "accept-encoding" {
            continue;
        }
        if let (Ok(hn), Ok(hv)) = (
            reqwest::header::HeaderName::from_bytes(n.as_bytes()),
            reqwest::header::HeaderValue::from_bytes(value.as_bytes()),
        ) {
            out.insert(hn, hv);
        }
    }
    out
}

/// RFC 7230 hop-by-hop headers (must not be forwarded by a proxy).
fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailers"
            | "transfer-encoding"
            | "upgrade"
    )
}

fn error_response(code: StatusCode, msg: &str) -> Response {
    Response::builder()
        .status(code)
        .header("content-type", "text/plain")
        .body(Body::from(format!("drifterr proxy: {msg}")))
        .expect("error response")
}
