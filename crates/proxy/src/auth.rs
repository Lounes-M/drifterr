//! Access control for the **control API**.
//!
//! # Why localhost is not a boundary
//!
//! The control API used to bind `127.0.0.1` and answer everything with
//! `Access-Control-Allow-Origin: *` and no credential. That reasoning — "it is
//! local, so only the user can reach it" — is wrong in the one place it matters:
//! the user's browser is *already inside* the boundary. Any page they have open
//! can `fetch("http://127.0.0.1:8788/anchor")`, and with a wildcard ACAO the
//! browser hands the response to that page. `/anchor`, `/status`, `/reanchor`,
//! `/journal` and `/report` all carry conversation content — the goal verbatim,
//! the offending span of a violation, the re-anchor snapshot. So a wildcard here
//! made every website a reader of the user's sessions, which is precisely the
//! thing the product promises cannot happen.
//!
//! # Two independent layers, because they stop different attacks
//!
//! 1. **Origin allowlist** ([`cors_headers`]). Browser-enforced. Stops a page
//!    from *reading* a response. It cannot stop the request being *sent* — a
//!    form post or a `no-cors` fetch still reaches the handler — so on its own it
//!    would leave every mutating route (`/entitlement`, `/judge`, `/ingest`)
//!    exposed to blind CSRF.
//! 2. **Bearer token** ([`Token`]). Checked on the server, so it does not care
//!    what the caller claims to be. It also forces a preflight for any
//!    cross-origin request (a custom header is never a "simple request"), and the
//!    preflight is answered by layer 1 — the two compose rather than overlap.
//!
//! Neither layer alone is sufficient; both are cheap.
//!
//! # Where the token lives
//!
//! One file, `<state dir>/token`, mode `0600` on Unix. Every Drifterr process
//! resolves it the same way ([`state_dir`]), which is what lets the `hook` and
//! `mcp` subcommands — separate processes, launched by the agent, not by us —
//! talk to a running app without any configuration. The desktop shell exports
//! `DRIFTERR_STATE_DIR` explicitly so its app-data dir and this resolution can
//! never drift apart.
//!
//! The token is a capability, not a secret shared with a server: it never leaves
//! the machine, and rotating it is `rm` plus a restart.

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::path::PathBuf;
use std::sync::Arc;

/// Header the panel and the extension send the token in. `Authorization: Bearer`
/// is accepted too, for `curl` and any generic client.
pub const TOKEN_HEADER: &str = "x-drifterr-token";

/// Filename of the token inside the state dir.
const TOKEN_FILE: &str = "token";

/// Bytes of entropy in a generated token. 32 bytes rendered as 64 hex chars.
const TOKEN_BYTES: usize = 32;

/// Routes reachable without a token.
///
/// Deliberately tiny, and deliberately *not* including anything that reads or
/// writes session state:
///
/// * `/health` — a liveness probe. Returns the fixed string `ok` and nothing else,
///   so it reveals only that Drifterr is running. The `hook` uses it to decide
///   whether to stay quiet, and it must work before pairing.
/// * The dashboard assets (`/`, `/app.js`, `/styles.css`, `/public/*`) — the
///   browser dashboard has to be able to load itself before it can authenticate.
///   These are served **without** CORS headers, so a foreign origin can request
///   them but never read the response, and `index.html` is the one place the
///   token is inlined (same-origin only).
fn is_public_path(path: &str) -> bool {
    matches!(
        path,
        "/health" | "/" | "/index.html" | "/app.js" | "/styles.css"
    ) || path.starts_with("/public/")
}

/// Origins allowed to read a control-API response.
///
/// * The two Tauri webview origins — the menubar panel is cross-origin to
///   `127.0.0.1:8788` by construction, so it needs an entry here.
/// * The control server's own origins, for the browser dashboard.
/// * Browser-extension origins, by scheme. An extension id is not known until the
///   extension is packed, so pinning one is not possible; the token is what
///   actually authorizes the extension, and the scheme check only keeps the
///   allowlist from being a wildcard.
///
/// `DRIFTERR_ALLOWED_ORIGINS` (comma-separated) adds to this, for a self-hosted
/// dashboard on another port.
pub fn origin_allowed(origin: &str) -> bool {
    const FIXED: [&str; 6] = [
        "tauri://localhost",
        "https://tauri.localhost",
        "http://127.0.0.1:8788",
        "http://localhost:8788",
        "http://127.0.0.1:1420", // `tauri dev`
        "http://localhost:1420",
    ];
    if FIXED.contains(&origin) {
        return true;
    }
    if origin.starts_with("chrome-extension://")
        || origin.starts_with("moz-extension://")
        || origin.starts_with("safari-web-extension://")
    {
        return true;
    }
    std::env::var("DRIFTERR_ALLOWED_ORIGINS")
        .ok()
        .into_iter()
        .flat_map(|v| {
            v.split(',')
                .map(|s| s.trim().to_string())
                .collect::<Vec<_>>()
        })
        .any(|allowed| !allowed.is_empty() && allowed == origin)
}

