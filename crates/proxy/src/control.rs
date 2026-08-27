//! The control API: every endpoint the panel, the extension and the CLI talk to.
//!
//! Split out of `lib.rs`, which had grown to hold the relay, the shared state, the
//! file watcher and thirty-odd handlers at once. The seam is the one that matters
//! operationally: everything here is off the request path and may fail, while the
//! relay next door is on it and may not.
//!
//! Access control lives in [`crate::auth`] and is applied to this whole router, so
//! a handler added below is authenticated by default rather than by remembering.

use super::*;

/// The control API the panel, the extension and the CLI read, plus the built-in
/// dashboard that serves the panel's assets.
///
/// Every route here goes through [`auth::guard`], so a handler added below is
/// authenticated and origin-checked by default. Making a route public is an
/// explicit edit to `auth::is_public_path` with a reason, which is the right way
/// round: the previous default was a wildcard CORS policy and no credential, and
/// every endpoint added under it inherited that silently.
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
        .route("/intent/confirm", post(confirm_constraint_handler))
        .route("/anchor", get(anchor_handler))
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
        .route("/data/forget", post(forget_handler))
        .route("/diagnostics", get(diagnostics_handler))
        .route("/history", get(history_handler))
        .route("/journal", get(journal_handler))
        .route("/report", get(report_handler))
        .route("/packs", get(list_packs_handler))
        .route("/packs/apply", post(apply_pack_handler))
        .route("/packs/export", get(export_pack_handler))
        .route("/team/share-preview", get(team_share_preview_handler))
        .route("/standing-orders", get(standing_orders_handler))
        .route("/standing-orders/promote", post(promote_handler))
        .route("/feedback", post(feedback_handler))
        .route("/ingest", post(ingest_handler))
        .route("/public/{*path}", get(public_handler))
        .route("/health", get(|| async { "ok" }))
        .layer(middleware::from_fn_with_state(
            state.token.clone(),
            auth::guard,
        ))
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
    // Reflect whether the Claude Code file channel is currently watching.
    meta.watching_claude_code = app
        .watching_files
        .load(std::sync::atomic::Ordering::Relaxed);
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
    let cutoff = retention_cutoff_ms(&app);
    Json(
        sessions
            .into_iter()
            // Retention is the Free plan's real limit: recent sessions always
            // readable, older ones behind Pro. Sessions with no recorded activity
            // (last_ts == 0) are kept — they're new, not old.
            .filter(|s| cutoff.is_none_or(|c| s.last_ts == 0 || s.last_ts >= c))
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
/// Query for `GET /report`: an optional window in days.
#[derive(Deserialize)]
struct ReportQuery {
    #[serde(default)]
    days: Option<i64>,
}

/// The weekly drift report, rendered locally as markdown.
///
/// The retention loop the product lacked: without an accumulated view, a tool that
/// correctly stays silent most of the time is indistinguishable from a broken one.
/// Entirely local — this reads the SQLite store and touches no network.
#[derive(Serialize)]
struct ReportResponse {
    markdown: String,
    flags: usize,
    sessions: usize,
    reanchors: usize,
    /// Nothing worth reporting; the caller should stay quiet rather than notify.
    #[serde(rename = "quietWeek")]
    quiet_week: bool,
}

async fn report_handler(State(app): State<AppState>, Query(q): Query<ReportQuery>) -> Response {
    // Clamp the window: a negative or absurd span would produce a meaningless report.
    let days = q.days.unwrap_or(7).clamp(1, 365);
    let out = app
        .core
        .lock()
        .ok()
        .and_then(|c| c.weekly_report(days * 86_400_000));
    match out {
        Some(r) => Json(ReportResponse {
            markdown: r.markdown,
            flags: r.flags,
            sessions: r.sessions,
            reanchors: r.reanchors,
            quiet_week: r.quiet_week,
        })
        .into_response(),
        // No durable store ⇒ no history. Say so rather than render an empty report
        // that reads as "nothing drifted".
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            "no local database — session history is not being persisted",
        )
            .into_response(),
    }
}

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

// --- rule packs -------------------------------------------------------------
//
// A pack is the one artefact in Drifterr that composes over time: rules you've settled
// on, in a file you own, portable between projects and tools. See
// `drifterr_engine::pack` for why packs carry natural-language intent rather than
// compiled regexes.

/// One pack in the catalogue.
#[derive(Serialize)]
struct PackSummary {
    id: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    /// How many of its rules the engine can check deterministically.
    enforceable: usize,
    /// Rule ids that would import as advisory only — stated up front so a user is never
    /// left believing a rule is enforced when it isn't.
    advisory: Vec<String>,
    rules: Vec<String>,
    /// "builtin" today; user-installed packs arrive with the same shape.
    source: &'static str,
}

