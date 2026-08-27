//! Drifterr menubar (tray) app — the native shell around the panel UI.
//!
//! The panel itself is the shared static UI in `apps/desktop/ui` (also served by
//! the proxy's control server). This shell does two extra things a browser tab
//! can't:
//!
//! 1. lives in the system tray with a **state-colored icon** that updates live;
//! 2. toggles the panel window open/closed on tray click, like a real menubar
//!    dropdown.
//!
//! A background task polls the control API (`/status`) every 1.5s, recolors the
//! tray, and fires a **native notification** when a session escalates into drift
//! (so you're warned even with the panel closed). The webview panel polls the
//! same endpoint for its detailed view.
//!
//! **Fusion (M-packaging):** the app embeds the Drifterr proxy as a library and
//! starts it in-process on launch, so installing one app gives you both the
//! local proxy/control API and the menubar — no separate process to run.
//!
//! NOTE: this crate is excluded from the Cargo workspace and is not compiled in
//! the headless CI used for the rest of the repo (it needs platform GUI libs).
//! Build it on a dev machine: `cargo tauri dev` (or `cargo tauri build`).

mod model;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, WindowEvent,
};
use tauri_plugin_updater::UpdaterExt;

// Menubar auto-hide bookkeeping. The panel hides when it loses focus (click
// outside), like a real menubar dropdown. Two millisecond timestamps guard the
// two races this creates:
//  * LAST_HIDDEN — a tray click while the panel is open first blurs it (→ hide);
//    without this the click's toggle would immediately re-open it.
//  * LAST_SHOWN — showing + focusing can emit a spurious blur mid-transition;
//    we ignore blur that lands within a beat of a show so the panel doesn't
//    flicker shut the instant it opens.
static LAST_HIDDEN_MS: AtomicU64 = AtomicU64::new(0);
static LAST_SHOWN_MS: AtomicU64 = AtomicU64::new(0);
static CLOCK: OnceLock<Instant> = OnceLock::new();
fn now_ms() -> u64 {
    CLOCK.get_or_init(Instant::now).elapsed().as_millis() as u64
}

const TRAY_GREEN: &[u8] = include_bytes!("../icons/tray-green.png");
const TRAY_AMBER: &[u8] = include_bytes!("../icons/tray-amber.png");
const TRAY_RED: &[u8] = include_bytes!("../icons/tray-red.png");
const TRAY_UNKNOWN: &[u8] = include_bytes!("../icons/tray-unknown.png");

const TRAY_ID: &str = "drifterr";

/// Anchor the panel to the tray icon so it pops up beside it (bottom-right on
/// Windows, top-right on macOS) instead of at a default cascade position. We
/// place the window's bottom-right corner just inside the click point on the
/// tray, then clamp into the icon's monitor work area so it never spills
/// off-screen or under the taskbar.
fn anchor_to_tray(win: &tauri::WebviewWindow, cursor: tauri::PhysicalPosition<f64>) {
    let size = win
        .outer_size()
        .unwrap_or(tauri::PhysicalSize::new(380, 600));
    let (w, h) = (size.width as f64, size.height as f64);
    let margin = 12.0;

    // Default: window sits up-and-left of the cursor (menubar-style).
    let mut x = cursor.x - w + margin;
    let mut y = cursor.y - h - margin;

    // Keep it on the monitor under the tray.
    if let Ok(Some(monitor)) = win.monitor_from_point(cursor.x, cursor.y) {
        let mp = monitor.position();
        let ms = monitor.size();
        let (min_x, min_y) = (mp.x as f64, mp.y as f64);
        let max_x = min_x + ms.width as f64 - w;
        let max_y = min_y + ms.height as f64 - h;
        x = x.clamp(min_x, max_x.max(min_x));
        y = y.clamp(min_y, max_y.max(min_y));
    } else {
        x = x.max(0.0);
        y = y.max(0.0);
    }

    let _ = win.set_position(tauri::PhysicalPosition::new(x as i32, y as i32));
}

