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
pub mod upstreams;

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
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// When the OpenAI-schema upstream carries its own path prefix (e.g. Gemini's
    /// `/v1beta/openai`), strip the incoming `/v1` so the path joins correctly.
    pub openai_strip_v1: bool,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        // Drifterr standardizes on OpenRouter (OpenAI schema) as the default,
        // but any of the major providers can be selected — see `from_env`.
        Self {
            openai_upstream: "https://openrouter.ai/api".to_string(),
            anthropic_upstream: "https://api.anthropic.com".to_string(),
            openai_strip_v1: false,
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

    /// Resolve the upstreams from the environment. `DRIFTERR_PROVIDER` selects a
    /// built-in preset (openrouter | openai | anthropic | gemini | groq | mistral
    /// | deepseek | xai | together); explicit `OPENAI_UPSTREAM` /
    /// `ANTHROPIC_UPSTREAM` URLs always win for a fully custom endpoint.
    pub fn from_env() -> Self {
        let mut cfg = ProxyConfig::default();

        if let Ok(name) = std::env::var("DRIFTERR_PROVIDER") {
            if let Some(p) = upstreams::find(&name) {
                if !p.openai_base.is_empty() {
                    cfg.openai_upstream = p.openai_base.to_string();
                    cfg.openai_strip_v1 = p.openai_strip_v1;
                }
                if !p.anthropic_base.is_empty() {
                    cfg.anthropic_upstream = p.anthropic_base.to_string();
                }
            } else if !name.trim().is_empty() {
                eprintln!("drifterr: unknown DRIFTERR_PROVIDER '{name}' — using default");
            }
        }

        // Explicit URLs override the preset (custom / self-hosted endpoints).
        if let Ok(u) = std::env::var("OPENAI_UPSTREAM") {
            if !u.is_empty() {
                cfg.openai_upstream = u;
                cfg.openai_strip_v1 = false;
            }
        }
        if let Ok(u) = std::env::var("ANTHROPIC_UPSTREAM") {
            if !u.is_empty() {
                cfg.anthropic_upstream = u;
            }
        }
        cfg
    }

    /// Friendly name of the active OpenAI-schema provider, for `/config`/the UI.
    pub fn provider_label(&self) -> &'static str {
        upstreams::label_for_base(&self.openai_upstream)
    }
}

/// Effective configuration, exposed read-only at `GET /config` for the UI's
/// settings view and for diagnostics. (Editable judge/privacy settings arrive
/// with the judge milestone; this is the foundation they'll extend.)
#[derive(Debug, Clone, Serialize)]
pub struct ConfigMeta {
    /// The user-facing app version. Defaults to this crate's version, but the
    /// desktop shell overrides it (via `DRIFTERR_APP_VERSION`) so the settings
    /// view shows the installed app's version, not the embedded proxy crate's.
    pub version: String,
    #[serde(rename = "openaiUpstream")]
    pub openai_upstream: String,
    #[serde(rename = "anthropicUpstream")]
    pub anthropic_upstream: String,
    /// Friendly name of the active OpenAI-schema provider (OpenRouter, OpenAI,
    /// Google Gemini, …) so the UI can show what you're plugged into.
    pub provider: String,
    /// Whether sessions are persisted to SQLite (vs in-memory only).
    pub persisted: bool,
    /// Judge backend label ("disabled", a model id, or "stub").
    pub judge: String,
    /// Whether opt-in auto-re-anchor is active.
    #[serde(rename = "autoReanchor")]
    pub auto_reanchor: bool,
}

/// The upstream provider currently in effect. Runtime-mutable so the menubar's
/// provider selector can switch where traffic is relayed without a restart.
#[derive(Debug, Clone, Serialize)]
pub struct ActiveUpstream {
    #[serde(rename = "openaiUpstream")]
    pub openai_upstream: String,
    #[serde(rename = "anthropicUpstream")]
    pub anthropic_upstream: String,
    #[serde(skip)]
    pub openai_strip_v1: bool,
    /// Preset id (e.g. "openai") or "custom".
    pub provider: String,
    #[serde(rename = "providerLabel")]
    pub provider_label: String,
}