async fn list_packs_handler() -> Json<Vec<PackSummary>> {
    let out = drifterr_engine::pack::builtin()
        .into_iter()
        .map(|(id, p)| {
            let applied = p.apply(id);
            PackSummary {
                enforceable: applied.enforced.len() - applied.advisory.len(),
                advisory: applied.advisory,
                rules: p.rules.iter().map(|r| r.text.clone()).collect(),
                id: id.to_string(),
                name: p.name,
                description: p.description,
                source: "builtin",
            }
        })
        .collect();
    Json(out)
}

/// `POST /packs/apply` — apply a built-in pack by id, or an inline pack body.
#[derive(Deserialize)]
struct ApplyPackBody {
    #[serde(default)]
    id: String,
    #[serde(default)]
    session: Option<String>,
    /// An inline pack, for importing a file the user was handed.
    #[serde(default)]
    pack: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct ApplyPackResponse {
    applied: usize,
    /// Rules that landed as advisory rather than enforced.
    advisory: Vec<String>,
}

async fn apply_pack_handler(
    State(app): State<AppState>,
    Json(body): Json<ApplyPackBody>,
) -> Response {
    // An inline pack goes through the same validation as a file on disk — a pack from
    // outside is untrusted input, and version/size checks are the point.
    let resolved = match body.pack {
        Some(v) => match drifterr_engine::pack::Pack::from_json(&v.to_string()) {
            Ok(p) => Some(("imported".to_string(), p)),
            Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
        },
        None => drifterr_engine::pack::builtin()
            .into_iter()
            .find(|(id, _)| *id == body.id)
            .map(|(id, p)| (id.to_string(), p)),
    };
    let Some((pack_id, pack)) = resolved else {
        return (StatusCode::NOT_FOUND, "no such pack").into_response();
    };
    let total = pack.rules.len();
    let advisory = match app.core.lock() {
        Ok(mut core) => core.apply_pack(body.session.as_deref(), &pack_id, &pack),
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "busy").into_response(),
    };
    Json(ApplyPackResponse {
        applied: total,
        advisory,
    })
    .into_response()
}

/// `GET /packs/export` — the user's promoted standing orders as a portable pack.
#[derive(Deserialize)]
struct ExportQuery {
    #[serde(default)]
    name: Option<String>,
    /// When set, render the pack as markdown ready to paste into a rules file instead of
    /// JSON. This is the cross-tool direction: telling the *agent* the rules, not just
    /// the watcher.
    #[serde(default)]
    markdown: Option<bool>,
}

async fn export_pack_handler(
    State(app): State<AppState>,
    Query(q): Query<ExportQuery>,
) -> Response {
    let name = q
        .name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "My standing orders".to_string());
    let pack = match app.core.lock() {
        Ok(core) => core.export_pack(&name),
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "busy").into_response(),
    };
    if q.markdown.unwrap_or(false) {
        return (
            [(header::CONTENT_TYPE, "text/markdown; charset=utf-8")],
            pack.to_markdown(),
        )
            .into_response();
    }
    ([(header::CONTENT_TYPE, "application/json")], pack.to_json()).into_response()
}

// --- team sharing -----------------------------------------------------------
//
// `GET /team/share-preview` returns *exactly* what a share would upload, so the user can
// read it before anything leaves. The upload itself is done by the layer that holds the
// account session — see `crate::team` for why this crate deliberately cannot do it.

#[derive(Deserialize)]
struct SharePreviewQuery {
    /// Pack ids to include. Sharing is per-pack and opt-in; omitting this shares counts
    /// only, which is the safer default.
    #[serde(default)]
    packs: Option<String>,
    #[serde(default)]
    days: Option<u32>,
}

#[derive(Serialize)]
struct SharePreview {
    /// The exact payload, or `null` when the plan does not include team sharing.
    #[serde(skip_serializing_if = "Option::is_none")]
    payload: Option<team::SharePayload>,
    /// One sentence naming what the filter withheld, for the panel.
    #[serde(skip_serializing_if = "Option::is_none")]
    withheld: Option<String>,
    /// False when the plan has no team sharing — the panel shows the upsell rather than
    /// an empty payload that reads like a bug.
    entitled: bool,
}