/// The shared control-API token.
///
/// Resolution is **lazy**: constructing one touches nothing, and the first read
/// consults the environment, then the token file, then generates and persists.
/// That laziness is not an optimization — it is what lets a caller (a test, an
/// embedder) pin an explicit token with [`Token::from_value`] and be certain the
/// real state directory was never read or written.
#[derive(Clone)]
pub struct Token(Arc<std::sync::OnceLock<String>>);

impl Token {
    /// A token that resolves itself the first time it is read.
    pub fn lazy() -> Self {
        Token(Arc::new(std::sync::OnceLock::new()))
    }

    /// Resolve now and return the value — for the binary, which prints it at
    /// startup so a developer can pair a `curl` without hunting for the file.
    pub fn resolve() -> Self {
        let t = Token::lazy();
        let _ = t.as_str();
        t
    }

    /// A fixed token. Never touches the filesystem, which is what makes it safe
    /// for tests and for an embedder that manages its own state directory.
    pub fn from_value(v: impl Into<String>) -> Self {
        let cell = std::sync::OnceLock::new();
        let _ = cell.set(v.into());
        Token(Arc::new(cell))
    }

    pub fn as_str(&self) -> &str {
        self.0.get_or_init(resolve_from_disk)
    }

    /// Constant-time comparison, so a caller cannot recover the token one byte at
    /// a time from response latency.
    fn matches(&self, presented: &str) -> bool {
        let a = self.as_str().as_bytes();
        let b = presented.as_bytes();
        // Fold every byte before deciding, with no early return on a mismatch.
        let mut diff = (a.len() ^ b.len()) as u8;
        for i in 0..a.len().max(b.len()) {
            let x = a.get(i).copied().unwrap_or(0);
            let y = b.get(i).copied().unwrap_or(0);
            diff |= x ^ y;
        }
        diff == 0
    }
}

/// `DRIFTERR_TOKEN`, else the token file, else generate one and persist it.
///
/// Generation is a fallback rather than the norm: every long-lived install reads
/// the same file back, so the panel does not have to re-pair on restart.
fn resolve_from_disk() -> String {
    if let Ok(t) = std::env::var("DRIFTERR_TOKEN") {
        let t = t.trim().to_string();
        if !t.is_empty() {
            return t;
        }
    }
    let path = token_path();
    if let Some(existing) = std::fs::read_to_string(&path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        return existing;
    }
    let fresh = generate();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::write(&path, &fresh).is_ok() {
        restrict(&path);
    } else {
        // No writable state dir (a read-only container, a locked-down profile).
        // The token still works for this process; it just will not survive a
        // restart, so the panel re-pairs. Failing closed would be worse: an app
        // that cannot start is not more secure than one that re-pairs.
        eprintln!(
            "drifterr: could not persist the control token to {} — it will change on restart",
            path.display()
        );
    }
    fresh
}

/// Pull the presented token out of either accepted header.
fn presented(headers: &HeaderMap) -> Option<String> {
    if let Some(v) = headers.get(TOKEN_HEADER).and_then(|v| v.to_str().ok()) {
        let v = v.trim();
        if !v.is_empty() {
            return Some(v.to_string());
        }
    }
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())?;
    let rest = auth
        .strip_prefix("Bearer ")
        .or_else(|| auth.strip_prefix("bearer "))?;
    let rest = rest.trim();
    (!rest.is_empty()).then(|| rest.to_string())
}

/// Directory Drifterr keeps install-scoped state in (the token, and the database
/// when the desktop shell is not choosing the path).
///
/// `DRIFTERR_STATE_DIR` wins — the desktop shell sets it to Tauri's own
/// `app_data_dir()` so the app and the `hook`/`mcp` subprocesses cannot disagree
/// about where to look. Otherwise this mirrors the platform convention Tauri
/// itself uses for the `com.drifterr.app` identifier.
pub fn state_dir() -> PathBuf {
    if let Ok(d) = std::env::var("DRIFTERR_STATE_DIR") {
        if !d.trim().is_empty() {
            return PathBuf::from(d);
        }
    }
    const ID: &str = "com.drifterr.app";
    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home)
                .join("Library/Application Support")
                .join(ID);
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata).join(ID);
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if let Ok(x) = std::env::var("XDG_DATA_HOME") {
            if !x.is_empty() {
                return PathBuf::from(x).join(ID);
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(".local/share").join(ID);
        }
    }
    PathBuf::from(".drifterr")
}