/// Shared, cheaply-clonable application state.
#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<ProxyConfig>,
    pub client: reqwest::Client,
    pub core: Arc<Mutex<AppCore>>,
    pub meta: Arc<ConfigMeta>,
    /// The judge backend. `RwLock` so the settings panel can swap it in at
    /// runtime (enter an API key → judge on) without a restart.
    pub judge: Arc<RwLock<drifterr_judge::Judge>>,
    /// Opt-in: inject the re-anchor preamble into outgoing requests when the
    /// session is drifting (RED). Off by default — it modifies user requests.
    /// Runtime-toggleable (the panel's Auto re-anchor switch) via an atomic, so
    /// the choice takes effect without a restart.
    pub auto_reanchor: Arc<AtomicBool>,
    /// Opt-in: let the judge infer the session's goal + constraints from the
    /// conversation (Auto-intent), so the user never has to type them. Off by
    /// default; requires the judge to be configured. Runtime-toggleable.
    pub auto_intent: Arc<AtomicBool>,
    /// Do Not Disturb: when on, the native shell suppresses all OS notifications
    /// (drift alerts + update alerts). The panel still updates. Off by default.
    pub notifications_muted: Arc<AtomicBool>,
    /// Current plan entitlement, set by the desktop app after login. Defaults to
    /// Free so the proxy works standalone. Gates paid capabilities (drift map,
    /// extra sessions, auto-re-anchor).
    pub entitlement: Arc<RwLock<Entitlement>>,
    /// The active upstream provider — runtime-switchable via `POST /provider`.
    pub upstream: Arc<RwLock<ActiveUpstream>>,
}