async fn team_share_preview_handler(
    State(app): State<AppState>,
    Query(q): Query<SharePreviewQuery>,
) -> Response {
    if !app.entitlement().team_sharing {
        return Json(SharePreview {
            payload: None,
            withheld: None,
            entitled: false,
        })
        .into_response();
    }

    let days = q.days.unwrap_or(14).clamp(1, team::MAX_PERIOD_DAYS);
    let wanted: Vec<String> = q
        .packs
        .as_deref()
        .map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let packs: Vec<drifterr_engine::pack::Pack> = drifterr_engine::pack::builtin()
        .into_iter()
        .filter(|(id, _)| wanted.iter().any(|w| w == id))
        .map(|(_, p)| p)
        .collect();

    let since = state::now_millis() - i64::from(days) * 86_400_000;
    let counts = app
        .core
        .lock()
        .map(|core| core.flag_counts_since(since))
        .unwrap_or_default();

    let payload = team::build(packs, &counts, days);
    let withheld = payload.withheld.explain();
    Json(SharePreview {
        payload: Some(payload),
        withheld,
        entitled: true,
    })
    .into_response()
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
        .and_then(|mut core| core.reanchor(q.session.as_deref()));
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

/// The machine-readable anchor (`GET /anchor`) — the baseline exactly as the engine holds
/// it, constraints and their rules included.
///
/// This exists alongside `/intent` rather than replacing it because the two have different
/// jobs. `/intent` is for the human: flattened, rules hidden, only active constraints.
/// `/anchor` is for a tool that must reach the *same verdict as the engine* — most notably
/// the MCP server, where an agent checks its own work before returning it. Handing that
/// consumer labels instead of rules would let it report a confident pass on rules it never
/// actually evaluated.
async fn anchor_handler(State(app): State<AppState>, Query(q): Query<ReanchorQuery>) -> Response {
    match app
        .core
        .lock()
        .ok()
        .and_then(|core| core.baseline_of(q.session.as_deref()))
    {
        Some(baseline) => Json(baseline).into_response(),
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
    /// Days of history kept on disk; `null` means forever.
    #[serde(rename = "retentionDays")]
    retention_days: Option<u32>,
    /// How many sessions are stored right now, so the panel's delete control can
    /// name what it is about to remove instead of asking for blind confirmation.
    #[serde(rename = "storedSessions")]
    stored_sessions: usize,
}

fn prefs_of(app: &AppState) -> Prefs {
    Prefs {
        notifications_muted: app.notifications_muted.load(Ordering::Relaxed),
        retention_days: app.retention_days.read().ok().and_then(|d| *d),
        stored_sessions: app
            .core
            .lock()
            .map(|c| c.stored_session_count())
            .unwrap_or(0),
    }
}

async fn get_prefs_handler(State(app): State<AppState>) -> Json<Prefs> {
    Json(prefs_of(&app))
}

#[derive(Deserialize)]
struct SetPrefs {
    #[serde(default, rename = "notificationsMuted")]
    notifications_muted: bool,
    /// Absent leaves retention unchanged; `null` means keep forever. Distinguishing
    /// the two matters: a panel that only sends the mute toggle must not silently
    /// switch retention off.
    #[serde(default, rename = "retentionDays")]
    retention_days: Option<Option<u32>>,
}

/// Set preferences (Do Not Disturb, retention). The native shell reads the result
/// from `/status` and suppresses OS notifications accordingly.
///
/// Shortening the retention window takes effect immediately rather than at the next
/// startup: a user who has just decided they want less of their history on disk
/// should not have to keep the app running until tomorrow to get it.
async fn set_prefs_handler(State(app): State<AppState>, Json(body): Json<SetPrefs>) -> Json<Prefs> {
    app.notifications_muted
        .store(body.notifications_muted, Ordering::Relaxed);
    if let Some(days) = body.retention_days {
        if let Ok(mut w) = app.retention_days.write() {
            *w = days;
        }
        if let Ok(mut core) = app.core.lock() {
            core.apply_retention(days);
        }
    }
    Json(prefs_of(&app))
}

/// `POST /data/forget` — delete stored conversations.
///
/// The control a privacy-first product has to have and did not. Everything
/// Drifterr sees is written to local SQLite in full, and until now there was no
/// way to get rid of any of it: no per-session delete, no retention, no wipe.
/// "It never leaves your machine" is only half a promise if the other half is
/// "and it stays there forever whether you like it or not".
#[derive(Deserialize)]
struct ForgetReq {
    /// A single session id. Omit (or pass `all: true`) to delete everything.
    #[serde(default)]
    session: Option<String>,
    #[serde(default)]
    all: bool,
}

#[derive(Serialize)]
struct ForgetResult {
    deleted: usize,
    #[serde(rename = "storedSessions")]
    stored_sessions: usize,
}

async fn forget_handler(State(app): State<AppState>, Json(body): Json<ForgetReq>) -> Response {
    let Ok(mut core) = app.core.lock() else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "busy").into_response();
    };
    let deleted = match (&body.session, body.all) {
        (Some(id), false) => usize::from(core.forget_session(id)),
        // `all` must be explicit. A malformed body that deserialized to "no session
        // named" would otherwise wipe everything, which is not a failure mode a
        // delete endpoint gets to have.
        (_, true) => core.forget_everything(),
        (None, false) => {
            return (
                StatusCode::BAD_REQUEST,
                "pass `session` to delete one, or `all: true` to delete everything",
            )
                .into_response()
        }
    };
    let stored = core.stored_session_count();
    Json(ForgetResult {
        deleted,
        stored_sessions: stored,
    })
    .into_response()
}