/// Listen addresses for the embedded proxy (overridable via env).
fn proxy_addr() -> std::net::SocketAddr {
    std::env::var("DRIFTERR_PROXY_ADDR")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| "127.0.0.1:8787".parse().unwrap())
}
fn control_addr() -> std::net::SocketAddr {
    std::env::var("DRIFTERR_CONTROL_ADDR")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| "127.0.0.1:8788".parse().unwrap())
}

/// Start the embedded Drifterr proxy in-process. Persists to the app's data dir
/// when available. Best-effort: if the ports are taken (an external proxy is
/// already running), the menubar simply attaches to that one.
/// Hand the panel the control-API token.
///
/// The webview's origin is `tauri://localhost`, which is cross-origin to the
/// control server on `127.0.0.1:8788`, so the panel cannot be given the token in
/// its HTML the way the browser dashboard is.
///
/// This is a command rather than an injected script on purpose: an injected
/// global races the page's own scripts, and a panel that renders one 401 before
/// the token lands looks broken. A command the panel awaits during boot has no
/// ordering to get wrong.
///
/// Nothing crosses a trust boundary here — the token never leaves the machine,
/// and the only caller is a webview we build and ship.
#[tauri::command]
fn control_token() -> String {
    drifterr_proxy::auth::read_token().unwrap_or_default()
}