impl AppState {
    /// Build state with a config and an optional durable store. Judge and
    /// auto-re-anchor are configured from the environment.
    pub fn new(cfg: ProxyConfig, store: Option<drifterr_store::Store>) -> Self {
        let truthy = |k: &str| {
            matches!(
                std::env::var(k).as_deref(),
                Ok("1") | Ok("true") | Ok("yes")
            )
        };
        let s = Self::build(
            cfg,
            store,
            drifterr_judge::Judge::from_env(),
            truthy("DRIFTERR_AUTO_REANCHOR"),
        );
        s.auto_intent
            .store(truthy("DRIFTERR_AUTO_INTENT"), Ordering::Relaxed);
        s
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
    pub fn with_auto_reanchor(self, on: bool) -> Self {
        self.auto_reanchor.store(on, Ordering::Relaxed);
        self
    }

    /// Whether auto-re-anchor injection is currently on (the toggle only; the
    /// plan entitlement is the second gate, checked at inject time).
    pub fn auto_reanchor_on(&self) -> bool {
        self.auto_reanchor.load(Ordering::Relaxed)
    }

    /// Enable/disable Auto-intent (used by tests).
    pub fn with_auto_intent(self, on: bool) -> Self {
        self.auto_intent.store(on, Ordering::Relaxed);
        self
    }

    /// Whether Auto-intent inference is currently on.
    pub fn auto_intent_on(&self) -> bool {
        self.auto_intent.load(Ordering::Relaxed)
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
            version: std::env::var("DRIFTERR_APP_VERSION")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string()),
            openai_upstream: cfg.openai_upstream.clone(),
            anthropic_upstream: cfg.anthropic_upstream.clone(),
            provider: cfg.provider_label().to_string(),
            persisted: store.is_some(),
            judge: judge.label(),
            auto_reanchor,
        };
        let judge = Arc::new(RwLock::new(judge));
        let upstream = ActiveUpstream {
            openai_upstream: cfg.openai_upstream.clone(),
            anthropic_upstream: cfg.anthropic_upstream.clone(),
            openai_strip_v1: cfg.openai_strip_v1,
            provider: upstreams::id_for_base(&cfg.openai_upstream).to_string(),
            provider_label: cfg.provider_label().to_string(),
        };
        Self {
            cfg: Arc::new(cfg),
            client,
            core: Arc::new(Mutex::new(AppCore::new(store))),
            meta: Arc::new(meta),
            judge,
            auto_reanchor: Arc::new(AtomicBool::new(auto_reanchor)),
            auto_intent: Arc::new(AtomicBool::new(false)),
            notifications_muted: Arc::new(AtomicBool::new(false)),
            entitlement: Arc::new(RwLock::new(Entitlement::default())),
            upstream: Arc::new(RwLock::new(upstream)),
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
        .route("/providers", get(providers_handler))
        .route("/provider", post(set_provider_handler))
        .route(
            "/entitlement",
            get(entitlement_handler).post(set_entitlement_handler),
        )
        .route("/reanchor", get(reanchor_handler))
        .route("/intent", get(get_intent_handler).post(set_intent_handler))
        .route("/intent/retire", post(retire_constraint_handler))
        .route("/judge", get(get_judge_handler).post(set_judge_handler))
        .route(
            "/auto-reanchor",
            get(get_auto_reanchor_handler).post(set_auto_reanchor_handler),
        )
        .route(
            "/auto-intent",
            get(get_auto_intent_handler).post(set_auto_intent_handler),
        )
        .route("/intent-shift", post(resolve_intent_shift_handler))
        .route("/prefs", get(get_prefs_handler).post(set_prefs_handler))
        .route("/history", get(history_handler))
        .route("/journal", get(journal_handler))
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
    let mut meta = (*app.meta).clone();
    // Reflect the live (possibly switched) provider, not just the boot config.
    if let Ok(u) = app.upstream.read() {
        meta.openai_upstream = u.openai_upstream.clone();
        meta.anthropic_upstream = u.anthropic_upstream.clone();
        meta.provider = u.provider_label.clone();
    }
    // Reflect the live auto-re-anchor toggle, not just the boot value.
    meta.auto_reanchor = app.auto_reanchor_on();
    // Reflect the live (possibly runtime-configured) judge, not just the boot one.
    if let Ok(j) = app.judge.read() {
        meta.judge = j.label();
    }
    Json(meta)
}

/// One selectable provider, for the menubar's provider selector.
#[derive(Serialize)]
struct ProviderInfo {
    id: &'static str,
    label: &'static str,
}

#[derive(Serialize)]
struct ProvidersResponse {
    current: String,
    providers: Vec<ProviderInfo>,
}

async fn providers_handler(State(app): State<AppState>) -> Json<ProvidersResponse> {
    let current = app
        .upstream
        .read()
        .map(|u| u.provider.clone())
        .unwrap_or_else(|_| "openrouter".to_string());
    let providers = upstreams::PRESETS
        .iter()
        .map(|p| ProviderInfo {
            id: p.id,
            label: p.label,
        })
        .collect();
    Json(ProvidersResponse { current, providers })
}

#[derive(Deserialize)]
struct SetProvider {
    #[serde(default)]
    id: String,
}

/// Switch the upstream provider at runtime (the selector calls this). Derives the
/// upstreams from the preset id — never trusts arbitrary URLs from the client.
async fn set_provider_handler(
    State(app): State<AppState>,
    Json(body): Json<SetProvider>,
) -> Response {
    let Some(p) = upstreams::find(&body.id) else {
        return (StatusCode::BAD_REQUEST, "unknown provider").into_response();
    };
    if let Ok(mut u) = app.upstream.write() {
        if !p.openai_base.is_empty() {
            u.openai_upstream = p.openai_base.to_string();
            u.openai_strip_v1 = p.openai_strip_v1;
        }
        if !p.anthropic_base.is_empty() {
            u.anthropic_upstream = p.anthropic_base.to_string();
        }
        u.provider = p.id.to_string();
        u.provider_label = p.label.to_string();
        return Json(u.clone()).into_response();
    }
    (StatusCode::INTERNAL_SERVER_ERROR, "busy").into_response()
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

/// One past session for the history view.
#[derive(Serialize)]
struct HistoryItem {
    #[serde(rename = "sessionId")]
    session_id: String,
    model: String,
    goal: String,
    /// "green" | "amber" | "red" | "" (unknown).
    state: String,
    turns: i64,
    #[serde(rename = "lastActivity")]
    last_activity: i64,
}

/// Recent sessions (newest first) for the history/timeline view. Reads the local
/// store only — nothing leaves the machine.
async fn history_handler(State(app): State<AppState>) -> Json<Vec<HistoryItem>> {
    let sessions = app
        .core
        .lock()
        .map(|c| c.session_history(50))
        .unwrap_or_default();
    Json(
        sessions
            .into_iter()
            .map(|s| HistoryItem {
                session_id: s.session_id,
                model: s.model,
                goal: s.goal.unwrap_or_default(),
                state: s.status.unwrap_or_default(),
                turns: s.turns,
                last_activity: s.last_ts,
            })
            .collect(),
    )
}

/// One journal entry for the activity view.
#[derive(Serialize)]
struct JournalItem {
    signal: String,
    /// "amber" | "red".
    state: String,
    detail: String,
    #[serde(rename = "constraintId", skip_serializing_if = "Option::is_none")]
    constraint_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    span: Option<String>,
    #[serde(rename = "turn", skip_serializing_if = "Option::is_none")]
    turn_index: Option<usize>,
}

/// Recent flag events (amber/red) for a session — the readable "what fired and
/// when" journal. Reads the local store only.
async fn journal_handler(State(app): State<AppState>, Query(q): Query<ReanchorQuery>) -> Response {
    let flags = app
        .core
        .lock()
        .map(|c| c.journal(q.session.as_deref(), 30))
        .unwrap_or_default();
    let items: Vec<JournalItem> = flags
        .into_iter()
        .map(|f| JournalItem {
            state: f.state,
            signal: f.signal,
            detail: f.detail,
            constraint_id: f.constraint_id,
            span: f.span,
            turn_index: f.turn_index,
        })
        .collect();
    Json(items).into_response()
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

/// The user-declared intent for a session (`POST /intent`). An empty `goal`
/// leaves the existing goal untouched; `constraints` are full phrases the user
/// typed (deterministic when a rule can be inferred, fuzzy/judge otherwise).
#[derive(Deserialize)]
struct SetIntentBody {
    #[serde(default)]
    session: Option<String>,
    #[serde(default)]
    goal: String,
    #[serde(default)]
    constraints: Vec<String>,
}

/// Read the current intent (goal + constraints) for a session — powers the
/// intent editor and the onboarding "your intent" step.
async fn get_intent_handler(
    State(app): State<AppState>,
    Query(q): Query<ReanchorQuery>,
) -> Response {
    match app
        .core
        .lock()
        .ok()
        .and_then(|core| core.intent_of(q.session.as_deref()))
    {
        Some(view) => Json(view).into_response(),
        None => (StatusCode::NOT_FOUND, "no session yet").into_response(),
    }
}

/// Declare/replace the intent for a session (or seed the next one when none is
/// live). Returns the resolved intent so the UI can render exactly what stuck.
async fn set_intent_handler(
    State(app): State<AppState>,
    Json(body): Json<SetIntentBody>,
) -> Response {
    if let Ok(mut core) = app.core.lock() {
        let view = core.set_intent(body.session.as_deref(), &body.goal, &body.constraints);
        return Json(view).into_response();
    }
    (StatusCode::INTERNAL_SERVER_ERROR, "busy").into_response()
}

/// User preferences the panel controls and the native shell reads.
#[derive(Serialize)]
struct Prefs {
    #[serde(rename = "notificationsMuted")]
    notifications_muted: bool,
}

async fn get_prefs_handler(State(app): State<AppState>) -> Json<Prefs> {
    Json(Prefs {
        notifications_muted: app.notifications_muted.load(Ordering::Relaxed),
    })
}

#[derive(Deserialize)]
struct SetPrefs {
    #[serde(default, rename = "notificationsMuted")]
    notifications_muted: bool,
}

/// Set preferences (Do Not Disturb). The native shell reads the result from
/// `/status` and suppresses OS notifications accordingly.
async fn set_prefs_handler(State(app): State<AppState>, Json(body): Json<SetPrefs>) -> Json<Prefs> {
    app.notifications_muted
        .store(body.notifications_muted, Ordering::Relaxed);
    Json(Prefs {
        notifications_muted: body.notifications_muted,
    })
}

/// Retire (remove) a constraint the user no longer wants enforced.
#[derive(Deserialize)]
struct RetireConstraint {
    id: String,
    #[serde(default)]
    session: Option<String>,
}

async fn retire_constraint_handler(
    State(app): State<AppState>,
    Json(body): Json<RetireConstraint>,
) -> Response {
    match app
        .core
        .lock()
        .ok()
        .and_then(|mut c| c.retire_constraint(body.session.as_deref(), &body.id))
    {
        Some(view) => Json(view).into_response(),
        None => (StatusCode::NOT_FOUND, "unknown constraint").into_response(),
    }
}

/// Judge status for the settings panel — never echoes the key back.
#[derive(Serialize)]
struct JudgeState {
    /// True when a judge backend is active (fuzzy signals run).
    enabled: bool,
    /// Backend label: "disabled", "stub", or the model id.
    label: String,
}

impl JudgeState {
    fn of(app: &AppState) -> Self {
        let (enabled, label) = app
            .judge
            .read()
            .map(|j| (j.enabled(), j.label()))
            .unwrap_or((false, "disabled".to_string()));
        JudgeState { enabled, label }
    }
}

async fn get_judge_handler(State(app): State<AppState>) -> Json<JudgeState> {
    Json(JudgeState::of(&app))
}

/// Configure the judge at runtime from the user's own OpenRouter key + model.
/// The key lives only in the proxy's memory (never persisted here, never echoed
/// back). An empty key disables the judge. This is the "turn on the fuzzy
/// signals in one field" path — no restart, no env var.
#[derive(Deserialize)]
struct SetJudge {
    #[serde(default, rename = "apiKey")]
    api_key: String,
    #[serde(default)]
    model: Option<String>,
}

async fn set_judge_handler(State(app): State<AppState>, Json(body): Json<SetJudge>) -> Response {
    let new_judge = drifterr_judge::Judge::openrouter(&body.api_key, body.model.as_deref());
    match app.judge.write() {
        Ok(mut j) => *j = new_judge,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "busy").into_response(),
    }
    Json(JudgeState::of(&app)).into_response()
}

/// Auto-intent state for the settings switch.
#[derive(Serialize)]
struct AutoIntentState {
    on: bool,
    /// Auto-intent needs the judge (it's an LLM inference); surface that so the
    /// UI can tell the user to add their key first.
    #[serde(rename = "judgeReady")]
    judge_ready: bool,
}

impl AutoIntentState {
    fn of(app: &AppState) -> Self {
        AutoIntentState {
            on: app.auto_intent_on(),
            judge_ready: app.judge.read().map(|j| j.enabled()).unwrap_or(false),
        }
    }
}

async fn get_auto_intent_handler(State(app): State<AppState>) -> Json<AutoIntentState> {
    Json(AutoIntentState::of(&app))
}

async fn set_auto_intent_handler(
    State(app): State<AppState>,
    Json(body): Json<SetAutoReanchor>,
) -> Json<AutoIntentState> {
    app.auto_intent.store(body.on, Ordering::Relaxed);
    Json(AutoIntentState::of(&app))
}

/// Resolve a pending Auto-intent goal shift: accept the new goal (a pivot) or
/// keep the old one (drift).
#[derive(Deserialize)]
struct ResolveShift {
    accept: bool,
    #[serde(default)]
    session: Option<String>,
}

async fn resolve_intent_shift_handler(
    State(app): State<AppState>,
    Json(body): Json<ResolveShift>,
) -> Response {
    match app
        .core
        .lock()
        .ok()
        .and_then(|mut c| c.resolve_intent_shift(body.session.as_deref(), body.accept))
    {
        Some(view) => Json(view).into_response(),
        None => (StatusCode::NOT_FOUND, "no pending shift").into_response(),
    }
}

/// Auto-re-anchor toggle state for the panel switch.
#[derive(Serialize)]
struct AutoReanchorState {
    /// Whether the user has the switch on.
    on: bool,
    /// Whether the current plan actually permits injection (the second gate).
    allowed: bool,
    /// `on && allowed` — whether injection will really happen while drifting.
    effective: bool,
}

impl AutoReanchorState {
    fn of(app: &AppState) -> Self {
        let on = app.auto_reanchor_on();
        let allowed = app.entitlement().auto_reanchor;
        AutoReanchorState {
            on,
            allowed,
            effective: on && allowed,
        }
    }
}

async fn get_auto_reanchor_handler(State(app): State<AppState>) -> Json<AutoReanchorState> {
    Json(AutoReanchorState::of(&app))
}

#[derive(Deserialize)]
struct SetAutoReanchor {
    on: bool,
}

/// Toggle auto-re-anchor injection at runtime (the panel's switch). The plan
/// entitlement is still enforced at inject time, so turning it on with a Free
/// plan is accepted but simply won't inject until upgraded — the response says
/// so via `effective`.
async fn set_auto_reanchor_handler(
    State(app): State<AppState>,
    Json(body): Json<SetAutoReanchor>,
) -> Json<AutoReanchorState> {
    app.auto_reanchor.store(body.on, Ordering::Relaxed);
    Json(AutoReanchorState::of(&app))
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
    /// Do Not Disturb — the native shell reads this to suppress OS notifications.
    #[serde(rename = "notificationsMuted")]
    notifications_muted: bool,
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
        notifications_muted: app.notifications_muted.load(Ordering::Relaxed),
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
    // Read the live upstream (runtime-switchable via the provider selector).
    let (base, strip_v1) = match app.upstream.read() {
        Ok(u) => match provider {
            Provider::OpenAI => (u.openai_upstream.clone(), u.openai_strip_v1),
            Provider::Anthropic => (u.anthropic_upstream.clone(), false),
        },
        Err(_) => (app.cfg.upstream_for(provider).to_string(), false),
    };
    // Gemini-style upstreams carry their own `/v1beta/openai` prefix, so strip the
    // incoming `/v1` for OpenAI-schema traffic when configured.
    let strip = matches!(provider, Provider::OpenAI) && strip_v1;
    let url = upstreams::join_url(&base, &path_and_query, strip);
    let parsed_req = provider::parse_request(provider, &body_bytes);

    // Opt-in auto-re-anchor: if this session is currently drifting (RED), inject
    // the re-anchor preamble into the outgoing request. Idempotent and best-
    // effort — on any doubt we relay the original bytes unchanged.
    let mut body_to_send = body_bytes.to_vec();
    if app.auto_reanchor_on() && app.entitlement().auto_reanchor {
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

        // Judge phase — async, off the lock. It powers the *fuzzy* checks the
        // deterministic engine can't make: Signal 3 (decision coherence) and the
        // judge half of Signal 1 (fuzzy constraint adherence). All of it is
        // fail-safe and AMBER-only — a judge that cries wolf can only raise a
        // watch, never a wall.
        // Snapshot the judge (cheap clone) so the runtime-swappable RwLock isn't
        // held across the awaits below.
        let judge = match app2.judge.read() {
            Ok(j) => j.clone(),
            Err(_) => return,
        };
        if !judge.enabled() || parsed_resp.assistant_text.is_empty() {
            return;
        }

        let last = drifterr_engine::conversation::Turn {
            index: parsed_req.turns.len(),
            role: drifterr_engine::conversation::Role::Assistant,
            content: parsed_resp.assistant_text.clone(),
            tokens: parsed_resp.output_tokens.unwrap_or(0),
            timestamp: 0,
        };

        // (a) LLM-assisted constraint extraction. Gate on a cheap local cue so we
        // only spend a call on the newest user turn when it plausibly states a
        // rule; add whatever fuzzy constraints it yields to the baseline.
        if let Some(user_msg) = parsed_req
            .turns
            .iter()
            .rev()
            .find(|t| t.role == drifterr_engine::conversation::Role::User)
        {
            if drifterr_engine::infer::has_constraint_cue(&user_msg.content) {
                let extracted = judge.extract_constraints(&user_msg.content).await;
                if !extracted.is_empty() {
                    if let Ok(mut core) = app2.core.lock() {
                        core.add_judge_constraints(&session_id, extracted);
                    }
                }
            }
        }

        // (a2) Auto-intent: infer the whole intent (goal + constraints) from the
        // conversation so the user never has to type it. Opt-in, rate-limited, and
        // fail-safe — an empty/failed inference just advances the rate limiter. The
        // goal it sets only feeds the soft signal; a big goal shift is surfaced as
        // a prompt, never a silent overwrite (see apply_inferred_intent).
        if app2.auto_intent_on() {
            let due = app2
                .core
                .lock()
                .map(|c| c.due_for_intent_synthesis(&session_id))
                .unwrap_or(false);
            if due {
                let transcript = state::transcript_for(&parsed_req, &parsed_resp);
                let intent = judge
                    .synthesize_intent(&transcript)
                    .await
                    .unwrap_or_default();
                if let Ok(mut core) = app2.core.lock() {
                    core.apply_inferred_intent(&session_id, &intent);
                }
            }
        }

        // (b) Run both judge signals against the new assistant turn, then merge
        // their events in one pass so the status updates once.
        let judge_constraints = {
            let Ok(core) = app2.core.lock() else {
                return;
            };
            core.judge_constraints_for(&session_id)
        };
        let embedder = drifterr_embeddings::BagEmbedder::default();

        let mut extra =
            drifterr_judge::constraint::constraint_adherence(&last, &judge_constraints, &judge)
                .await;

        if !decisions.is_empty() {
            if let Some(event) =
                drifterr_judge::decision::decision_coherence(&last, &decisions, &embedder, &judge)
                    .await
            {
                extra.push(event);
            }
        }

        if !extra.is_empty() {
            if let Ok(mut core) = app2.core.lock() {
                core.apply_extra_events(&session_id, extra);
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
