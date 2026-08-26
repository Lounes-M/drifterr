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

pub mod auth;
pub mod check;
pub mod dashboard;
pub mod entitlement;
pub mod hook;
pub mod mcp;
pub mod plan_token;
pub mod provider;
pub mod state;
pub mod team;
pub mod upstreams;
pub mod views;

use axum::body::Body;
use axum::extract::{Path as AxPath, Query, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bytes::Bytes;
use entitlement::{Entitlement, Plan};
use futures_util::StreamExt;
use provider::Provider;
use serde::Deserialize;
use serde::Serialize;
use state::AppCore;
use std::future::IntoFuture;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tokio::net::TcpListener;
use views::SessionStatus;

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
    /// Whether the Claude Code file channel is actively watching sessions.
    #[serde(rename = "watchingClaudeCode")]
    pub watching_claude_code: bool,
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
    /// True when the Claude Code file channel is active (the app is watching the
    /// local session transcripts). Set by the embedder after the watcher starts,
    /// surfaced at `GET /config` so the panel can show a "Watching Claude Code"
    /// indicator. Purely informational.
    pub watching_files: Arc<AtomicBool>,
    /// The plan the desktop app reported for the signed-in account, or Free when
    /// nobody is signed in — which is a fully supported state, since detection is
    /// local. The *effective* entitlement is derived from this plus the local
    /// trial; read it via [`AppState::entitlement`], never straight from here.
    pub account_plan: Arc<RwLock<Plan>>,
    /// When the local first-run Pro trial started (ms since epoch), if it has.
    /// Read from `app_meta` on startup — no account and no network involved.
    pub trial_started_ms: Arc<RwLock<Option<i64>>>,
    /// How many days of session history to keep on disk, or `None` for "forever".
    ///
    /// A *deletion* policy, not a display filter. The plan's "7 days of history"
    /// used to hide older sessions while every turn of them stayed on disk, so a
    /// user whose history had expired still had it. Applied on startup and whenever
    /// the setting changes.
    pub retention_days: Arc<RwLock<Option<u32>>>,
    /// The control API's access token. Every route but `/health` and the
    /// dashboard's own assets requires it; see [`auth`] for why localhost alone
    /// was never a boundary.
    pub token: auth::Token,
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
        // Production path only: stamp (or read back) the local trial start. Test
        // constructors deliberately skip this so plan gating stays deterministic.
        s.begin_trial_if_new();
        s
    }

    /// Enforce the retention window now.
    ///
    /// Called once at startup by every embedder, because retention that only ran
    /// when the setting changed would leave an app that is opened rarely holding
    /// months of history it had promised to delete. Returns how many sessions went.
    pub fn sweep_retention(&self) -> usize {
        let days = self.retention_days.read().ok().and_then(|d| *d);
        self.core
            .lock()
            .map(|mut c| c.apply_retention(days))
            .unwrap_or(0)
    }

    /// Set the retention window (embedders and tests).
    pub fn with_retention_days(self, days: Option<u32>) -> Self {
        if let Ok(mut w) = self.retention_days.write() {
            *w = days;
        }
        self
    }

    /// Start the local Pro trial on first launch, or read back the existing start
    /// stamp. No-op without a durable store (nothing to remember it in).
    ///
    /// The trial exists so a new user meets the full product before any wall.
    /// It is intentionally local and therefore resettable — requiring an account
    /// to unlock a trial would put back the signup gate we removed, and a trial
    /// is not a security boundary.
    pub fn begin_trial_if_new(&self) {
        let now = state::now_millis();
        let started = self
            .core
            .lock()
            .ok()
            .and_then(|c| c.trial_started_or_init(now));
        if let Ok(mut w) = self.trial_started_ms.write() {
            *w = started;
        }
    }

    /// Override the trial start (tests).
    pub fn with_trial_started(self, ms: Option<i64>) -> Self {
        if let Ok(mut w) = self.trial_started_ms.write() {
            *w = ms;
        }
        self
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

    /// Set the account plan (used by tests and the app embedder).
    pub fn with_plan(self, plan: Plan) -> Self {
        self.set_account_plan(plan);
        self
    }

    /// Record the plan reported for the signed-in account (Free when signed out).
    pub fn set_account_plan(&self, plan: Plan) {
        if let Ok(mut w) = self.account_plan.write() {
            *w = plan;
        }
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
            watching_claude_code: false,
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
            watching_files: Arc::new(AtomicBool::new(false)),
            account_plan: Arc::new(RwLock::new(Plan::default())),
            trial_started_ms: Arc::new(RwLock::new(None)),
            upstream: Arc::new(RwLock::new(upstream)),
            token: auth::Token::lazy(),
            retention_days: Arc::new(RwLock::new(retention_from_env())),
        }
    }

    /// Replace the control token (tests, and any embedder that wants to pin one).
    pub fn with_token(mut self, token: auth::Token) -> Self {
        self.token = token;
        self
    }

    /// The entitlement actually in force: the account plan, upgraded to
    /// [`Plan::Trial`] while the local first-run trial is still running.
    ///
    /// Derived on every read rather than cached, so an expiring trial downgrades
    /// on its own without anything having to notice the clock.
    pub fn entitlement(&self) -> Entitlement {
        let account = self.account_plan.read().map(|p| *p).unwrap_or_default();
        let trial = self.trial_started_ms.read().ok().and_then(|t| *t);
        let now = state::now_millis();
        let plan = entitlement::resolve_plan(account, trial, now);
        let mut ent = Entitlement::for_plan(plan)
            .with_trial_days_left(entitlement::trial_days_left(trial, now));
        // A trial is granted locally by design and needs no signature; what
        // `verified` answers is whether an *account* plan was proven.
        ent.verified = plan_token::verification_available();
        ent
    }

    /// Mark whether the Claude Code file channel is active (embedder-set).
    pub fn set_watching_files(&self, on: bool) {
        self.watching_files
            .store(on, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Open a durable SQLite store at `path`, or `None` on failure. Convenience for
/// embedders (e.g. the Tauri app) so they don't depend on `drifterr-store`.
pub fn open_store(path: &str) -> Option<drifterr_store::Store> {
    drifterr_store::Store::open(path).ok()
}

/// The default location Claude Code writes its session transcripts to
/// (`~/.claude/projects`), or `None` if the home directory can't be resolved.
/// Embedders use this to watch Claude Code with zero configuration.
pub fn default_claude_projects_dir() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|h| !h.is_empty())?;
    Some(std::path::Path::new(&home).join(".claude").join("projects"))
}