fn start_embedded_proxy(app: &tauri::App) {
    // Point the embedder at a semantic model if one is present — a bundled resource
    // first, then one the user downloaded on demand into app-data. A no-op when
    // neither exists or the build has no ONNX support: detection then runs on the
    // lexical embedder, which is a working configuration rather than a degraded one
    // (the eval set's goal-drift case is caught either way).
    model::activate_if_present(app.handle());

    let db = app
        .path()
        .app_data_dir()
        .ok()
        .map(|d| {
            let _ = std::fs::create_dir_all(&d);
            d.join("drifterr.sqlite")
        })
        .and_then(|p| p.to_str().map(str::to_string));

    // Pin the state directory before anything resolves a path from it.
    //
    // The control token lives here, and `drifterr-proxy hook` / `mcp` run as
    // separate processes launched by the agent, not by us — they find the token by
    // resolving this same directory. Exporting Tauri's own `app_data_dir()` rather
    // than letting each side guess is what makes the two impossible to disagree
    // about, on every platform and under every packaging layout.
    if let Ok(dir) = app.path().app_data_dir() {
        let _ = std::fs::create_dir_all(&dir);
        std::env::set_var("DRIFTERR_STATE_DIR", &dir);
    }

    // Tell the embedded proxy our real app version so /config (and the settings
    // view) reports the installed app's version, not the proxy crate's 0.0.1.
    std::env::set_var("DRIFTERR_APP_VERSION", env!("CARGO_PKG_VERSION"));

    // Put the local false-positive feedback log in the app-data dir (next to the
    // DB), unless the user overrode it. Stays local — never leaves the machine.
    if std::env::var_os("DRIFTERR_FEEDBACK_FILE").is_none() {
        if let Ok(dir) = app.path().app_data_dir() {
            let _ = std::fs::create_dir_all(&dir);
            std::env::set_var("DRIFTERR_FEEDBACK_FILE", dir.join("feedback.jsonl"));
        }
    }

    let store = db.and_then(|p| drifterr_proxy::open_store(&p));
    let cfg = drifterr_proxy::ProxyConfig::default();
    let state = drifterr_proxy::AppState::new(cfg, store);

    // Zero-config Claude Code channel: watch the local session transcripts and
    // feed them through the same engine as the HTTP proxy, so drift on a Claude
    // Code session lights the panel and fires notifications with no setup and no
    // API key. DRIFTERR_WATCH_DIR overrides the default (~/.claude/projects); we
    // only watch a directory that actually exists. The watcher is leaked so it
    // lives for the whole app session (there is no teardown before exit).
    let watch_dir = std::env::var("DRIFTERR_WATCH_DIR")
        .ok()
        .filter(|d| !d.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(drifterr_proxy::default_claude_projects_dir);
    if let Some(dir) = watch_dir.filter(|p| p.is_dir()) {
        if let Some(watcher) = drifterr_proxy::watch_claude_sessions(&dir, state.clone()) {
            state.set_watching_files(true);
            std::mem::forget(watcher);
        }
    }

    // Enforce the retention window at launch, before anything is served.
    state.sweep_retention();

    tauri::async_runtime::spawn(async move {
        if let Err(e) = drifterr_proxy::serve(proxy_addr(), control_addr(), state).await {
            eprintln!("drifterr: embedded proxy stopped: {e}");
        }
    });
}

/// Background update check, run shortly after launch. When a newer release is
/// published (verified against the bundled pubkey), tell the panel so it can
/// show its in-app "Update available" banner. Best-effort and silent on failure
/// — a network hiccup or no-update must never disturb the app.
///
/// Runs on launch and then every 6h, so an app left open for days still learns
/// about a new release. A given version is announced with an OS notification at
/// most once (so the 6h re-checks don't nag), and never when Do Not Disturb is on.
async fn update_loop(handle: tauri::AppHandle) {
    const RECHECK: Duration = Duration::from_secs(6 * 60 * 60);
    tokio::time::sleep(Duration::from_secs(4)).await; // let the app settle first
    let client = reqwest::Client::builder().no_proxy().build().ok();
    let base =
        std::env::var("DRIFTERR_CONTROL").unwrap_or_else(|_| "http://127.0.0.1:8788".to_string());
    let mut notified_version: Option<String> = None;

    loop {
        if let Ok(updater) = handle.updater() {
            if let Ok(Some(update)) = updater.check().await {
                // Always refresh the in-app banner.
                let _ = handle.emit("update://available", update.version.clone());
                // Fire an OS notification once per version, unless muted.
                let already = notified_version.as_deref() == Some(update.version.as_str());
                let muted = match &client {
                    Some(c) => notifications_muted(c, &base).await,
                    None => false,
                };
                if !already && !muted {
                    use tauri_plugin_notification::NotificationExt;
                    let _ = handle
                        .notification()
                        .builder()
                        .title(format!("Drifterr {} is ready", update.version))
                        .body("Open Drifterr and click Update to install — no reinstall needed.")
                        .show();
                    notified_version = Some(update.version.clone());
                }
            }
        }
        tokio::time::sleep(RECHECK).await;
    }
}

/// Read the Do Not Disturb preference from the control API (`/prefs`). Any error
/// → not muted (fail toward informing the user).
async fn notifications_muted(client: &reqwest::Client, base: &str) -> bool {
    let Ok(resp) = client.get(format!("{base}/prefs")).send().await else {
        return false;
    };
    resp.json::<serde_json::Value>()
        .await
        .ok()
        .and_then(|v| v.get("notificationsMuted").and_then(|m| m.as_bool()))
        .unwrap_or(false)
}

/// Download + install the pending update, then relaunch into it — the SaaS-style
/// "update in place, no reinstall" flow. Progress is streamed to the panel so it
/// can animate a bar. Invoked by the panel's Update button.
#[tauri::command]
async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    let update = app
        .updater()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?;
    let Some(update) = update else {
        // Nothing to install (already current) — tell the panel to dismiss.
        let _ = app.emit("update://none", ());
        return Ok(());
    };

    let progress = app.clone();
    update
        .download_and_install(
            move |downloaded, total| {
                let pct = match total {
                    Some(t) if t > 0 => (downloaded as f64 / t as f64) * 100.0,
                    _ => 0.0,
                };
                let _ = progress.emit("update://progress", pct);
            },
            || {},
        )
        .await
        .map_err(|e| e.to_string())?;

    // Installed — relaunch into the new version (diverges).
    app.restart()
}

