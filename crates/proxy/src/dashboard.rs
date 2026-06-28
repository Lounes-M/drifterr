//! Built-in dashboard: the control server serves the menubar panel's static
//! assets so it is usable in any browser (`http://<control-addr>/`) without a
//! build step — and the Tauri webview loads the same files.
//!
//! The canonical home of these assets is `apps/desktop/ui/`; they are embedded
//! here at compile time so the proxy binary is self-contained.

use axum::http::header;
use axum::response::{Html, IntoResponse, Response};

const INDEX: &str = include_str!("../../../apps/desktop/ui/index.html");
const STYLES: &str = include_str!("../../../apps/desktop/ui/styles.css");
const APP_JS: &str = include_str!("../../../apps/desktop/ui/app.js");

pub async fn index() -> Html<&'static str> {
    Html(INDEX)
}

pub async fn styles() -> Response {
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], STYLES).into_response()
}

pub async fn app_js() -> Response {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        APP_JS,
    )
        .into_response()
}