type RulesConstraint = drifterr_engine::baseline::Constraint;

/// Import the rules file of the project a session belongs to, and stage its
/// constraints for that session.
///
/// This is what makes the zero-config path actually zero-config: a developer with
/// a `CLAUDE.md` has already written their standing rules down, so Drifterr can
/// start from a real anchor instead of an empty form. The project is identified by
/// the `cwd` the transcript records — the app's own working directory is
/// meaningless, since it launches from Applications.
///
/// Cached per project directory, and staged only for sessions that don't exist
/// yet, so a rules file is read once and never resurrects a constraint the user
/// retired.
fn seed_project_rules(
    state: &AppState,
    cache: &Arc<Mutex<std::collections::HashMap<String, Vec<RulesConstraint>>>>,
    content: &str,
    session_id: &str,
) {
    let Some(cwd) = drifterr_adapters::claude_code::session_cwd(content) else {
        return;
    };
    let constraints = {
        let Ok(mut c) = cache.lock() else { return };
        c.entry(cwd.clone())
            .or_insert_with(|| {
                drifterr_adapters::rules_import::discover(std::path::Path::new(&cwd))
                    .map(|i| i.constraints)
                    .unwrap_or_default()
            })
            .clone()
    };
    if constraints.is_empty() {
        return;
    }
    if let Ok(mut core) = state.core.lock() {
        core.stage_imported_constraints(session_id, constraints);
    }
}

/// Watch a directory of Claude Code sessions and feed each into the shared engine
/// state — the SAME pipeline the HTTP proxy uses, so detection, notifications and
/// the panel all work identically for file-sourced sessions. Does an initial scan
/// so existing sessions show up immediately, then watches for changes. Returns the
/// live watcher, which the caller MUST keep alive for the lifetime of the process.
pub fn watch_claude_sessions(
    dir: &std::path::Path,
    state: AppState,
) -> Option<drifterr_adapters::RecommendedWatcher> {
    // Rules files are read once per project directory, not once per file event: a
    // busy session fires many change events per minute and the rules file rarely
    // moves. `None` caches "this project has no rules file" just as firmly.
    let rules_cache: Arc<Mutex<std::collections::HashMap<String, Vec<RulesConstraint>>>> =
        Arc::new(Mutex::new(std::collections::HashMap::new()));

    // Initial scan so already-open sessions are picked up on launch.
    for (path, conv) in drifterr_adapters::claude_code::scan_dir(dir) {
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        seed_project_rules(&state, &rules_cache, &content, &conv.session_id);
        if let Ok(mut core) = state.core.lock() {
            core.record_conversation(&conv);
        }
    }

    let ingest_state = state.clone();
    let ingest_cache = rules_cache.clone();
    let ingest = move |file: std::path::PathBuf| {
        let Ok(content) = std::fs::read_to_string(&file) else {
            return;
        };
        let stem = file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("session");
        if let Some(conv) = drifterr_adapters::claude_code::parse_session(&content, stem) {
            // Stage before recording: the import must be part of the session's
            // first baseline, not applied a turn later.
            seed_project_rules(&ingest_state, &ingest_cache, &content, &conv.session_id);
            if let Ok(mut core) = ingest_state.core.lock() {
                core.record_conversation(&conv);
            }
        }
    };

    match drifterr_adapters::watch_dir(dir, ingest) {
        Ok(w) => Some(w),
        Err(e) => {
            eprintln!("drifterr: could not watch {}: {e}", dir.display());
            None
        }
    }
}

/// Retention window from the environment, for the standalone proxy and as the
/// default the panel then edits. Unset means keep everything, which is what the
/// product did before this existed — so nobody's data changes meaning on upgrade.
fn retention_from_env() -> Option<u32> {
    std::env::var("DRIFTERR_RETENTION_DAYS")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|d| *d > 0)
}

pub mod control;
pub mod relay;

pub use control::control_router;
pub use relay::proxy_router;

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
