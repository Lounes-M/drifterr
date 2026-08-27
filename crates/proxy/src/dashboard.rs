//! Built-in dashboard: the control server serves the menubar panel's static
//! assets so it is usable in any browser (`http://<control-addr>/`) without a
//! build step — and the Tauri webview loads the same files.
//!
//! The canonical home of these assets is `apps/desktop/ui/`; they are embedded
//! here at compile time so the proxy binary is self-contained.

use axum::extract::State;
use axum::http::header;
use axum::response::{Html, IntoResponse, Response};

const INDEX: &str = include_str!("../../../apps/desktop/ui/index.html");
const STYLES: &str = include_str!("../../../apps/desktop/ui/styles.css");
const APP_JS: &str = include_str!("../../../apps/desktop/ui/app.js");

/// Serve the panel, with the control token inlined.
///
/// This is the browser-dashboard's pairing step, and it is safe precisely because
/// this route carries no `Access-Control-Allow-Origin`: a foreign page can issue
/// the request but the browser will not let it read the response, so the token
/// only ever reaches a same-origin document. The Tauri webview does not use this
/// path — it is cross-origin to the control server, so the shell injects the
/// token before the page loads instead.
///
/// The placeholder is replaced rather than appended so there is exactly one
/// definition of the global, and `index.html` stays valid standalone (its own
/// value is the empty string, which the panel treats as "not paired").
pub async fn index(State(app): State<crate::AppState>) -> Html<String> {
    let escaped: String = app
        .token
        .as_str()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    Html(INDEX.replace(TOKEN_PLACEHOLDER, &escaped))
}

/// The literal in `index.html` that [`index`] swaps the live token into. Kept as a
/// constant so a rename of the global in the HTML fails this build rather than
/// silently shipping an unpaired dashboard.
const TOKEN_PLACEHOLDER: &str = "__DRIFTERR_CONTROL_TOKEN__";

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
