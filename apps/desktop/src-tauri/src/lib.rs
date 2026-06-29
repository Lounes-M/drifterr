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
    Manager, WindowEvent,
};

const TRAY_GREEN: &[u8] = include_bytes!("../icons/tray-green.png");
const TRAY_AMBER: &[u8] = include_bytes!("../icons/tray-amber.png");
const TRAY_RED: &[u8] = include_bytes!("../icons/tray-red.png");
const TRAY_UNKNOWN: &[u8] = include_bytes!("../icons/tray-unknown.png");

const TRAY_ID: &str = "drifterr";

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            // Start the bundled proxy first so the panel has data to show.
            start_embedded_proxy(app);

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
                        ..
                    } = event
                    {
                        if let Some(win) = tray.app_handle().get_webview_window("main") {
                            if win.is_visible().unwrap_or(false) {
                                let _ = win.hide();
                            } else {
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