fn token_path() -> PathBuf {
    state_dir().join(TOKEN_FILE)
}

/// Read the token a running Drifterr is using, without starting one. Used by the
/// `hook` and `mcp` subcommands, which are separate short-lived processes.
///
/// Returns `None` rather than generating: a subprocess that minted its own token
/// would authenticate against nothing and simply be refused, and a confusing 401
/// is worse than the documented "Drifterr is not running" path.
pub fn read_token() -> Option<String> {
    if let Ok(t) = std::env::var("DRIFTERR_TOKEN") {
        let t = t.trim().to_string();
        if !t.is_empty() {
            return Some(t);
        }
    }
    std::fs::read_to_string(token_path())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 64 hex chars from the OS RNG.
///
/// Sourced from `/dev/urandom` (Unix) or `BCryptGenRandom` via `getrandom`-style
/// syscall on Windows — we avoid pulling a crate for this because the proxy's
/// dependency surface is itself part of the security story, and the fallback path
/// is never reached on a supported platform.
fn generate() -> String {
    let bytes = os_random(TOKEN_BYTES);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(unix)]
fn os_random(n: usize) -> Vec<u8> {
    use std::io::Read;
    let mut buf = vec![0u8; n];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        if f.read_exact(&mut buf).is_ok() {
            return buf;
        }
    }
    weak_random(n)
}

#[cfg(windows)]
fn os_random(n: usize) -> Vec<u8> {
    // `ProcessPrng` is the documented user-mode CSPRNG entry point on Windows 10+
    // and cannot fail. Declared directly so no crate is needed.
    #[link(name = "bcryptprimitives")]
    extern "system" {
        fn ProcessPrng(pbData: *mut u8, cbData: usize) -> i32;
    }
    let mut buf = vec![0u8; n];
    let ok = unsafe { ProcessPrng(buf.as_mut_ptr(), buf.len()) };
    if ok != 0 {
        return buf;
    }
    weak_random(n)
}

/// Last-resort entropy if the OS RNG is unavailable. Only reachable on a broken
/// system; mixes the clock, the pid and heap addresses so the result is still not
/// guessable from outside the machine, and the failure is announced.
fn weak_random(n: usize) -> Vec<u8> {
    eprintln!("drifterr: OS RNG unavailable — control token uses a weaker fallback");
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15)
        ^ (std::process::id() as u64).rotate_left(17)
        ^ (&SEED_ANCHOR as *const _ as u64);
    (0..n)
        .map(|_| {
            seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = seed;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            ((z ^ (z >> 31)) & 0xff) as u8
        })
        .collect()
}

/// A fixed item whose address varies with ASLR, used only by [`weak_random`].
static SEED_ANCHOR: u8 = 0;

/// Tighten the token file to owner-only. A no-op on Windows, where the app-data
/// directory is already per-user.
#[cfg(unix)]
fn restrict(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict(_path: &std::path::Path) {}

/// Attach CORS headers for `origin`, if it is allowed.
///
/// An origin that is not on the list gets **no** `Access-Control-Allow-Origin` at
/// all — which is what makes the browser refuse to hand the body to the caller.
/// We never echo an origin we have not checked, and we never send
/// `Allow-Credentials`, because the token is an explicit header and not a cookie.
pub fn cors_headers(origin: Option<&str>, res: &mut Response) {
    let h = res.headers_mut();
    // `Vary: Origin` regardless, so a shared cache can never serve one origin's
    // allowed response to another.
    h.insert(header::VARY, HeaderValue::from_static("Origin"));
    // Never let a foreign page frame the dashboard.
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
    h.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    let Some(origin) = origin else { return };
    if !origin_allowed(origin) {
        return;
    }
    if let Ok(v) = HeaderValue::from_str(origin) {
        h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, v);
        h.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST, OPTIONS"),
        );
        h.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("content-type, authorization, x-drifterr-token"),
        );
        h.insert(
            header::ACCESS_CONTROL_MAX_AGE,
            HeaderValue::from_static("600"),
        );
    }
}

