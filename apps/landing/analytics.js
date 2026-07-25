// Funnel analytics for the **marketing site only**.
//
// Why this exists: the download → first-launch → activation funnel was entirely
// unmeasured, so the biggest drop-offs (an unsigned-build warning nobody read, a
// signup wall nobody wanted) were invisible. You cannot fix a funnel you cannot
// see.
//
// Why it does not violate local-first: this file never runs inside the app. It
// ships with the public website, and it records *page-level* events — which page,
// which button, which platform. It has no access to and never touches a
// conversation, a prompt, a goal or a constraint; those live on the user's machine
// and the app has no analytics at all. See /proof for the full boundary.
//
// The rules this module holds itself to:
//   * No cookies, no localStorage, no fingerprinting, no cross-site identifier.
//     There is no visitor id of any kind — events are counted, not people.
//   * No PII, and no free-text: event names and property values are validated
//     against an allowlist below, so a refactor cannot start shipping URLs with
//     query strings or email addresses in them.
//   * First-party only: events post to our own /api/event, never to a
//     third-party script. Nothing external is loaded, so there is no vendor with
//     a view of our visitors.
//   * Honors Do Not Track and Global Privacy Control. If either is set we send
//     nothing at all.
//   * Entirely optional: with no collector configured this is a no-op, which is
//     also what keeps the test suite hermetic.

/** Events we're willing to record. Anything else is dropped. */
const ALLOWED = new Set([
  "page_view",
  "download_click", // props: os
  "download_started", // props: os
  "download_failed", // props: os
  "install_help_view", // the unsigned-build notice scrolled into view
  "pricing_view",
  "plan_click", // props: plan
  "signup_start", // props: plan
  "faq_open", // props: index
]);

/** Property values are constrained to these, so nothing free-text escapes. */
const ALLOWED_PROPS = {
  os: new Set(["mac", "win", "linux", "deb"]),
  plan: new Set(["free", "pro", "team"]),
  page: new Set(["home", "download", "pricing", "proof", "signup", "login", "account", "other"]),
  index: null, // small integers, range-checked below
};

function collector() {
  if (typeof window === "undefined") return "";
  const url = window.DRIFTERR_ANALYTICS_URL;
  return typeof url === "string" ? url : "";
}

/// True when the visitor has asked not to be tracked. We check both the legacy
/// DNT header signal and GPC, and treat either as a hard no.
function optedOut() {
  if (typeof navigator === "undefined") return true;
  if (navigator.globalPrivacyControl === true) return true;
  const dnt =
    navigator.doNotTrack ??
    (typeof window !== "undefined" ? window.doNotTrack : undefined) ??
    navigator.msDoNotTrack;
  return dnt === "1" || dnt === "yes" || dnt === true;
}

/// Which page this is, as a coarse label — never the raw URL, so query strings
/// and path parameters can't leak into the event stream.
export function pageLabel(pathname) {
  const p = String(pathname || "").replace(/\.html$/, "").replace(/\/+$/, "");
  if (p === "" || p === "/index") return "home";
  if (p === "/download") return "download";
  if (p === "/proof") return "proof";
  if (p === "/signup") return "signup";
  if (p === "/login") return "login";
  if (p === "/account") return "account";
  return "other";
}

/// Drop anything not explicitly allowed. Returns a clean object, possibly empty.
export function sanitize(props) {
  const out = {};
  for (const [k, v] of Object.entries(props || {})) {
    if (!(k in ALLOWED_PROPS)) continue;
    const allowed = ALLOWED_PROPS[k];
    if (allowed === null) {
      // Numeric properties: small non-negative integers only.
      const n = Number(v);
      if (Number.isInteger(n) && n >= 0 && n < 100) out[k] = n;
      continue;
    }
    if (allowed.has(v)) out[k] = v;
  }
  return out;
}

/// Record one event. Fire-and-forget and failure-tolerant by construction: a
/// blocked or missing collector must never affect the page.
export function track(name, props) {
  try {
    if (!ALLOWED.has(name)) return false;
    const url = collector();
    if (!url || optedOut()) return false;
    const body = JSON.stringify({
      e: name,
      p: sanitize(props),
      // Coarse page + referrer host only. No full referrer URL, no path detail.
      pg: pageLabel(typeof location !== "undefined" ? location.pathname : ""),
      rf: referrerHost(),
    });
    if (navigator.sendBeacon) {
      // sendBeacon survives the page unload that a download click often triggers.
      return navigator.sendBeacon(url, new Blob([body], { type: "application/json" }));
    }
    fetch(url, { method: "POST", body, keepalive: true, headers: { "Content-Type": "application/json" } }).catch(
      () => {}
    );
    return true;
  } catch (_e) {
    return false;
  }
}

/// The referrer's host, so we can tell HN from Reddit from direct — never the
/// full referring URL, which can carry search terms and identifiers.
function referrerHost() {
  try {
    if (typeof document === "undefined" || !document.referrer) return "";
    const h = new URL(document.referrer).hostname;
    return h === location.hostname ? "" : h.slice(0, 80);
  } catch (_e) {
    return "";
  }
}

/// Fire `page_view`, plus one-shot view events for the sections that matter to
/// the funnel (the install-help notice and the pricing table).
export function init(doc) {
  if (!collector() || optedOut()) return;
  track("page_view");

  const once = (sel, event) => {
    const el = doc.querySelector(sel);
    if (!el || typeof IntersectionObserver === "undefined") return;
    const io = new IntersectionObserver(
      (entries) => {
        for (const e of entries) {
          if (e.isIntersecting) {
            track(event);
            io.disconnect();
          }
        }
      },
      { threshold: 0.35 }
    );
    io.observe(el);
  };
  once("#firstrun", "install_help_view");
  once("#pricing", "pricing_view");
}