/// Manually check for an update now (the settings "Check for updates" button).
/// Emits `update://available` (with version) or `update://none` so the panel can
/// react. Returns the new version string, or `None` when already current.
#[tauri::command]
async fn check_update_now(app: tauri::AppHandle) -> Result<Option<String>, String> {
    let update = app
        .updater()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?;
    match update {
        Some(u) => {
            let _ = app.emit("update://available", u.version.clone());
            Ok(Some(u.version))
        }
        None => {
            let _ = app.emit("update://none", ());
            Ok(None)
        }
    }
}

/// How the semantic embedding model is provisioned, for the settings view.
#[tauri::command]
fn semantic_model_status(app: tauri::AppHandle) -> model::ModelStatus {
    model::status(&app)
}

/// Fetch the semantic model on demand.
///
/// It used to be bundled by default: ~127 MB of install, paid by every user on every
/// update, to feed the signal that fires least — and since the goal signal's thresholds
/// became scale-free, the lexical embedder already catches the eval set's goal-drift
/// case. So nothing ships and a user who wants semantic similarity asks once.
///
/// A failed or unverifiable download changes nothing: the app carries on with the
/// lexical embedder, which is a working configuration rather than a broken one.
#[tauri::command]
async fn download_semantic_model(app: tauri::AppHandle) -> Result<String, String> {
    let dir = model::download(app.clone()).await?;
    // Take effect without a restart where possible.
    if std::env::var_os("DRIFTERR_EMBED_MODEL").is_none() {
        std::env::set_var("DRIFTERR_EMBED_MODEL", &dir);
    }
    Ok(dir.to_string_lossy().to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            install_update,
            check_update_now,
            semantic_model_status,
            download_semantic_model,
            control_token
        ])
        .setup(|app| {
            // Start the bundled proxy first so the panel has data to show.
            start_embedded_proxy(app);

            // Check for updates on launch and every few hours after; the panel
            // shows a banner and (unless muted) the OS gets a notification.
            tauri::async_runtime::spawn(update_loop(app.handle().clone()));

            let quit = MenuItem::with_id(app, "quit", "Quit Drifterr", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&quit])?;

            TrayIconBuilder::with_id(TRAY_ID)
                .icon(Image::from_bytes(TRAY_UNKNOWN)?)
                .tooltip("Drifterr: connecting…")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| {
                    if event.id.as_ref() == "quit" {
                        app.exit(0);
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    // Left-click toggles the panel, like a menubar dropdown.
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        position,
                        ..
                    } = event
                    {
                        if let Some(win) = tray.app_handle().get_webview_window("main") {
                            // A tray click that lands right after an auto-hide is
                            // the SAME gesture that blurred (and hid) the panel —
                            // treat it as "dismiss", not "re-open".
                            let just_auto_hid = now_ms()
                                .saturating_sub(LAST_HIDDEN_MS.load(Ordering::Relaxed))
                                < 250;
                            if win.is_visible().unwrap_or(false) {
                                let _ = win.hide();
                            } else if !just_auto_hid {
                                anchor_to_tray(&win, position);
                                let _ = win.show();
                                let _ = win.set_focus();
                                LAST_SHOWN_MS.store(now_ms(), Ordering::Relaxed);
                                let _ = tray.app_handle().emit("window://opened", ());
                            }
                        }
                    }
                })
                .build(app)?;

            // Live tray recoloring.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(poll_loop(handle));
            Ok(())
        })
        .on_window_event(|window, event| {
            match event {
                // Closing the panel just hides it — the app stays in the tray.
                WindowEvent::CloseRequested { api, .. } => {
                    api.prevent_close();
                    let _ = window.hide();
                }
                // Menubar behaviour: hide the moment the panel loses focus (a
                // click anywhere outside it). Because we HIDE rather than close,
                // the webview stays alive and the panel reopens exactly where the
                // user left it — same view, same scroll, same open settings. We
                // ignore a blur that arrives right after a show (spurious focus
                // churn during the open transition) so it can't flicker shut.
                WindowEvent::Focused(false) => {
                    let just_shown =
                        now_ms().saturating_sub(LAST_SHOWN_MS.load(Ordering::Relaxed)) < 150;
                    if !just_shown && window.is_visible().unwrap_or(false) {
                        LAST_HIDDEN_MS.store(now_ms(), Ordering::Relaxed);
                        let _ = window.hide();
                    }
                }
                _ => {}
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Drifterr");
}

/// A minimal snapshot of the current session, pulled from `/status`.
#[derive(Default, Clone)]
struct StatusBrief {
    /// "green" | "amber" | "red" (absent → no active session).
    state: Option<String>,
    /// Session id, so an alert fires per session, not per state flip.
    session: Option<String>,
    /// Human label of the triggering signal (e.g. "Constraints").
    signal: Option<String>,
    /// Evidence detail for the triggering signal.
    detail: Option<String>,
    /// Do Not Disturb — suppress OS notifications when true.
    muted: bool,
}

/// Poll the control API and recolor the tray to match the session state, and
/// fire a **native notification** the moment a session crosses into drift — the
/// whole point of "warn before the wall" when the panel is closed.
async fn poll_loop(app: tauri::AppHandle) {
    let base =
        std::env::var("DRIFTERR_CONTROL").unwrap_or_else(|_| "http://127.0.0.1:8788".to_string());
    let client = match reqwest::Client::builder().no_proxy().build() {
        Ok(c) => c,
        Err(_) => return,
    };

    // Remember the last alerted (session, state) so we notify only on a genuine
    // escalation, never every 1.5s tick.
    let mut last_alert: Option<(String, String)> = None;

    loop {
        let brief = fetch_status(&client, &base).await.unwrap_or_default();
        let state = brief.state.clone();
        if let Some(tray) = app.tray_by_id(TRAY_ID) {
            let (icon, tip) = match state.as_deref() {
                Some("green") => (TRAY_GREEN, "Drifterr: aligned"),
                Some("amber") => (TRAY_AMBER, "Drifterr: drifting (watch)"),
                Some("red") => (TRAY_RED, "Drifterr: drifting"),
                _ => (TRAY_UNKNOWN, "Drifterr: no active session"),
            };
            if let Ok(img) = Image::from_bytes(icon) {
                let _ = tray.set_icon(Some(img));
            }
            let _ = tray.set_tooltip(Some(tip));
        }

        maybe_notify(&app, &brief, &mut last_alert);
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }
}

/// What a notification would say, if one is warranted.
#[derive(Debug, PartialEq, Eq)]
struct Alert {
    title: String,
    body: String,
}

/// Decide whether a session's current state warrants interrupting the user.
///
/// Separated from the sending so it can be tested. This is the code that decides
/// whether someone is pulled out of what they are doing, and the rules are subtle
/// enough to get quietly wrong: RED always alerts, AMBER only from a calm state,
/// and neither re-alerts while the session sits at the same state. A regression
/// here does not crash anything — it just turns a tool people keep into one they
/// mute, which is the kind of failure nobody files a bug about.
///
/// `last_alert` is updated on every call regardless of the outcome, so the next
/// tick does not re-fire on a state we have already observed.
fn notification_for(
    brief: &StatusBrief,
    last_alert: &mut Option<(String, String)>,
) -> Option<Alert> {
    // Do Not Disturb suppresses OS notifications (the tray/panel still update).
    if brief.muted {
        return None;
    }
    let (state, session) = (brief.state.as_deref()?, brief.session.as_deref()?);

    // Same session already alerted at this state → nothing new to say.
    if last_alert.as_ref() == Some(&(session.to_string(), state.to_string())) {
        return None;
    }
    let prev_state = last_alert
        .as_ref()
        .filter(|(s, _)| s == session)
        .map(|(_, st)| st.clone());

    let escalating = match state {
        "red" => prev_state.as_deref() != Some("red"),
        "amber" => matches!(prev_state.as_deref(), None | Some("green")),
        _ => false, // green / unknown never alert
    };
    // Track the observed state regardless, so we don't re-alert on the next tick.
    *last_alert = Some((session.to_string(), state.to_string()));
    if !escalating {
        return None;
    }

    let signal = brief.signal.as_deref().unwrap_or("your intent");
    Some(if state == "red" {
        Alert {
            title: "Drifterr — off track".to_string(),
            body: match brief.detail.as_deref() {
                Some(d) if !d.is_empty() => format!("{signal}: {d}"),
                _ => format!("This session is drifting from {signal}."),
            },
        }
    } else {
        Alert {
            title: "Drifterr — starting to drift".to_string(),
            body: format!("Watch {signal} — the conversation is beginning to stray."),
        }
    })
}

/// Send the notification [`notification_for`] decided on, if any.
fn maybe_notify(
    app: &tauri::AppHandle,
    brief: &StatusBrief,
    last_alert: &mut Option<(String, String)>,
) {
    use tauri_plugin_notification::NotificationExt;
    let Some(alert) = notification_for(brief, last_alert) else {
        return;
    };
    let _ = app
        .notification()
        .builder()
        .title(alert.title)
        .body(alert.body)
        .show();
}

/// Fetch a compact snapshot of the current session, or `None` if unreachable /
/// idle. Reads the named triggering signal so a notification can say *why*.
async fn fetch_status(client: &reqwest::Client, base: &str) -> Option<StatusBrief> {
    let v: serde_json::Value = client
        .get(format!("{base}/status"))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    let cur = v.get("current")?;
    if cur.is_null() {
        return Some(StatusBrief::default());
    }
    let trig = cur.get("triggering");
    Some(StatusBrief {
        state: cur.get("state").and_then(|s| s.as_str()).map(String::from),
        session: cur
            .get("sessionId")
            .and_then(|s| s.as_str())
            .map(String::from),
        signal: trig
            .and_then(|t| t.get("signal"))
            .and_then(|s| s.as_str())
            .map(signal_label),
        detail: trig
            .and_then(|t| t.get("detail"))
            .and_then(|s| s.as_str())
            .map(String::from),
        muted: v
            .get("notificationsMuted")
            .and_then(|m| m.as_bool())
            .unwrap_or(false),
    })
}

/// Human label for a signal id (mirrors the panel's SIGNAL_LABELS).
fn signal_label(id: &str) -> String {
    match id {
        "constraint" => "Constraints",
        "saturation" => "Saturation",
        "goal_alignment" => "Goal alignment",
        "decision_coherence" => "Decision coherence",
        "degradation" => "Degradation",
        other => other,
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brief(state: &str, session: &str) -> StatusBrief {
        StatusBrief {
            state: Some(state.to_string()),
            session: Some(session.to_string()),
            signal: Some("Constraints".to_string()),
            detail: Some("constraint c1 violated".to_string()),
            muted: false,
        }
    }

    /// The rule that keeps the app worth leaving running: alert on escalation,
    /// then stay quiet. A tool that re-notifies every 1.5s poll gets muted, and a
    /// muted tool detects nothing.
    #[test]
    fn a_session_alerts_once_per_escalation_not_once_per_poll() {
        let mut last = None;

        let first = notification_for(&brief("red", "s1"), &mut last);
        assert!(first.is_some(), "entering RED must alert");
        assert!(first.unwrap().body.contains("constraint c1 violated"));

        // The poll loop runs every 1.5 seconds. None of these may alert.
        for _ in 0..5 {
            assert!(
                notification_for(&brief("red", "s1"), &mut last).is_none(),
                "staying RED must not re-alert"
            );
        }
    }

    /// De-escalation is not news. RED → AMBER means things got *better*, and
    /// telling someone about it is the definition of crying wolf.
    #[test]
    fn falling_back_to_amber_stays_quiet() {
        let mut last = None;
        assert!(notification_for(&brief("red", "s1"), &mut last).is_some());
        assert!(
            notification_for(&brief("amber", "s1"), &mut last).is_none(),
            "RED → AMBER is an improvement, not an alert"
        );
        // And recovering to green, then drifting again, is genuinely new.
        assert!(notification_for(&brief("green", "s1"), &mut last).is_none());
        assert!(
            notification_for(&brief("amber", "s1"), &mut last).is_some(),
            "GREEN → AMBER is a fresh escalation"
        );
    }

    /// AMBER from a calm start is worth one alert; AMBER while already amber is not.
    #[test]
    fn amber_alerts_from_calm_only() {
        let mut last = None;
        assert!(notification_for(&brief("amber", "s1"), &mut last).is_some());
        assert!(notification_for(&brief("amber", "s1"), &mut last).is_none());
        // Escalating amber → red still alerts: it is worse than before.
        assert!(notification_for(&brief("red", "s1"), &mut last).is_some());
    }

    /// Green never interrupts anyone, from any prior state.
    #[test]
    fn green_never_alerts() {
        for prior in ["red", "amber", "green"] {
            let mut last = None;
            let _ = notification_for(&brief(prior, "s1"), &mut last);
            assert!(
                notification_for(&brief("green", "s1"), &mut last).is_none(),
                "green must never alert (from {prior})"
            );
        }
    }

    /// A different session drifting is new information even at the same state.
    #[test]
    fn a_second_session_alerts_on_its_own() {
        let mut last = None;
        assert!(notification_for(&brief("red", "s1"), &mut last).is_some());
        assert!(
            notification_for(&brief("red", "s2"), &mut last).is_some(),
            "a different session's drift is its own event"
        );
    }

    /// Do Not Disturb must suppress the notification *and* leave the tracking
    /// alone — otherwise un-muting would fire a burst for states already passed.
    #[test]
    fn do_not_disturb_suppresses_without_swallowing_the_escalation() {
        let mut last = None;
        let mut muted = brief("red", "s1");
        muted.muted = true;
        assert!(notification_for(&muted, &mut last).is_none());
        assert!(last.is_none(), "a muted poll must not record state");

        // Un-mute while still red: the user has not been told yet, so they should be.
        assert!(notification_for(&brief("red", "s1"), &mut last).is_some());
    }

    /// With no session there is nothing to say, and nothing to remember.
    #[test]
    fn an_idle_status_alerts_nothing() {
        let mut last = None;
        let idle = StatusBrief {
            state: None,
            session: None,
            signal: None,
            detail: None,
            muted: false,
        };
        assert!(notification_for(&idle, &mut last).is_none());
        assert!(last.is_none());
    }

    /// A red alert with no evidence still has to say something useful.
    #[test]
    fn a_missing_detail_falls_back_to_a_readable_sentence() {
        let mut last = None;
        let mut b = brief("red", "s1");
        b.detail = None;
        let alert = notification_for(&b, &mut last).expect("should alert");
        assert!(alert.body.contains("Constraints"));
        assert!(
            !alert.body.contains("None"),
            "no debug formatting in a user-facing string"
        );

        let mut last = None;
        let mut b = brief("red", "s2");
        b.signal = None;
        b.detail = None;
        let alert = notification_for(&b, &mut last).expect("should alert");
        assert!(alert.body.contains("your intent"));
    }

    #[test]
    fn signal_labels_are_human_and_unknown_ids_pass_through() {
        assert_eq!(signal_label("goal_alignment"), "Goal alignment");
        assert_eq!(signal_label("constraint"), "Constraints");
        // A signal the shell has not been taught about must still render as
        // something, rather than vanishing from the notification.
        assert_eq!(signal_label("brand_new_signal"), "brand_new_signal");
    }

    /// The addresses the shell binds and polls. A bad override should fall back
    /// to the default rather than panic the app on launch.
    #[test]
    fn addresses_come_from_the_environment_and_tolerate_nonsense() {
        std::env::set_var("DRIFTERR_CONTROL_ADDR", "127.0.0.1:9999");
        assert_eq!(control_addr().port(), 9999);

        std::env::set_var("DRIFTERR_CONTROL_ADDR", "not an address");
        assert_eq!(control_addr().port(), 8788, "a bad value must fall back");

        std::env::remove_var("DRIFTERR_CONTROL_ADDR");
        assert_eq!(control_addr().port(), 8788);
        assert_eq!(proxy_addr().port(), 8787);
    }
}