/// The control API's single gate: CORS on the way out, token on the way in.
///
/// Ordering matters. The preflight is answered *before* the token check, because
/// a preflight by definition cannot carry one — refusing it would make every
/// cross-origin call fail even with a valid token.
pub async fn guard(
    axum::extract::State(token): axum::extract::State<Token>,
    req: Request,
    next: Next,
) -> Response {
    let origin = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let path = req.uri().path().to_string();

    if req.method() == Method::OPTIONS {
        let mut res = Response::new(Body::empty());
        cors_headers(origin.as_deref(), &mut res);
        return res;
    }

    if !is_public_path(&path) {
        let ok = presented(req.headers())
            .map(|p| token.matches(&p))
            .unwrap_or(false);
        if !ok {
            let mut res = (
                StatusCode::UNAUTHORIZED,
                // Say what to do, not just that it failed. This message is the
                // whole troubleshooting story for a mis-paired extension.
                "drifterr: this endpoint needs the local control token. \
                 Send it as `X-Drifterr-Token`. The panel shows it under \
                 Settings → Browser extension, and `drifterr-proxy token` prints it.\n",
            )
                .into_response();
            cors_headers(origin.as_deref(), &mut res);
            return res;
        }
    }

    let mut res = next.run(req).await;
    // Dashboard assets are same-origin only: no ACAO, so a foreign page can
    // request them and still never read one.
    if is_public_path(&path) && path != "/health" {
        let h = res.headers_mut();
        h.insert(header::VARY, HeaderValue::from_static("Origin"));
        h.insert("x-frame-options", HeaderValue::from_static("DENY"));
        h.insert(
            "x-content-type-options",
            HeaderValue::from_static("nosniff"),
        );
    } else {
        cors_headers(origin.as_deref(), &mut res);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_are_long_and_distinct() {
        let a = generate();
        let b = generate();
        assert_eq!(a.len(), TOKEN_BYTES * 2);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b, "two generated tokens must not collide");
    }

    #[test]
    fn comparison_is_exact() {
        let t = Token::from_value("abc123");
        assert!(t.matches("abc123"));
        assert!(!t.matches("abc124"));
        assert!(!t.matches("abc12"), "a prefix must not authenticate");
        assert!(!t.matches("abc1234"), "an extension must not authenticate");
        assert!(!t.matches(""));
    }

    #[test]
    fn tauri_and_extension_origins_are_allowed_others_are_not() {
        assert!(origin_allowed("tauri://localhost"));
        assert!(origin_allowed("https://tauri.localhost"));
        assert!(origin_allowed("http://127.0.0.1:8788"));
        assert!(origin_allowed("chrome-extension://abcdefghijklmnop"));
        assert!(origin_allowed("moz-extension://1234-5678"));

        assert!(!origin_allowed("https://evil.example"));
        assert!(!origin_allowed("http://localhost:3000"));
        assert!(!origin_allowed("null"));
        // The classic prefix-match bug: a lookalike host must not pass.
        assert!(!origin_allowed("https://tauri.localhost.evil.example"));
        assert!(!origin_allowed("http://127.0.0.1:8788.evil.example"));
    }

    #[test]
    fn extra_origins_come_from_the_environment() {
        std::env::set_var("DRIFTERR_ALLOWED_ORIGINS", "https://ops.internal, ");
        assert!(origin_allowed("https://ops.internal"));
        assert!(!origin_allowed("https://other.internal"));
        // An empty entry must never match the empty origin.
        assert!(!origin_allowed(""));
        std::env::remove_var("DRIFTERR_ALLOWED_ORIGINS");
    }

    #[test]
    fn only_health_and_dashboard_assets_are_public() {
        for p in [
            "/health",
            "/",
            "/index.html",
            "/app.js",
            "/styles.css",
            "/public/fonts/x.woff2",
        ] {
            assert!(is_public_path(p), "{p} should be public");
        }
        // Everything that touches a session must not be.
        for p in [
            "/status",
            "/sessions",
            "/anchor",
            "/reanchor",
            "/intent",
            "/journal",
            "/history",
            "/report",
            "/entitlement",
            "/judge",
            "/ingest",
            "/packs",
            "/feedback",
            "/prefs",
            "/team/share-preview",
            "/standing-orders",
            "/config",
            "/data/purge",
        ] {
            assert!(!is_public_path(p), "{p} must require the token");
        }
    }

    #[test]
    fn both_header_forms_are_accepted() {
        let mut h = HeaderMap::new();
        h.insert(TOKEN_HEADER, HeaderValue::from_static("tok"));
        assert_eq!(presented(&h).as_deref(), Some("tok"));

        let mut h = HeaderMap::new();
        h.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer tok"),
        );
        assert_eq!(presented(&h).as_deref(), Some("tok"));

        let mut h = HeaderMap::new();
        h.insert(header::AUTHORIZATION, HeaderValue::from_static("Basic tok"));
        assert_eq!(presented(&h), None, "only Bearer counts");

        assert_eq!(presented(&HeaderMap::new()), None);
    }
}
