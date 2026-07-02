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
//! A background task polls the control API (`/status`) every 1.5s and recolors
//! the tray. The webview panel polls the same endpoint for its detailed view.
//!
//! **Fusion (M-packaging):** the app embeds the Drifterr proxy as a library and
//! starts it in-process on launch, so installing one app gives you both the
//! local proxy/control API and the menubar — no separate process to run.
//!
//! NOTE: this crate is excluded from the Cargo workspace and is not compiled in
//! the headless CI used for the rest of the repo (it needs platform GUI libs).
//! Build it on a dev machine: `cargo tauri dev` (or `cargo tauri build`).

use std::time::Duration;
use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, WindowEvent,
};
use tauri_plugin_updater::UpdaterExt;

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
fn start_embedded_proxy(app: &tauri::App) {
    // If a semantic model was bundled as a resource (models/embed/model.onnx)
    // and the user hasn't pointed DRIFTERR_EMBED_MODEL somewhere else, use it.
    // No-op when absent, or when the app wasn't built with `--features semantic`
    // (the embedder simply falls back to the local lexical model).
    if std::env::var_os("DRIFTERR_EMBED_MODEL").is_none() {
        if let Ok(res) = app.path().resource_dir() {
            let model = res.join("models").join("embed");
            if model.join("model.onnx").exists() {
                std::env::set_var("DRIFTERR_EMBED_MODEL", &model);
            }
        }
    }

    let db = app
        .path()
        .app_data_dir()
        .ok()
        .map(|d| {
            let _ = std::fs::create_dir_all(&d);
            d.join("drifterr.sqlite")
        })
        .and_then(|p| p.to_str().map(str::to_string));

    let store = db.and_then(|p| drifterr_proxy::open_store(&p));
    let cfg = drifterr_proxy::ProxyConfig::default();
    let state = drifterr_proxy::AppState::new(cfg, store);

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
async fn check_for_update(handle: tauri::AppHandle) {
    tokio::time::sleep(Duration::from_secs(4)).await;
    let Ok(updater) = handle.updater() else { return };
    if let Ok(Some(update)) = updater.check().await {
        let _ = handle.emit("update://available", update.version.clone());
    }
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![install_update])
        .setup(|app| {
            // Start the bundled proxy first so the panel has data to show.
            start_embedded_proxy(app);

            // Check for updates in the background; the panel shows the banner.
            tauri::async_runtime::spawn(check_for_update(app.handle().clone()));

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
                            if win.is_visible().unwrap_or(false) {
                                let _ = win.hide();
                            } else {
                                anchor_to_tray(&win, position);
                                let _ = win.show();
                                let _ = win.set_focus();
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
            // Closing the panel just hides it — the app stays in the tray.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Drifterr");
}

/// Poll the control API and recolor the tray to match the session state.
async fn poll_loop(app: tauri::AppHandle) {
    let base = std::env::var("DRIFTERR_CONTROL")
        .unwrap_or_else(|_| "http://127.0.0.1:8788".to_string());
    let client = match reqwest::Client::builder().no_proxy().build() {
        Ok(c) => c,
        Err(_) => return,
    };

    loop {
        let state = fetch_state(&client, &base).await;
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
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }
}

/// Fetch `current.state` from the control API, or `None` if unreachable / idle.
async fn fetch_state(client: &reqwest::Client, base: &str) -> Option<String> {
    let v: serde_json::Value = client
        .get(format!("{base}/status"))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    v.get("current")?
        .get("state")?
        .as_str()
        .map(|s| s.to_string())
}