/// `GET /diagnostics` — everything a support conversation needs, and nothing else.
///
/// Drifterr has no crash reporting and no telemetry, by design. The cost of that
/// is that a launch failure on one OS version is invisible to us and a user's bug
/// report is "it doesn't work". This is the honest way to close that gap without
/// touching the privacy position: the user presses a button, sees exactly what it
/// contains, and decides whether to paste it anywhere.
///
/// So the shape is the point. Counts, states, versions and configuration —
/// never a goal, a constraint text, a span, a prompt, a file path or a session id.
/// `tests/egress.rs` asserts that, because a diagnostics endpoint is exactly where
/// conversation content would end up leaking by accident.
#[derive(Serialize)]
struct Diagnostics {
    version: String,
    /// Target triple, so "only on Windows" stops being a guess.
    platform: String,
    provider: String,
    persisted: bool,
    judge: String,
    #[serde(rename = "autoReanchor")]
    auto_reanchor: bool,
    #[serde(rename = "autoIntent")]
    auto_intent: bool,
    #[serde(rename = "watchingClaudeCode")]
    watching_claude_code: bool,
    plan: String,
    #[serde(rename = "entitlementVerified")]
    entitlement_verified: bool,
    #[serde(rename = "liveSessions")]
    live_sessions: usize,
    #[serde(rename = "storedSessions")]
    stored_sessions: usize,
    #[serde(rename = "retentionDays")]
    retention_days: Option<u32>,
    /// How many sessions are in each state right now — enough to tell "it never
    /// fires" from "it fires constantly" without seeing a single one of them.
    #[serde(rename = "stateCounts")]
    state_counts: std::collections::BTreeMap<String, usize>,
    #[serde(rename = "embedderSemantic")]
    embedder_semantic: bool,
}

async fn diagnostics_handler(State(app): State<AppState>) -> Json<Diagnostics> {
    let ent = app.entitlement();
    let (live, counts) = app
        .core
        .lock()
        .map(|c| {
            let all = c.all();
            let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
            for s in &all {
                *counts
                    .entry(format!("{:?}", s.state).to_lowercase())
                    .or_default() += 1;
            }
            (all.len(), counts)
        })
        .unwrap_or_default();
    Json(Diagnostics {
        version: app.meta.version.clone(),
        platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        provider: app.meta.provider.clone(),
        persisted: app.meta.persisted,
        judge: app
            .judge
            .read()
            .map(|j| j.label())
            .unwrap_or_else(|_| "unknown".into()),
        auto_reanchor: app.auto_reanchor_on(),
        auto_intent: app.auto_intent_on(),
        watching_claude_code: app.watching_files.load(Ordering::Relaxed),
        plan: format!("{:?}", ent.plan).to_lowercase(),
        entitlement_verified: ent.verified,
        live_sessions: live,
        stored_sessions: app
            .core
            .lock()
            .map(|c| c.stored_session_count())
            .unwrap_or(0),
        retention_days: app.retention_days.read().ok().and_then(|d| *d),
        state_counts: counts,
        embedder_semantic: std::env::var("DRIFTERR_EMBED_MODEL")
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false),
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

/// Confirm a proposed (imported) constraint, so it is enforced like a stated one.
///
/// The other half of the proposal contract. An imported rule flags at AMBER and
/// says "confirm it to enforce"; this is where that sentence leads. Without it the
/// safety net would just be a permanent downgrade, and users would learn to ignore
/// a class of warning that can never be resolved.
async fn confirm_constraint_handler(
    State(app): State<AppState>,
    Json(body): Json<RetireConstraint>,
) -> Response {
    match app
        .core
        .lock()
        .ok()
        .and_then(|mut c| c.confirm_constraint(body.session.as_deref(), &body.id))
    {
        Some(view) => Json(view).into_response(),
        None => (StatusCode::NOT_FOUND, "unknown constraint").into_response(),
    }
}

#[derive(Deserialize)]
struct FeedbackReq {
    session: Option<String>,
    note: Option<String>,
}

/// Where user-reported false positives are appended (local JSONL). Configurable
/// via `DRIFTERR_FEEDBACK_FILE`; the desktop app points it at the app-data dir.
/// Defaults to the working directory for the standalone proxy.
fn feedback_file() -> std::path::PathBuf {
    std::env::var_os("DRIFTERR_FEEDBACK_FILE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("drifterr-feedback.jsonl"))
}

/// `POST /feedback` — the user says "this wasn't drift". We capture the fired
/// signal + the baseline it was measured against as a local false-positive
/// sample (never leaves the machine) that can later seed the eval corpus.
async fn feedback_handler(State(app): State<AppState>, Json(body): Json<FeedbackReq>) -> Response {
    let sample = app
        .core
        .lock()
        .ok()
        .and_then(|c| c.feedback_sample(body.session.as_deref(), body.note.clone()));
    let Some(sample) = sample else {
        return (StatusCode::NOT_FOUND, "no active signal to correct").into_response();
    };
    let Ok(mut line) = serde_json::to_string(&sample) else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "serialize").into_response();
    };
    line.push('\n');
    use std::io::Write;
    let path = feedback_file();
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(line.as_bytes()))
    {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => {
            eprintln!(
                "drifterr: could not write feedback to {}: {e}",
                path.display()
            );
            (StatusCode::INTERNAL_SERVER_ERROR, "write failed").into_response()
        }
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

/// Oldest `last_ts` still readable under the active plan's retention, or `None`
/// when retention is unlimited.
fn retention_cutoff_ms(app: &AppState) -> Option<i64> {
    let days = app.entitlement().history_days? as i64;
    Some(state::now_millis() - days * 86_400_000)
}

async fn entitlement_handler(State(app): State<AppState>) -> Json<Entitlement> {
    Json(app.entitlement())
}

/// Set the active plan (the desktop app calls this after `/me`). We derive the
/// capabilities from the plan id — never from client-sent flags.
#[derive(Deserialize)]
struct SetEntitlement {
    /// The plan, asserted. Accepted only by a build with no entitlement key
    /// configured — see [`plan_token`] for why an assertion is not a boundary.
    #[serde(default)]
    plan: String,
    /// A signed plan assertion from the accounts backend. Required whenever this
    /// build can verify one.
    #[serde(default, rename = "planToken")]
    plan_token: Option<String>,
}

/// Record the plan for the signed-in account.
///
/// This used to store whatever it was told. A signed token means the proxy now
/// *verifies* a plan rather than trusting one — see [`plan_token`], including an
/// honest account of what that does and does not buy for local software.
///
/// A build with no key configured (development, a self-hoster) still accepts the
/// plain assertion, and `GET /entitlement` reports `verified: false` so the state
/// is never ambiguous.
async fn set_entitlement_handler(
    State(app): State<AppState>,
    Json(body): Json<SetEntitlement>,
) -> Response {
    let plan = if plan_token::verification_available() {
        let Some(token) = body.plan_token.as_deref() else {
            return (
                StatusCode::BAD_REQUEST,
                "this build verifies entitlements: send `planToken` from /me, not `plan`",
            )
                .into_response();
        };
        match plan_token::verify(token, state::now_millis()) {
            Ok(claims) => Plan::from_id(&claims.plan),
            // Downgrade to Free rather than refuse outright. A lapsed or unreadable
            // token means we cannot establish a paid plan, and the honest response
            // to that is the free tier — which still runs the whole detection loop.
            // Refusing would leave the previous plan in force, which is the one
            // outcome an expiry must not produce.
            Err(e) => {
                eprintln!("drifterr: plan token refused ({e}) — falling back to Free");
                Plan::Free
            }
        }
    } else {
        Plan::from_id(&body.plan)
    };

    app.set_account_plan(plan);
    // Re-derive: an account on Free while the local trial is still running stays
    // on trial capabilities.
    Json(app.entitlement()).into_response()
}

async fn sessions_handler(State(app): State<AppState>) -> Json<Vec<SessionStatus>> {
    let sessions = app.core.lock().map(|c| c.all()).unwrap_or_default();
    Json(sessions)
}
