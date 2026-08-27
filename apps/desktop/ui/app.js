// Drifterr menubar panel logic.
//
// The rendering is split into pure functions (`stateInfo`, `signalLabel`,
// `render`, `renderError`) so they can be driven by tests without a live
// server. The polling loop only auto-starts in a real browser document.
//
// Data shape (from the control API `GET /status`):
//   { current: SessionStatus | null, sessions: SessionStatus[] }
// where SessionStatus = { sessionId, model, state, saturationPct, exact,
//   triggering?: {signal,state,detail,constraintId?,span?}, signals: [...] }

export const STATES = {
  green: { label: "Aligned", cls: "green", blurb: "On track with your intent." },
  amber: { label: "Watch", cls: "amber", blurb: "Starting to drift — keep an eye on it." },
  red: { label: "Drifting", cls: "red", blurb: "Off track. Consider re-anchoring." },
};

export function stateInfo(state) {
  return STATES[state] || { label: "Unknown", cls: "unknown", blurb: "" };
}

// --- icons -----------------------------------------------------------------
// Inline Lucide (lucide.dev, ISC) glyphs — no CDN, no emoji, CSP-safe and
// offline. `stroke: currentColor` so each icon inherits its button's colour.
// Keep the raw path data verbatim from Lucide so the shapes stay pixel-accurate.
const ICON_PATHS = {
  "rotate-ccw": '<path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/>',
  x: '<path d="M18 6 6 18"/><path d="m6 6 12 12"/>',
  eye: '<path d="M2.062 12.348a1 1 0 0 1 0-.696 10.75 10.75 0 0 1 19.876 0 1 1 0 0 1 0 .696 10.75 10.75 0 0 1-19.876 0"/><circle cx="12" cy="12" r="3"/>',
  sparkles: '<path d="M11.017 2.814a1 1 0 0 1 1.966 0l1.051 5.558a2 2 0 0 0 1.594 1.594l5.558 1.051a1 1 0 0 1 0 1.966l-5.558 1.051a2 2 0 0 0-1.594 1.594l-1.051 5.558a1 1 0 0 1-1.966 0l-1.051-5.558a2 2 0 0 0-1.594-1.594l-5.558-1.051a1 1 0 0 1 0-1.966l5.558-1.051a2 2 0 0 0 1.594-1.594z"/><path d="M20 2v4"/><path d="M22 4h-4"/><circle cx="4" cy="20" r="2"/>',
};

/// Build an inline SVG string for a Lucide icon name. `size` in px (default 16).
export function icon(name, size = 16) {
  const paths = ICON_PATHS[name];
  if (!paths) return "";
  return (
    `<svg class="ic ic-${name}" width="${size}" height="${size}" viewBox="0 0 24 24" ` +
    'fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" ' +
    `stroke-linejoin="round" aria-hidden="true">${paths}</svg>`
  );
}

const SIGNAL_LABELS = {
  constraint: "Constraints",
  saturation: "Saturation",
  goal_alignment: "Goal alignment",
  decision_coherence: "Decision coherence",
  degradation: "Degradation",
};

export function signalLabel(signal) {
  return SIGNAL_LABELS[signal] || signal;
}

/// Color band for the saturation bar — independent of the overall state so the
/// bar always tells the literal occupancy story.
export function saturationClass(pct) {
  if (pct >= 80) return "red";
  if (pct >= 55) return "amber";
  return "green";
}

/// Where to reach the control API. When the page is served by the control
/// server itself, same-origin relative calls work. In the Tauri webview the
/// origin is the custom protocol — `tauri://localhost` on macOS but
/// `http://tauri.localhost` on Windows/Linux — which does NOT serve the control
/// API, so we must fall back to the default localhost port there. (A naive
/// `startsWith("http")` check wrongly treats `http://tauri.localhost` as a
/// servable origin, which is why the panel failed to connect on Windows/Linux.)
/// A `window.DRIFTERR_API` override wins over both.
/**
 * The control-API pairing token, or "" when the panel has not been paired.
 *
 * Read live rather than cached: the Tauri shell injects it before page scripts
 * run, a browser dashboard served straight from the control server gets it from
 * the substituted placeholder in index.html, and a test harness sets it later.
 * One accessor keeps all three paths identical.
 */
export function apiToken() {
  const t = typeof window !== "undefined" ? window.DRIFTERR_TOKEN : "";
  return typeof t === "string" ? t : "";
}

/**
 * Add the control token to a fetch init.
 *
 * Every call into the control API goes through this. It exists as one function
 * rather than a header spelled out at each call site so a new endpoint cannot
 * quietly ship unauthenticated - tests/config.test.mjs scans this file and fails
 * if any apiBase() request skips it.
 *
 * Sending it as a custom header (not a cookie, not a query parameter) is
 * deliberate: a custom header makes every cross-origin request non-simple, so
 * the browser must preflight it and the server's origin allowlist answers that
 * preflight. A token in the URL would leak into logs and history instead.
 */
export function withAuth(init) {
  const base = init || {};
  const tok = apiToken();
  if (!tok) return base;
  return { ...base, headers: { ...(base.headers || {}), "X-Drifterr-Token": tok } };
}

/**
 * Make sure the pairing token is available before the first control-API call.
 *
 * Under the Tauri shell the panel is cross-origin to the control server, so the
 * token cannot be inlined in the HTML — the shell exposes it as a command and
 * this awaits it once at boot. Awaiting a command rather than reading a global
 * an injected script may or may not have set yet is what keeps the first poll
 * from rendering a spurious "not reachable" while the token is still in flight.
 *
 * Idempotent and never throws: a failure leaves the token empty, the first call
 * gets a 401, and the panel says it needs pairing — which is the truth.
 */
export async function ensureToken() {
  if (typeof window === "undefined") return "";
  if (apiToken()) return apiToken();
  const tauri = window.__TAURI__ || window.__TAURI_INTERNALS__;
  if (!tauri) return "";
  try {
    const invoke = window.__TAURI__?.core?.invoke || window.__TAURI_INTERNALS__?.invoke;
    if (!invoke) return "";
    const t = await invoke("control_token");
    if (typeof t === "string" && t) window.DRIFTERR_TOKEN = t;
  } catch (_e) {
    /* Shell too old, or the command is unavailable — the 401 path explains it. */
  }
  return apiToken();
}

export function apiBase() {
  if (typeof window !== "undefined" && window.DRIFTERR_API) return window.DRIFTERR_API;
  const isTauri =
    typeof window !== "undefined" && (window.__TAURI__ || window.__TAURI_INTERNALS__);
  const host = typeof location !== "undefined" ? location.hostname : "";
  if (
    !isTauri &&
    host !== "tauri.localhost" &&
    typeof location !== "undefined" &&
    location.origin &&
    location.origin.startsWith("http")
  ) {
    return location.origin;
  }
  return "http://127.0.0.1:8788";
}

/// Report whether the last re-anchor actually held.
///
/// Three honest states, and the third one matters: "still checking" is shown rather
/// than rounded up to success. A re-anchor success rate that counts undecided cases
/// as wins is the same species of invented number as the "52 min saved" stat this
/// mechanism exists to replace.
export function renderReanchorOutcome(doc, mark) {
  const el = doc.getElementById("reanchor-outcome");
  if (!el) return;
  if (!mark) {
    el.hidden = true;
    return;
  }
  el.hidden = false;
  const broke = Number.isFinite(mark.brokeAgainAtTurn);
  const held = mark.heldTurns || 0;
  const cause = mark.constraintId ? `“${mark.constraintId}”` : mark.signal;

  let cls, badge, text;
  if (broke) {
    cls = "broke";
    badge = "Didn't hold";
    text = `${cause} came back on turn ${mark.brokeAgainAtTurn + 1}. Consider restating it more explicitly, or retiring it if you've changed your mind.`;
  } else if (held >= 2) {
    cls = "held";
    badge = "Re-anchor held";
    text = `${cause} has stayed clear for ${held} turn${held > 1 ? "s" : ""} since you re-anchored.`;
  } else {
    cls = "pending";
    badge = "Checking";
    text = `Watching whether ${cause} stays clear — ${held} turn${held === 1 ? "" : "s"} so far.`;
  }
  el.className = "reanchor-outcome session-only " + cls;
  setText(doc, "ro-badge", badge);
  setText(doc, "ro-text", text);
}

/// Show the effective plan from a `/status` entitlement.
///
/// The proxy resolves the plan, including the local first-run trial, so this is
/// the honest answer for a signed-out user too. A running trial reads as
/// "Pro trial · 6d" rather than "Pro", and never shows the upgrade nudge — you
/// already have everything.
export function renderEffectivePlan(doc, ent) {
  const plan = ent && ent.plan;
  if (!plan) return;
  const trialDays = ent.trialDaysLeft;
  const label =
    plan === "trial"
      ? `Pro trial${Number.isFinite(trialDays) ? ` · ${trialDays}d` : ""}`
      : plan.charAt(0).toUpperCase() + plan.slice(1);
  const isFree = plan === "free";
  const pill = doc.getElementById("plan-pill");
  if (pill) {
    pill.hidden = false;
    pill.textContent = label;
    pill.classList.toggle("free", isFree);
    pill.classList.toggle("trial", plan === "trial");
  }
  const nudge = doc.getElementById("upgrade-nudge");
  if (nudge) nudge.hidden = !isFree;
  // Near the end of the trial, say what is about to be lost rather than leaving
  // the downgrade to be discovered.
  const expiring = plan === "trial" && Number.isFinite(trialDays) && trialDays <= 3;
  const soon = doc.getElementById("trial-ending");
  if (soon) {
    soon.hidden = !expiring;
    if (expiring) {
      setText(
        doc,
        "trial-ending-text",
        trialDays <= 0
          ? "Your Pro trial ends today — the drift map and auto re-anchor switch off after that. Detection keeps running."
          : `${trialDays} day${trialDays > 1 ? "s" : ""} of Pro left. After that the drift map and auto re-anchor switch off — detection keeps running.`
      );
    }
  }
}

/// Render a full status payload into the document. Pure w.r.t. the network:
/// give it a document and data, it mutates the DOM.
export function render(doc, data) {
  hide(doc, "error");
  const cur = data && data.current;
  const body = doc.getElementById("app-body");

  if (!cur) {
    // No live session: hide the metric sections (empty bars/dashes would just be
    // noise) and show only the header + intent card + the empty prompt.
    if (body) body.classList.add("no-session");
    hide(doc, "intent-shift");
    hide(doc, "activity");
    showEmpty(doc, true);
    setText(doc, "state-label", "No session");
    setText(doc, "blurb", "Waiting for activity.");
    setClass(doc, "dot", "dot unknown");
    setText(doc, "meta", "");
    return;
  }
  if (body) body.classList.remove("no-session");
  showEmpty(doc, false);

  const info = stateInfo(cur.state);
  setClass(doc, "dot", "dot " + info.cls);
  setText(doc, "state-label", info.label);
  setText(doc, "blurb", info.blurb);

  // Auto-intent goal-shift prompt: did the goal deliberately pivot, or is this
  // drift? The user decides.
  const shiftEl = doc.getElementById("intent-shift");
  if (shiftEl) {
    const sh = cur.intentShift;
    if (sh && sh.to) {
      shiftEl.hidden = false;
      setText(doc, "ishift-to", sh.to);
    } else {
      shiftEl.hidden = true;
    }
  }

  // Triggering signal — the named cause.
  const trigger = doc.getElementById("trigger");
  if (cur.triggering) {
    trigger.hidden = false;
    trigger.className = "trigger " + (cur.triggering.state === "amber" ? "amber" : "");
    setText(doc, "trigger-signal", signalLabel(cur.triggering.signal));
    setText(doc, "trigger-detail", cur.triggering.detail || "");
    const spanEl = doc.getElementById("trigger-span");
    if (cur.triggering.span) {
      spanEl.hidden = false;
      spanEl.textContent = cur.triggering.span;
    } else {
      spanEl.hidden = true;
    }
  } else {
    trigger.hidden = true;
    // No active trigger ⇒ drop any stale re-anchor snapshot.
    const re = doc.getElementById("reanchor");
    if (re) re.hidden = true;
  }

  renderReanchorOutcome(doc, cur.reanchor);

  // Drift score (0–100 display aggregate).
  const drift = clampPct(cur.driftScore);
  const dfill = doc.getElementById("drift-fill");
  if (dfill) {
    dfill.style.width = drift + "%";
    dfill.className = "bar-fill " + saturationClass(drift);
    setText(doc, "drift-meta", `${drift} / 100`);
  }

  // Drift map — sparkline of drift score over the recorded turns. Gated: the
  // proxy withholds history unless the plan unlocks it, so we lock the section
  // and prompt to upgrade instead of showing an empty chart.
  const ent = (data && data.entitlement) || {};

  // The proxy is the authority on the *effective* plan: it resolves the local
  // trial, which a signed-out user has with no account at all. So the pill and
  // the upgrade nudge follow /status, not the auth module.
  renderEffectivePlan(doc, ent);

  const mapSection = doc.getElementById("drift-map-section");
  const mapLocked = ent.driftMap === false;
  if (mapSection) mapSection.classList.toggle("locked", mapLocked);
  const mapLock = doc.getElementById("map-lock");
  if (mapLock) mapLock.hidden = !mapLocked;
  const map = doc.getElementById("drift-map");
  if (map) {
    map.innerHTML = "";
    if (mapLocked) {
      setText(doc, "map-meta", "");
    } else {
      const hist = Array.isArray(cur.history) ? cur.history : [];
      for (const v of hist.slice(-40)) {
        const s = clampPct(v);
        const b = doc.createElement("span");
        b.className = "map-bar " + saturationClass(s);
        b.style.height = Math.max(8, s) + "%";
        b.title = s + " / 100";
        map.appendChild(b);
      }
      setText(doc, "map-meta", hist.length ? `${hist.length} turn${hist.length > 1 ? "s" : ""}` : "no turns yet");
    }
  }

  // Saturation bar.
  const pct = clampPct(cur.saturationPct);
  const fill = doc.getElementById("sat-fill");
  fill.style.width = pct + "%";
  fill.className = "bar-fill " + saturationClass(pct);
  setText(doc, "sat-meta", `${pct}% · ${cur.exact ? "exact" : "estimated"}`);

  // Per-signal list.
  const list = doc.getElementById("signals");
  list.innerHTML = "";
  for (const s of cur.signals || []) {
    list.appendChild(signalRow(doc, s));
  }

  setText(doc, "meta", `${cur.model || "?"} · ${cur.sessionId || ""}`);
}

export function renderError(doc, message) {
  const el = doc.getElementById("error");
  if (el) {
    el.hidden = false;
    el.textContent = message;
  }
}

/// Signals allowed to drive RED. Mirrors `HARD_SIGNALS` in the engine and the eval
/// harness — a deterministic constraint violation and context saturation are facts;
/// everything else is advisory.
const HARD_SIGNALS = new Set(["constraint", "saturation"]);

function signalRow(doc, s) {
  const li = doc.createElement("li");
  li.className = "signal-row";
  const dot = doc.createElement("span");
  dot.className = "mini " + (stateInfo(s.state).cls);
  const body = doc.createElement("div");
  body.className = "signal-body";

  const head = doc.createElement("div");
  head.className = "signal-head";
  const name = doc.createElement("span");
  name.className = "signal-name";
  name.textContent = signalLabel(s.signal);
  head.appendChild(name);
  // Say which kind it is, so an amber reads as "advisory" rather than "weak alarm".
  const kind = doc.createElement("span");
  const isHard = HARD_SIGNALS.has(s.signal);
  kind.className = "signal-kind " + (isHard ? "hard" : "soft");
  kind.textContent = isHard ? "hard" : "soft";
  head.appendChild(kind);

  const detail = doc.createElement("span");
  detail.className = "signal-detail";
  detail.textContent = s.detail || "";
  body.appendChild(head);
  body.appendChild(detail);

  // Evidence, per signal rather than only for the one named as the cause. This is
  // what makes a flag checkable instead of something to be taken on trust.
  if (s.constraintId || s.span) {
    const ev = doc.createElement("div");
    ev.className = "signal-evidence";
    if (s.constraintId) {
      const c = doc.createElement("span");
      c.className = "ev-chip";
      c.textContent = s.constraintId;
      ev.appendChild(c);
    }
    if (s.span) {
      const sp = doc.createElement("code");
      sp.className = "ev-span";
      sp.textContent = s.span;
      ev.appendChild(sp);
    }
    body.appendChild(ev);
  }

  li.appendChild(dot);
  li.appendChild(body);
  return li;
}

// --- small DOM helpers -----------------------------------------------------

function setText(doc, id, text) {
  const el = doc.getElementById(id);
  if (el) el.textContent = text;
}
function setClass(doc, id, cls) {
  const el = doc.getElementById(id);
  if (el) el.className = cls;
}
function hide(doc, id) {
  const el = doc.getElementById(id);
  if (el) el.hidden = true;
}
function showEmpty(doc, on) {
  const el = doc.getElementById("empty");
  if (el) el.hidden = !on;
  // Hide the signal-heavy sections when there's nothing to show.
  for (const id of ["trigger"]) hide(doc, id);
}
function clampPct(n) {
  n = Number(n) || 0;
  return Math.max(0, Math.min(100, Math.round(n)));
}

// --- polling loop (browser only) -------------------------------------------

// --- settings view ---------------------------------------------------------

/// Render the effective config into the settings view. `cfg` is the `/config`
/// payload, or `null` when it couldn't be fetched.
export function renderConfig(doc, cfg) {
  setText(doc, "cfg-upstream", cfg ? cfg.openaiUpstream : "—");
  setText(doc, "cfg-judge", cfg ? cfg.judge : "—");
  setText(doc, "cfg-storage", cfg ? (cfg.persisted ? "SQLite (persisted)" : "In-memory") : "—");
  setText(doc, "cfg-version", cfg ? "v" + cfg.version : "—");
  // "Watching Claude Code" confidence chip on the main panel — reflects the live
  // file channel. Shown regardless of whether a session is active yet.
  const watching = doc.getElementById("watching");
  if (watching) watching.hidden = !(cfg && cfg.watchingClaudeCode);
}

/// Show/hide the settings view. With no `show` arg, toggles. Returns the new
/// visibility.
export function toggleSettings(doc, show) {
  const s = doc.getElementById("settings");
  if (!s) return false;
  const willShow = show === undefined ? s.hidden : show;
  s.hidden = !willShow;
  // Settings replaces the live view (the CSS hides every other #app-body child
  // while this class is set) so the two never stack into a giant scroll.
  const body = doc.getElementById("app-body");
  if (body) body.classList.toggle("settings-open", willShow);
  return willShow;
}

// --- activity journal ------------------------------------------------------
//
// A readable log of the flags this session raised — "at turn N, signal X fired:
// detail" — so the panel explains *why* it warned, not just the current color.

/// Render the journal (array from `GET /journal`) into the Activity section.
/// Hides the whole section when there's nothing flagged yet.
export function renderJournal(doc, items) {
  const section = doc.getElementById("activity");
  const list = doc.getElementById("activity-list");
  const rows = Array.isArray(items) ? items : [];
  if (section) section.hidden = rows.length === 0;
  if (!list) return;
  list.innerHTML = "";
  for (const it of rows) {
    const li = doc.createElement("li");
    li.className = "activity-row";
    const dot = doc.createElement("span");
    dot.className = "mini " + (stateInfo(it.state).cls);
    const body = doc.createElement("div");
    body.className = "activity-body";
    const head = doc.createElement("div");
    head.className = "activity-head";
    const name = doc.createElement("span");
    name.className = "activity-name";
    name.textContent = signalLabel(it.signal);
    head.appendChild(name);
    if (typeof it.turn === "number") {
      const turn = doc.createElement("span");
      turn.className = "activity-turn";
      turn.textContent = "turn " + (it.turn + 1);
      head.appendChild(turn);
    }
    const detail = doc.createElement("span");
    detail.className = "activity-detail";
    detail.textContent = it.detail || "";
    body.appendChild(head);
    body.appendChild(detail);
    li.appendChild(dot);
    li.appendChild(body);
    list.appendChild(li);
  }
}

export async function loadJournal(doc, fetchImpl) {
  const f = fetchImpl || fetch;
  try {
    const res = await f(apiBase() + "/journal", withAuth({ cache: "no-store" }));
    if (!res.ok) throw new Error("HTTP " + res.status);
    renderJournal(doc, await res.json());
  } catch (_e) {
    renderJournal(doc, []);
  }
}

// --- session history -------------------------------------------------------

/// Show/hide the history view (mutually exclusive with settings). Like
/// `toggleSettings`, it REPLACES the live view via a body class.
export function toggleHistory(doc, show) {
  const h = doc.getElementById("history");
  if (!h) return false;
  const willShow = show === undefined ? h.hidden : show;
  h.hidden = !willShow;
  const body = doc.getElementById("app-body");
  if (body) body.classList.toggle("history-open", willShow);
  if (willShow) toggleSettings(doc, false); // never stack the two views
  return willShow;
}

/// Compact "time ago" for a ms timestamp (0/absent → "").
export function timeAgo(ms, nowMs) {
  const t = Number(ms) || 0;
  if (!t) return "";
  const now = nowMs || Date.now();
  const s = Math.max(0, Math.round((now - t) / 1000));
  if (s < 60) return "just now";
  const m = Math.round(s / 60);
  if (m < 60) return m + "m ago";
  const h = Math.round(m / 60);
  if (h < 24) return h + "h ago";
  const d = Math.round(h / 24);
  return d + "d ago";
}

/// Render the history list from `GET /history` (array of items).
export function renderHistory(doc, items, nowMs) {
  const list = doc.getElementById("history-list");
  const empty = doc.getElementById("history-empty");
  const rows = Array.isArray(items) ? items : [];
  if (empty) empty.hidden = rows.length > 0;
  if (!list) return;
  list.innerHTML = "";
  for (const it of rows) {
    const li = doc.createElement("li");
    li.className = "history-row";
    const dot = doc.createElement("span");
    dot.className = "dot " + (stateInfo(it.state).cls);
    const body = doc.createElement("div");
    body.className = "history-body";
    const goal = doc.createElement("span");
    const hasGoal = it.goal && it.goal.trim();
    goal.className = "history-goal" + (hasGoal ? "" : " untitled");
    goal.textContent = hasGoal ? it.goal.trim() : "Untitled session";
    const meta = doc.createElement("span");
    meta.className = "history-meta";
    const bits = [it.model || "?"];
    if (it.turns) bits.push(it.turns + " turn" + (it.turns > 1 ? "s" : ""));
    const ago = timeAgo(it.lastActivity, nowMs);
    if (ago) bits.push(ago);
    meta.textContent = bits.join(" · ");
    body.appendChild(goal);
    body.appendChild(meta);
    li.appendChild(dot);
    li.appendChild(body);

    // Delete this one session.
    //
    // `POST /data/forget` has taken a session id since it existed, and nothing
    // sent one — the panel only offered delete-everything, so someone who wanted
    // a single embarrassing session gone had to erase all of it. Two clicks, like
    // the erase-all control, because there is no undo.
    if (it.sessionId) {
      const rm = doc.createElement("button");
      rm.type = "button";
      rm.className = "history-remove";
      rm.title = "Delete this session";
      rm.setAttribute("aria-label", "Delete this session");
      rm.innerHTML = icon("x", 14);
      rm.addEventListener("click", async (ev) => {
        ev.stopPropagation();
        if (rm.dataset.armed !== "1") {
          rm.dataset.armed = "1";
          rm.classList.add("armed");
          rm.title = "Click again to delete";
          rm.setAttribute("aria-label", "Click again to delete this session");
          window.setTimeout(() => {
            rm.dataset.armed = "";
            rm.classList.remove("armed");
            rm.title = "Delete this session";
            rm.setAttribute("aria-label", "Delete this session");
          }, 4000);
          return;
        }
        try {
          await fetch(apiBase() + "/data/forget", withAuth({
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify({ session: it.sessionId }),
          }));
          li.remove();
          if (empty && !list.children.length) empty.hidden = false;
        } catch (_e) {
          rm.title = "Couldn't delete";
        }
      });
      li.appendChild(rm);
    }

    list.appendChild(li);
  }
}

export async function loadHistory(doc, fetchImpl) {
  const f = fetchImpl || fetch;
  try {
    const res = await f(apiBase() + "/history", withAuth({ cache: "no-store" }));
    if (!res.ok) throw new Error("HTTP " + res.status);
    renderHistory(doc, await res.json());
  } catch (_e) {
    renderHistory(doc, []);
  }
}

export async function loadConfig(doc, fetchImpl) {
  const f = fetchImpl || fetch;
  try {
    const res = await f(apiBase() + "/config", withAuth({ cache: "no-store" }));
    if (!res.ok) throw new Error("HTTP " + res.status);
    renderConfig(doc, await res.json());
  } catch (_e) {
    renderConfig(doc, null);
  }
}

// --- judge (fuzzy signals) config ------------------------------------------
//
// Lets the user turn on the judge with their own OpenRouter key, no restart. The
// key is write-only here: it's POSTed to the local proxy (in-memory) and never
// stored in the browser or echoed back.

/// Render the judge status label from `GET /judge` ({ enabled, label }).
export function renderJudge(doc, data) {
  const status = doc.getElementById("judge-status");
  if (!status) return;
  if (data && data.enabled) status.textContent = "On · " + (data.label || "");
  else status.textContent = "Off";
}

export async function loadJudge(doc, fetchImpl) {
  const f = fetchImpl || fetch;
  try {
    const res = await f(apiBase() + "/judge", withAuth({ cache: "no-store" }));
    if (!res.ok) throw new Error("HTTP " + res.status);
    renderJudge(doc, await res.json());
  } catch (_e) {
    renderJudge(doc, null);
  }
}

export async function saveJudge(doc, fetchImpl) {
  const f = fetchImpl || fetch;
  const keyEl = doc.getElementById("judge-key");
  const modelEl = doc.getElementById("judge-model");
  const btn = doc.getElementById("judge-save");
  const apiKey = keyEl ? keyEl.value : "";
  const model = modelEl ? modelEl.value.trim() : "";
  if (btn) { btn.disabled = true; btn.textContent = "Saving…"; }
  try {
    const res = await f(apiBase() + "/judge", withAuth({
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ apiKey, model }),
    }));
    if (!res.ok) throw new Error("HTTP " + res.status);
    renderJudge(doc, await res.json());
    if (keyEl) keyEl.value = ""; // never keep the secret sitting in the DOM
    await loadConfig(doc); // refresh the Judge row
  } catch (_e) {
    renderJudge(doc, null);
  } finally {
    if (btn) { btn.disabled = false; btn.textContent = "Save judge"; }
  }
}

function setupJudge(doc) {
  const save = doc.getElementById("judge-save");
  if (save) save.addEventListener("click", () => saveJudge(doc));
}

// --- auto-intent (AI infers the intent) ------------------------------------

/// Render the Auto-intent switch from `GET /auto-intent` ({on, judgeReady}).
export function renderAutoIntent(doc, data) {
  const toggle = doc.getElementById("auto-intent-toggle");
  const label = doc.getElementById("auto-intent-label");
  const hint = doc.getElementById("auto-intent-hint");
  const on = !!(data && data.on);
  const ready = !!(data && data.judgeReady);
  if (toggle) { toggle.checked = on; toggle.disabled = !ready; }
  if (hint) hint.hidden = ready;
  if (label) label.textContent = !ready ? "Needs judge" : on ? "On" : "Off";
}

export async function loadAutoIntent(doc, fetchImpl) {
  const f = fetchImpl || fetch;
  try {
    const res = await f(apiBase() + "/auto-intent", withAuth({ cache: "no-store" }));
    if (!res.ok) throw new Error("HTTP " + res.status);
    renderAutoIntent(doc, await res.json());
  } catch (_e) {
    renderAutoIntent(doc, null);
  }
}

async function setAutoIntent(doc, on, fetchImpl) {
  const f = fetchImpl || fetch;
  try {
    const res = await f(apiBase() + "/auto-intent", withAuth({
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ on }),
    }));
    if (!res.ok) throw new Error("HTTP " + res.status);
    renderAutoIntent(doc, await res.json());
  } catch (_e) {
    await loadAutoIntent(doc);
  }
}

/// Resolve a goal-shift prompt: accept the pivot or keep the current goal.
export async function resolveIntentShift(doc, accept, fetchImpl) {
  const f = fetchImpl || fetch;
  const el = doc.getElementById("intent-shift");
  if (el) el.hidden = true; // optimistic — the next poll confirms
  try {
    await f(apiBase() + "/intent-shift", withAuth({
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ accept }),
    }));
    await loadIntent(doc, f);
  } catch (_e) { /* proxy unreachable — next poll re-surfaces if still pending */ }
}

function setupIntentShift(doc) {
  const acc = doc.getElementById("ishift-accept");
  if (acc) acc.addEventListener("click", () => resolveIntentShift(doc, true));
  const rej = doc.getElementById("ishift-reject");
  if (rej) rej.addEventListener("click", () => resolveIntentShift(doc, false));
  const toggle = doc.getElementById("auto-intent-toggle");
  if (toggle) toggle.addEventListener("change", () => setAutoIntent(doc, toggle.checked));
}

// --- preferences: Do Not Disturb -------------------------------------------

export function renderPrefs(doc, data) {
  const toggle = doc.getElementById("dnd-toggle");
  const label = doc.getElementById("dnd-label");
  const muted = !!(data && data.notificationsMuted);
  if (toggle) toggle.checked = muted;
  if (label) label.textContent = muted ? "On" : "Off";
}

export async function loadPrefs(doc, fetchImpl) {
  const f = fetchImpl || fetch;
  try {
    const res = await f(apiBase() + "/prefs", withAuth({ cache: "no-store" }));
    if (!res.ok) throw new Error("HTTP " + res.status);
    renderPrefs(doc, await res.json());
  } catch (_e) {
    renderPrefs(doc, null);
  }
}

async function setDnd(doc, muted, fetchImpl) {
  const f = fetchImpl || fetch;
  try {
    const res = await f(apiBase() + "/prefs", withAuth({
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ notificationsMuted: muted }),
    }));
    if (!res.ok) throw new Error("HTTP " + res.status);
    renderPrefs(doc, await res.json());
  } catch (_e) {
    await loadPrefs(doc);
  }
}

// --- updates (Tauri only): current version + manual check ------------------

/// Show the running version in the Updates row. In the Tauri shell this is the
/// native APP version (tauri.conf.json) via the app plugin — NOT #cfg-version,
/// which is the local proxy crate's version (0.0.1) and has nothing to do with
/// the installed app. Falls back to the proxy version in the browser dashboard.
async function syncUpdateVersion(doc) {
  const row = doc.getElementById("upd-version");
  if (!row) return;
  const T = typeof window !== "undefined" ? window.__TAURI__ : null;
  if (T && T.app && T.app.getVersion) {
    try { row.textContent = "v" + (await T.app.getVersion()); return; } catch (_e) { /* fall back */ }
  }
  const v = doc.getElementById("cfg-version");
  row.textContent = (v && v.textContent) || "—";
}

function setupUpdates(doc) {
  const T = typeof window !== "undefined" ? window.__TAURI__ : null;
  const btn = doc.getElementById("upd-check");
  const status = doc.getElementById("upd-check-status");
  syncUpdateVersion(doc);

  if (!btn) return;
  if (!T || !T.core) {
    // Browser dashboard: no in-app updater. Hide the manual check.
    btn.hidden = true;
    if (status) status.textContent = "";
    return;
  }
  btn.addEventListener("click", async () => {
    btn.disabled = true;
    if (status) status.textContent = "Checking…";
    try {
      const version = await T.core.invoke("check_update_now");
      if (status) status.textContent = version ? "Update " + version + " available" : "You're up to date";
    } catch (_e) {
      if (status) status.textContent = "Check failed";
    } finally {
      btn.disabled = false;
    }
  });
}

// --- session report export -------------------------------------------------

/// Build a shareable markdown report of the current session from the live
/// endpoints (intent + activity journal + status), copied to the clipboard.
export async function copySessionReport(doc, fetchImpl) {
  const f = fetchImpl || fetch;
  const btn = doc.getElementById("report-copy");
  const get = async (path) => {
    try { const r = await f(apiBase() + path, withAuth({ cache: "no-store" })); return r.ok ? await r.json() : null; }
    catch (_e) { return null; }
  };
  const [status, intent, journal] = await Promise.all([get("/status"), get("/intent"), get("/journal")]);
  const cur = status && status.current;
  const lines = ["# Drifterr session report", ""];
  if (cur) {
    lines.push(`- **State:** ${stateInfo(cur.state).label}`);
    lines.push(`- **Context:** ${clampPct(cur.saturationPct)}% ${cur.exact ? "(exact)" : "(estimated)"}`);
    lines.push(`- **Model:** ${cur.model || "?"}`);
    lines.push("");
  }
  if (intent && (intent.goal || (intent.constraints || []).length)) {
    lines.push("## Intent");
    if (intent.goal) lines.push(`**Goal:** ${intent.goal}`);
    for (const c of intent.constraints || []) {
      lines.push(`- [${c.checkable === "deterministic" ? "hard" : "soft"}] ${c.text}`);
    }
    lines.push("");
  }
  const flags = Array.isArray(journal) ? journal : [];
  if (flags.length) {
    lines.push("## Activity");
    for (const it of flags) {
      const t = typeof it.turn === "number" ? `turn ${it.turn + 1} · ` : "";
      lines.push(`- ${signalLabel(it.signal)} (${it.state}) — ${t}${it.detail || ""}`);
    }
    lines.push("");
  }
  const text = lines.join("\n");
  try {
    await navigator.clipboard.writeText(text);
    if (btn) { btn.textContent = "Copied!"; setTimeout(() => (btn.textContent = "Copy report"), 1500); }
  } catch (_e) {
    if (btn) { btn.textContent = "Copy failed"; setTimeout(() => (btn.textContent = "Copy report"), 1500); }
  }
  return text;
}

/// Report the current trigger as a false positive ("not a drift"). POSTs to
/// /feedback, which appends a local sample (never leaves the machine) that can
/// later seed the eval corpus. Gives quick inline confirmation on the button.
export async function reportNotDrift(doc, fetchImpl) {
  const f = fetchImpl || fetch;
  const btn = doc.getElementById("not-drift-btn");
  const restore = () => {
    if (btn) { btn.textContent = "Not a drift"; btn.disabled = false; }
  };
  try {
    const res = await f(apiBase() + "/feedback", withAuth({
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({}),
    }));
    if (btn) {
      if (res.ok) { btn.textContent = "Thanks — noted"; btn.disabled = true; }
      else { btn.textContent = "Couldn't save"; }
      setTimeout(restore, 2200);
    }
  } catch (_e) {
    if (btn) { btn.textContent = "Offline"; setTimeout(restore, 2200); }
  }
}

function setupExtras(doc) {
  const dnd = doc.getElementById("dnd-toggle");
  if (dnd) dnd.addEventListener("change", () => setDnd(doc, dnd.checked));
  setupUpdates(doc);
  const report = doc.getElementById("report-copy");
  if (report) report.addEventListener("click", () => copySessionReport(doc));
  setupWeekly(doc);
  setupSemanticModel(doc);
  setupPacks(doc);
  setupTeamShare(doc);
}

/// The rule-pack catalogue.
///
/// Two actions per pack, because they serve different jobs. **Apply** anchors *this*
/// session — useful now, gone tomorrow. **Copy for CLAUDE.md** writes the rules where
/// the agent already reads, which is the one that compounds: a rule the agent was told
/// is a rule it usually doesn't break, and detection is only the fallback for when it
/// does.
///
/// Advisory rules are labelled as such, always. A user who thinks a rule is enforced
/// when it isn't is worse off than one who knows it's a reminder.
export async function setupPacks(doc, fetchImpl) {
  const list = doc.getElementById("packs-list");
  if (!list) return;
  const f = fetchImpl || fetch;
  let packs = [];
  try {
    const res = await f(apiBase() + "/packs", withAuth({ cache: "no-store" }));
    if (!res.ok) return;
    packs = await res.json();
  } catch (_e) {
    return;
  }
  list.textContent = "";
  for (const p of packs) {
    const row = doc.createElement("div");
    row.className = "pack-row";
    const head = doc.createElement("div");
    head.className = "pack-head";
    const name = doc.createElement("span");
    name.className = "pack-name";
    name.textContent = p.name;
    const count = doc.createElement("span");
    count.className = "muted";
    // State enforceability up front rather than after a violation fails to appear.
    count.textContent =
      p.enforceable + " of " + (p.rules || []).length + " checked automatically";
    head.append(name, count);
    const rules = doc.createElement("ul");
    rules.className = "pack-rules";
    for (const text of p.rules || []) {
      const li = doc.createElement("li");
      li.textContent = text;
      rules.append(li);
    }
    const actions = doc.createElement("div");
    actions.className = "intent-actions";
    const apply = doc.createElement("button");
    apply.className = "mini-btn";
    apply.type = "button";
    apply.dataset.pack = p.id;
    apply.textContent = "Apply to this session";
    apply.addEventListener("click", async () => {
      apply.disabled = true;
      try {
        const res = await f(apiBase() + "/packs/apply", withAuth({
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ id: p.id }),
        }));
        apply.textContent = res.ok ? "Applied ✓" : "Couldn't apply";
      } catch (_e) {
        apply.textContent = "Drifterr not reachable";
      } finally {
        apply.disabled = false;
        setTimeout(() => (apply.textContent = "Apply to this session"), 1800);
      }
    });
    actions.append(apply);
    row.append(head, rules, actions);
    list.append(row);
  }
}

/// The team-share preview.
///
/// This exists so the local-first promise is checkable by the person relying on it, not
/// merely asserted in a privacy policy. The button shows the exact payload — nothing is
/// uploaded from here — alongside a plain sentence naming what the filter withheld and
/// why. A short list with no explanation would read as a bug; a long list with no
/// explanation would read as a leak.
export function setupTeamShare(doc, fetchImpl) {
  const btn = doc.getElementById("team-preview");
  const out = doc.getElementById("team-payload");
  const locked = doc.getElementById("team-locked");
  const withheld = doc.getElementById("team-withheld");
  if (!btn || !out) return;
  btn.addEventListener("click", async () => {
    if (!out.hidden) {
      out.hidden = true;
      if (withheld) withheld.hidden = true;
      return;
    }
    const f = fetchImpl || fetch;
    // Every built-in pack, so the preview shows the largest payload the user could
    // send rather than a flattering subset.
    const ids = Array.from(doc.querySelectorAll("#packs-list [data-pack]"))
      .map((el) => el.dataset.pack)
      .join(",");
    try {
      const res = await f(
        apiBase() + "/team/share-preview?days=14" + (ids ? "&packs=" + ids : ""), withAuth({ cache: "no-store" }));
      const data = await res.json();
      if (!data.entitled) {
        if (locked) locked.hidden = false;
        out.hidden = true;
        return;
      }
      if (locked) locked.hidden = true;
      out.hidden = false;
      out.textContent = JSON.stringify(data.payload, null, 2);
      if (withheld) {
        withheld.hidden = !data.withheld;
        withheld.textContent = data.withheld || "";
      }
    } catch (_e) {
      out.hidden = false;
      out.textContent = "Drifterr proxy not reachable.";
    }
  });
}

/// The optional semantic-model offer.
///
/// Only shown in the desktop shell (it needs a Tauri command to download), and only
/// when the build actually supports ONNX — offering a download that cannot be used
/// would be worse than saying nothing. Absent all that, the section stays hidden and
/// detection runs on the lexical embedder, which is a working default.
export async function setupSemanticModel(doc, invokeImpl) {
  const block = doc.getElementById("semantic-block");
  if (!block) return;
  const invoke =
    invokeImpl ||
    (typeof window !== "undefined" && window.__TAURI__?.core?.invoke) ||
    null;
  if (!invoke) return; // Browser dashboard: nothing to offer.

  const render = (st) => {
    if (!st || !st.supported) {
      block.hidden = true;
      return;
    }
    block.hidden = false;
    const get = doc.getElementById("semantic-get");
    const label = {
      bundled: "Bundled with this build",
      downloaded: "Installed",
      custom: "Custom path (DRIFTERR_EMBED_MODEL)",
      absent: "Not installed — using the lexical embedder",
    }[st.source] || "Unknown";
    setText(doc, "semantic-state", label);
    if (get) get.hidden = st.ready;
    // An unpinned build refuses to download rather than fetch an unverifiable binary
    // that an inference runtime would then execute. Say so instead of failing later.
    if (st.unpinned && !st.ready) {
      if (get) get.hidden = true;
      const msg = doc.getElementById("semantic-msg");
      if (msg) {
        msg.hidden = false;
        msg.textContent =
          "This build has no pinned model checksum, so the download is disabled. Point DRIFTERR_EMBED_MODEL at a model you obtained yourself.";
      }
    }
  };

  try {
    render(await invoke("semantic_model_status"));
  } catch (_e) {
    block.hidden = true;
    return;
  }

  const get = doc.getElementById("semantic-get");
  if (get) {
    get.addEventListener("click", async () => {
      const msg = doc.getElementById("semantic-msg");
      get.disabled = true;
      get.textContent = "Downloading…";
      try {
        await invoke("download_semantic_model");
        if (msg) { msg.hidden = false; msg.textContent = "Installed and verified."; }
        render(await invoke("semantic_model_status"));
      } catch (e) {
        // Includes a checksum mismatch, which must be visible rather than silent.
        if (msg) { msg.hidden = false; msg.textContent = String(e); }
      } finally {
        get.disabled = false;
        get.textContent = "Download (127 MB)";
      }
    });
  }
}

/// The weekly report view: fetch on open, toggle closed on a second click.
///
/// Fetched rather than cached so it is always current, and rendered as plain text
/// because the whole point is that the user can read, copy and keep it — it is their
/// data, generated on their machine.
function setupWeekly(doc, fetchImpl) {
  const btn = doc.getElementById("weekly-btn");
  const panel = doc.getElementById("weekly");
  if (!btn || !panel) return;
  btn.addEventListener("click", async () => {
    if (!panel.hidden) {
      panel.hidden = true;
      return;
    }
    // Close the sibling views so two full-width panels can't stack.
    for (const id of ["settings", "history"]) {
      const el = doc.getElementById(id);
      if (el) el.hidden = true;
    }
    setText(doc, "weekly-text", "Generating…");
    panel.hidden = false;
    const f = fetchImpl || fetch;
    try {
      const res = await f(apiBase() + "/report?days=7", withAuth({ cache: "no-store" }));
      if (!res.ok) {
        // 503 means there is no local database — say that plainly instead of
        // showing an empty report, which would read as "nothing drifted".
        setText(
          doc,
          "weekly-text",
          res.status === 503
            ? "No local database, so there's no history to report on yet. Sessions are only being held in memory."
            : "Couldn't generate the report (HTTP " + res.status + ")."
        );
        return;
      }
      const data = await res.json();
      setText(doc, "weekly-text", data.markdown || "");
    } catch (_e) {
      setText(doc, "weekly-text", "Drifterr proxy not reachable.");
    }
  });
  const copy = doc.getElementById("weekly-copy");
  if (copy) {
    copy.addEventListener("click", async () => {
      const text = doc.getElementById("weekly-text")?.textContent || "";
      try {
        await navigator.clipboard.writeText(text);
        copy.textContent = "Copied ✓";
        setTimeout(() => (copy.textContent = "Copy"), 1500);
      } catch (_e) { /* clipboard blocked — the text is on screen either way */ }
    });
  }
}

// --- provider selector -----------------------------------------------------

/// Render the provider pills from `GET /providers` ({ current, providers }) into
/// the given container (defaults to the settings selector).
export function renderProviders(doc, data, containerId = "provider-select") {
  const wrap = doc.getElementById(containerId);
  if (!wrap) return;
  wrap.innerHTML = "";
  const cur = data && data.current;
  for (const p of (data && data.providers) || []) {
    const b = doc.createElement("button");
    b.type = "button";
    b.className = "provider-pill" + (p.id === cur ? " active" : "");
    b.dataset.id = p.id;
    b.textContent = p.label;
    b.addEventListener("click", () => selectProvider(doc, p.id));
    wrap.appendChild(b);
  }
}

export async function loadProviders(doc, fetchImpl, containerId = "provider-select") {
  const f = fetchImpl || fetch;
  try {
    const res = await f(apiBase() + "/providers", withAuth({ cache: "no-store" }));
    if (!res.ok) throw new Error("HTTP " + res.status);
    renderProviders(doc, await res.json(), containerId);
  } catch (_e) {
    const wrap = doc.getElementById(containerId);
    if (wrap) wrap.innerHTML = "";
  }
}

/// Switch the upstream provider, remember it, and reflect it across every pill
/// group in the document (settings + onboarding share the same state).
async function selectProvider(doc, id) {
  try {
    const res = await fetch(apiBase() + "/provider", withAuth({
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id }),
    }));
    if (!res.ok) return;
    try { localStorage.setItem("drifterr_provider", id); } catch (_e) { /* private mode */ }
    for (const el of doc.querySelectorAll(".provider-pill")) {
      el.classList.toggle("active", el.dataset.id === id);
    }
    await loadConfig(doc); // refresh the Upstream row
  } catch (_e) { /* proxy unreachable */ }
}

/// Re-apply the saved provider on launch so the choice survives a restart
/// (the proxy itself defaults to the env/OpenRouter on boot).
export async function applySavedProvider() {
  let id = null;
  try { id = localStorage.getItem("drifterr_provider"); } catch (_e) { /* ignore */ }
  if (!id) return;
  try {
    await fetch(apiBase() + "/provider", withAuth({
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id }),
    }));
  } catch (_e) { /* best effort */ }
}

// --- re-anchor intervention ------------------------------------------------

/// The snapshot currently displayed, so "Copy" knows what to copy.
let currentReanchor = null;

/// Fetch the re-anchor snapshot/preamble and show it. Returns the payload (or
/// null on failure), which also makes it unit-testable.
export async function loadReanchor(doc, fetchImpl) {
  const f = fetchImpl || fetch;
  const section = doc.getElementById("reanchor");
  const textEl = doc.getElementById("reanchor-text");
  try {
    const res = await f(apiBase() + "/reanchor", withAuth({ cache: "no-store" }));
    if (!res.ok) throw new Error("HTTP " + res.status);
    const data = await res.json();
    currentReanchor = data;
    if (textEl) textEl.textContent = data.snapshot || "";
    if (section) section.hidden = false;
    return data;
  } catch (_e) {
    if (textEl) textEl.textContent = "Could not generate a re-anchor (is a session active?).";
    if (section) section.hidden = false;
    currentReanchor = null;
    return null;
  }
}

// --- intent (goal + constraints the user declares) -------------------------
//
// Drift is measured against the user's *stated* intent. This card shows the
// current goal + constraints and lets the user edit them. GET/POST /intent on
// the control API. Kept pure-ish so tests can drive it with a fake fetch.

/// The last intent payload we rendered, so "Edit" can prefill the form and we
/// know whether a save will attach to a live session or seed the next one.
let currentIntent = null;

/// Badge text for a constraint's enforcement strength.
///
/// "Proposed" outranks hard/soft, because it answers a different and more urgent
/// question: this rule was read out of your CLAUDE.md, not typed by you, and it
/// is only advisory until you say otherwise.
function checkableBadge(c) {
  if (c && c.proposed) return "Proposed";
  return c && c.checkable === "deterministic" ? "Hard" : "Soft";
}

/// Confirm a proposed (imported) constraint so it is enforced, then refresh.
export async function confirmConstraint(doc, id, fetchImpl) {
  const f = fetchImpl || fetch;
  try {
    const res = await f(apiBase() + "/intent/confirm", withAuth({
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ id }),
    }));
    if (res.ok) renderIntent(doc, await res.json());
  } catch (_e) {
    /* Proxy unreachable — the status poll reports it. */
  }
}

/// Render the intent payload (or null → empty state) into the view. Pure w.r.t.
/// the network. Always reveals the card so the user can set intent even with no
/// live session.
export function renderIntent(doc, data) {
  currentIntent = data;
  const section = doc.getElementById("intent");
  if (section) section.hidden = false;

  const goalEl = doc.getElementById("intent-goal");
  const list = doc.getElementById("intent-constraints");
  const empty = doc.getElementById("intent-empty");
  const hasGoal = !!(data && data.goal && data.goal.trim());
  const cs = (data && Array.isArray(data.constraints) ? data.constraints : []).filter(
    (c) => c && c.text
  );
  const has = hasGoal || cs.length > 0;

  if (empty) empty.hidden = has;
  if (goalEl) {
    goalEl.hidden = !has;
    goalEl.textContent = hasGoal ? data.goal.trim() : "(no goal set yet)";
    goalEl.classList.toggle("muted", !hasGoal);
  }
  if (list) {
    list.hidden = !has;
    list.innerHTML = "";
    for (const c of cs) {
      const li = doc.createElement("li");
      li.className = "intent-constraint";
      const badge = doc.createElement("span");
      badge.className =
        "intent-badge " +
        (c.proposed ? "proposed" : c.checkable === "deterministic" ? "hard" : "soft");
      badge.textContent = checkableBadge(c);
      if (c.proposed) {
        li.classList.add("proposed");
        badge.title =
          "Imported from your rules file. Advisory until you enforce it, so a " +
          "misread line can never raise a red alert.";
      }
      const text = doc.createElement("span");
      text.className = "intent-constraint-text";
      text.textContent = c.text;
      li.appendChild(badge);
      li.appendChild(text);
      // A proposal needs a way to say yes, or it is just a warning the user
      // learns to scroll past. Retire below is the way to say no.
      if (c.id && c.proposed) {
        const ok = doc.createElement("button");
        ok.type = "button";
        ok.className = "intent-confirm";
        ok.textContent = "Enforce";
        ok.title = "Treat this imported rule as one you set";
        ok.setAttribute("aria-label", "Enforce this proposed constraint");
        ok.addEventListener("click", () => confirmConstraint(doc, c.id));
        li.appendChild(ok);
      }
      // Retire (remove) a constraint the user no longer wants enforced.
      if (c.id) {
        const rm = doc.createElement("button");
        rm.type = "button";
        rm.className = "intent-remove";
        rm.title = "Remove this constraint";
        rm.setAttribute("aria-label", "Remove constraint");
        rm.innerHTML = icon("x", 14);
        rm.addEventListener("click", () => retireConstraint(doc, c.id));
        li.appendChild(rm);
      }
      list.appendChild(li);
    }
  }
}

export async function loadIntent(doc, fetchImpl) {
  const f = fetchImpl || fetch;
  try {
    const res = await f(apiBase() + "/intent", withAuth({ cache: "no-store" }));
    if (res.status === 404) {
      renderIntent(doc, null); // no session and nothing pending yet
      return null;
    }
    if (!res.ok) throw new Error("HTTP " + res.status);
    const data = await res.json();
    renderIntent(doc, data);
    return data;
  } catch (_e) {
    // Proxy unreachable — leave whatever's shown; the status poll surfaces the error.
    return null;
  }
}

/// Open the editor, prefilled from the current intent.
function openIntentEditor(doc) {
  const goalInput = doc.getElementById("intent-goal-input");
  const csInput = doc.getElementById("intent-constraints-input");
  if (goalInput) goalInput.value = currentIntent && currentIntent.goal ? currentIntent.goal : "";
  if (csInput) {
    const cs = currentIntent && Array.isArray(currentIntent.constraints) ? currentIntent.constraints : [];
    csInput.value = cs.map((c) => c.text).join("\n");
  }
  const pending = doc.getElementById("intent-pending");
  if (pending) pending.hidden = !(currentIntent && currentIntent.pending) && currentIntent !== null;
  const editBtn = doc.getElementById("intent-edit");
  if (editBtn) editBtn.hidden = true;
  doc.getElementById("intent-view").hidden = true;
  doc.getElementById("intent-editor").hidden = false;
}

function closeIntentEditor(doc) {
  doc.getElementById("intent-editor").hidden = true;
  doc.getElementById("intent-view").hidden = false;
  const editBtn = doc.getElementById("intent-edit");
  if (editBtn) editBtn.hidden = false;
}

export async function saveIntent(doc, fetchImpl) {
  const f = fetchImpl || fetch;
  const goal = (doc.getElementById("intent-goal-input")?.value || "").trim();
  const constraints = (doc.getElementById("intent-constraints-input")?.value || "")
    .split("\n")
    .map((s) => s.trim())
    .filter(Boolean);
  const btn = doc.getElementById("intent-save");
  if (btn) btn.disabled = true;
  try {
    const res = await f(apiBase() + "/intent", withAuth({
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ goal, constraints }),
    }));
    if (!res.ok) throw new Error("HTTP " + res.status);
    renderIntent(doc, await res.json());
    closeIntentEditor(doc);
  } catch (_e) {
    // Keep the editor open so the user doesn't lose their input.
  } finally {
    if (btn) btn.disabled = false;
  }
}

/// Retire (remove) a constraint by id, then refresh the intent card.
export async function retireConstraint(doc, id, fetchImpl) {
  const f = fetchImpl || fetch;
  try {
    const res = await f(apiBase() + "/intent/retire", withAuth({
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id }),
    }));
    if (!res.ok) throw new Error("HTTP " + res.status);
    renderIntent(doc, await res.json());
  } catch (_e) {
    await loadIntent(doc, f);
  }
}

function setupIntent(doc) {
  const edit = doc.getElementById("intent-edit");
  if (edit) edit.addEventListener("click", () => openIntentEditor(doc));
  const cancel = doc.getElementById("intent-cancel");
  if (cancel) cancel.addEventListener("click", () => closeIntentEditor(doc));
  const save = doc.getElementById("intent-save");
  if (save) save.addEventListener("click", () => saveIntent(doc));
}

/**
 * Diagnostics: copy, or read first.
 *
 * "Show" exists because a user should be able to see what they are about to paste
 * into a public issue. A support blob you cannot inspect is one people reasonably
 * refuse to send, and then the bug report is "it doesn't work" again.
 */
export function setupDiagnostics(doc, fetchImpl) {
  const f = fetchImpl || fetch;
  const copy = doc.getElementById("diag-copy");
  const show = doc.getElementById("diag-show");
  const out = doc.getElementById("diag-out");
  if (!copy && !show) return;

  const fetchText = async () => {
    const res = await f(apiBase() + "/diagnostics", withAuth({ cache: "no-store" }));
    if (!res.ok) throw new Error("HTTP " + res.status);
    return JSON.stringify(await res.json(), null, 2);
  };

  if (show) {
    show.addEventListener("click", async () => {
      const open = show.getAttribute("aria-expanded") === "true";
      if (open) {
        if (out) out.hidden = true;
        show.setAttribute("aria-expanded", "false");
        show.textContent = "Show";
        return;
      }
      try {
        if (out) {
          out.textContent = await fetchText();
          out.hidden = false;
        }
        show.setAttribute("aria-expanded", "true");
        show.textContent = "Hide";
      } catch (_e) {
        if (out) {
          out.textContent = "Couldn't read diagnostics — is Drifterr running?";
          out.hidden = false;
        }
      }
    });
  }

  if (copy) {
    copy.addEventListener("click", async () => {
      try {
        const text = await fetchText();
        await navigator.clipboard.writeText(text);
        copy.textContent = "Copied";
      } catch (_e) {
        // No clipboard, or the proxy is down. Show it instead of claiming success.
        try {
          if (out) {
            out.textContent = await fetchText();
            out.hidden = false;
          }
          copy.textContent = "Select it below";
        } catch (_e2) {
          copy.textContent = "Couldn't read it";
        }
      }
      window.setTimeout(() => {
        copy.textContent = "Copy diagnostics";
      }, 1800);
    });
  }
}

/**
 * The "Your data" controls: retention window, and delete-everything.
 *
 * Delete is two-step rather than a `confirm()` dialog: the first click arms the
 * button and names the count, the second does it. A dialog would be easier to
 * dismiss without reading, and this is the one control in the panel with no undo.
 */
export function setupDataControls(doc, fetchImpl) {
  const f = fetchImpl || fetch;
  const select = doc.getElementById("retention-select");
  const del = doc.getElementById("forget-all");
  const status = doc.getElementById("forget-status");
  if (!select && !del) return;

  let stored = 0;
  let armed = false;

  const paintCount = () => {
    if (!status || armed) return;
    status.textContent = stored
      ? `${stored} session${stored === 1 ? "" : "s"} stored`
      : "Nothing stored";
  };
  const disarm = () => {
    armed = false;
    if (del) {
      del.textContent = "Delete all history";
      del.classList.remove("armed");
    }
    paintCount();
  };

  const load = async () => {
    try {
      const res = await f(apiBase() + "/prefs", withAuth({ cache: "no-store" }));
      if (!res.ok) return;
      const p = await res.json();
      stored = Number(p.storedSessions) || 0;
      if (select) select.value = p.retentionDays == null ? "" : String(p.retentionDays);
      paintCount();
    } catch (_e) {
      /* Proxy unreachable — the status poll already says so. */
    }
  };

  if (select) {
    select.addEventListener("change", async () => {
      const raw = select.value;
      try {
        const res = await f(apiBase() + "/prefs", withAuth({
          method: "POST",
          headers: { "content-type": "application/json" },
          // `null` means forever. Sent explicitly, because omitting the field means
          // "leave it alone" and would make choosing Forever a silent no-op.
          body: JSON.stringify({
            notificationsMuted: doc.getElementById("dnd-toggle")?.checked || false,
            retentionDays: raw === "" ? null : Number(raw),
          }),
        }));
        if (res.ok) {
          const p = await res.json();
          stored = Number(p.storedSessions) || 0;
          disarm();
        }
      } catch (_e) {
        if (status) status.textContent = "Couldn't save that";
      }
    });
  }

  if (del) {
    del.addEventListener("click", async () => {
      if (!armed) {
        armed = true;
        del.classList.add("armed");
        del.textContent = "Click again to delete";
        if (status) {
          status.textContent = stored
            ? `${stored} session${stored === 1 ? "" : "s"} will be erased. This cannot be undone.`
            : "Nothing to delete.";
        }
        window.setTimeout(disarm, 6000);
        return;
      }
      try {
        const res = await f(apiBase() + "/data/forget", withAuth({
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ all: true }),
        }));
        const out = res.ok ? await res.json() : null;
        stored = out ? Number(out.storedSessions) || 0 : stored;
        armed = false;
        del.classList.remove("armed");
        del.textContent = "Delete all history";
        if (status) {
          status.textContent = out
            ? `Deleted ${out.deleted} session${out.deleted === 1 ? "" : "s"}.`
            : "Couldn't delete";
        }
      } catch (_e) {
        disarm();
        if (status) status.textContent = "Couldn't delete";
      }
    });
  }

  load();
}

/**
 * Show the extension pairing token, masked until asked for.
 *
 * The token is a local capability, but it is still a credential: leaving it in
 * plain sight in a panel that sits open all day - and so in every screenshot and
 * screen share of it - is the kind of small carelessness that undoes the work of
 * requiring it at all. Revealing is one click, and nothing needs it at a glance.
 */
export function setupPairingToken(doc) {
  const out = doc.getElementById("pair-token");
  const reveal = doc.getElementById("pair-reveal");
  const copy = doc.getElementById("pair-copy");
  if (!out) return;

  const MASK = "\u2022".repeat(16);
  const paint = (shown) => {
    const t = apiToken();
    out.textContent = t ? (shown ? t : MASK) : "Not available - restart Drifterr";
    if (reveal) {
      reveal.textContent = shown ? "Hide" : "Reveal";
      reveal.setAttribute("aria-expanded", shown ? "true" : "false");
      reveal.disabled = !t;
    }
    if (copy) copy.disabled = !t;
  };
  paint(false);

  if (reveal) {
    reveal.addEventListener("click", () => {
      paint(reveal.getAttribute("aria-expanded") !== "true");
    });
  }
  if (copy) {
    copy.addEventListener("click", async () => {
      const t = apiToken();
      if (!t) return;
      try {
        await navigator.clipboard.writeText(t);
        copy.textContent = "Copied";
      } catch (_e) {
        // No clipboard permission - reveal it so the user can select it by hand,
        // rather than being offered "Copy" and handed nothing.
        paint(true);
        copy.textContent = "Select it";
      }
      window.setTimeout(() => {
        copy.textContent = "Copy";
      }, 1600);
    });
  }
}

/// Is the intent editor currently open? (So the poll loop doesn't clobber edits.)
function intentEditorOpen(doc) {
  const ed = doc.getElementById("intent-editor");
  return !!(ed && !ed.hidden);
}

function setupUi(doc) {
  const gear = doc.getElementById("gear");
  if (gear) {
    gear.addEventListener("click", async () => {
      const showing = toggleSettings(doc);
      if (showing) { toggleHistory(doc, false); await loadConfig(doc); await loadProviders(doc); await loadAutoReanchor(doc); await loadJudge(doc); await loadAutoIntent(doc); await loadPrefs(doc); syncUpdateVersion(doc); }
    });
  }

  const histBtn = doc.getElementById("history-btn");
  if (histBtn) {
    histBtn.addEventListener("click", async () => {
      const showing = toggleHistory(doc);
      if (showing) await loadHistory(doc);
    });
  }

  setupIntent(doc);
  setupJudge(doc);
  setupIntentShift(doc);
  setupExtras(doc);

  const reanchorBtn = doc.getElementById("reanchor-btn");
  if (reanchorBtn) {
    reanchorBtn.addEventListener("click", () => loadReanchor(doc));
  }

  const notDriftBtn = doc.getElementById("not-drift-btn");
  if (notDriftBtn) {
    notDriftBtn.addEventListener("click", () => reportNotDrift(doc));
  }

  const closeBtn = doc.getElementById("reanchor-close");
  if (closeBtn) {
    closeBtn.addEventListener("click", () => {
      const s = doc.getElementById("reanchor");
      if (s) s.hidden = true;
    });
  }

  wireCopyButton(doc, "reanchor-copy", "Copy snapshot", () =>
    currentReanchor ? currentReanchor.snapshot : ""
  );
  wireCopyButton(doc, "reanchor-copy-preamble", "Copy preamble", () =>
    currentReanchor ? currentReanchor.preamble : ""
  );

  const autoToggle = doc.getElementById("auto-reanchor-toggle");
  if (autoToggle) {
    autoToggle.addEventListener("change", () => setAutoReanchor(doc, autoToggle.checked));
  }
}

/// Wire a copy-to-clipboard button that copies whatever `getText()` returns,
/// with a transient "Copied!" label. Shared by the snapshot + preamble buttons.
function wireCopyButton(doc, id, label, getText) {
  const btn = doc.getElementById(id);
  if (!btn) return;
  btn.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(getText() || "");
      btn.textContent = "Copied!";
    } catch (_e) {
      btn.textContent = "Copy failed";
    }
    setTimeout(() => (btn.textContent = label), 1500);
  });
}

// --- auto re-anchor toggle (settings) --------------------------------------

/// Render the auto-re-anchor switch from `GET /auto-reanchor`
/// ({ on, allowed, effective }). Shows the Pro hint when the plan doesn't allow.
export function renderAutoReanchor(doc, data) {
  const toggle = doc.getElementById("auto-reanchor-toggle");
  const label = doc.getElementById("auto-reanchor-label");
  const hint = doc.getElementById("auto-reanchor-hint");
  const on = !!(data && data.on);
  const allowed = !!(data && data.allowed);
  if (toggle) { toggle.checked = on; toggle.disabled = !allowed; }
  if (hint) hint.hidden = allowed;
  if (label) label.textContent = !allowed ? "Pro" : on ? "On" : "Off";
}

export async function loadAutoReanchor(doc, fetchImpl) {
  const f = fetchImpl || fetch;
  try {
    const res = await f(apiBase() + "/auto-reanchor", withAuth({ cache: "no-store" }));
    if (!res.ok) throw new Error("HTTP " + res.status);
    renderAutoReanchor(doc, await res.json());
  } catch (_e) {
    renderAutoReanchor(doc, null);
  }
}

async function setAutoReanchor(doc, on, fetchImpl) {
  const f = fetchImpl || fetch;
  try {
    const res = await f(apiBase() + "/auto-reanchor", withAuth({
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ on }),
    }));
    if (!res.ok) throw new Error("HTTP " + res.status);
    renderAutoReanchor(doc, await res.json());
  } catch (_e) {
    await loadAutoReanchor(doc); // resync to the real state on failure
  }
}

// --- accounts: optional sign-in + plan -------------------------------------
//
// auth.js (and the Supabase client it pulls from a CDN) is imported lazily so
// it can never block the core panel.
//
// **Signing in is optional.** Detection, constraints, the intent baseline and
// re-anchor are all local and must work with no account and no network — an
// account exists only to attach a paid plan to the install. So the panel body is
// never hidden behind auth: the sign-in sheet is opt-in (Settings → Account →
// Sign in) and dismissible. Gating a local-first tool behind a server account
// would contradict the product's own premise, and it costs activation for zero
// functional gain.

let _auth = null;
/// The resolved auth module, once (and if) the lazy import lands. Held separately
/// from the in-flight promise so synchronous code can use it opportunistically
/// without awaiting a CDN fetch that may be slow or never complete.
let _authReady = null;
function authMod() {
  return (_auth ??= import("./auth.js")
    .then((m) => (_authReady = m))
    .catch(() => null));
}

let gateMode = "signin"; // "signin" | "signup"

function applyGateMode(doc) {
  const signup = gateMode === "signup";
  setText(doc, "gate-title", signup ? "Create your account" : "Sign in to Drifterr");
  setText(doc, "gate-sub", signup
    ? "Starts a 14-day Pro trial. Detection keeps running locally either way."
    : "Only needed to buy or restore a paid plan. Drift detection runs locally either way.");
  setText(doc, "gate-submit", signup ? "Create account" : "Sign in");
  setText(doc, "gate-alt-text", signup ? "Already have an account?" : "New here?");
  setText(doc, "gate-toggle", signup ? "Sign in" : "Create an account");
  const nameField = doc.getElementById("gate-name-field");
  if (nameField) nameField.hidden = !signup;
}

function gateMessage(doc, text, kind) {
  const msg = doc.getElementById("gate-msg");
  if (!msg) return;
  msg.hidden = !text;
  msg.className = "gate-msg " + (kind || "");
  msg.textContent = text || "";
}

/// Open / close the opt-in sign-in sheet. The panel body stays mounted
/// underneath either way — the sheet is an overlay, never a gate.
export function openGate(doc) {
  const gate = doc.getElementById("gate");
  if (gate) gate.hidden = false;
}

export function closeGate(doc) {
  const gate = doc.getElementById("gate");
  if (gate) gate.hidden = true;
  gateMessage(doc, "", "");
}

async function submitGate(doc, mod) {
  const email = (doc.getElementById("gate-email").value || "").trim();
  const password = doc.getElementById("gate-password").value || "";
  const name = (doc.getElementById("gate-name")?.value || "").trim();
  const btn = doc.getElementById("gate-submit");
  if (!email || !password) return gateMessage(doc, "Enter your email and password.", "error");
  btn.disabled = true;
  try {
    if (gateMode === "signup") {
      const data = await mod.signUp(email, password, name);
      if (data.session) { closeGate(doc); await refreshAuth(doc, mod); }
      else gateMessage(doc, "Check your email to confirm, then sign in.", "ok");
    } else {
      await mod.signIn(email, password);
      closeGate(doc);
      await refreshAuth(doc, mod);
    }
  } catch (err) {
    gateMessage(doc, err.message || "Something went wrong.", "error");
  } finally {
    btn.disabled = false;
  }
}

function wireGate(doc, mod) {
  const toggle = doc.getElementById("gate-toggle");
  if (toggle) toggle.addEventListener("click", (e) => {
    e.preventDefault();
    gateMode = gateMode === "signin" ? "signup" : "signin";
    gateMessage(doc, "", "");
    applyGateMode(doc);
  });
  const submit = doc.getElementById("gate-submit");
  if (submit) submit.addEventListener("click", () => submitGate(doc, mod));
  for (const id of ["gate-close", "gate-dismiss"]) {
    const el = doc.getElementById(id);
    if (el) el.addEventListener("click", (e) => { e.preventDefault(); closeGate(doc); });
  }
  applyGateMode(doc);
}

/// Reflect the current auth state in the Account block and the plan pill.
///
/// Never hides the panel body: signed out is a fully supported, fully functional
/// state. Signed out simply means the Free entitlement, which the proxy already
/// defaults to.
export async function refreshAuth(doc, mod) {
  const user = await mod.currentUser();
  const block = doc.getElementById("account-block");
  if (block) block.hidden = false;
  const anon = doc.getElementById("acct-anon");
  const signed = doc.getElementById("acct-user");
  if (anon) anon.hidden = !!user;
  if (signed) signed.hidden = !user;

  if (!user) {
    // Local-only mode: make sure the proxy is on the Free entitlement and the
    // panel is honest about it. No account, no network required.
    await pushPlan("free", null);
    renderPlan(doc, "Free", "free");
    return;
  }
  await loadEntitlement(doc, mod, user);
}

/// Tell the local proxy which plan to enforce (identity only — no chat content).
/// The proxy derives the capability flags from the plan id itself.
async function pushPlan(planId, planToken) {
  try {
    await fetch(apiBase() + "/entitlement", withAuth({
      method: "POST",
      headers: { "Content-Type": "application/json" },
      // The signed assertion is what a release build actually accepts; `plan` is
      // the development fallback for a build with no entitlement key. Sending
      // both means one panel works against either.
      body: JSON.stringify({ plan: planId, planToken }),
    }));
  } catch (_e) { /* proxy not reachable yet — the status poll still reflects Free */ }
}

/// Show the *account's* plan in the Account block.
///
/// Deliberately does not touch the plan pill or the upgrade nudge: those follow
/// the proxy's effective entitlement (see `renderEffectivePlan`), which is what
/// actually gates capability and which knows about the local trial. Two writers
/// on one pill would just fight over it every poll.
function renderPlan(doc, planName, _planId) {
  setText(doc, "acct-plan", planName);
}

async function loadEntitlement(doc, mod, user) {
  setText(doc, "acct-email", user.email || "");

  let planName = "Free", planId = "free", planToken = null;
  try {
    const me = await mod.fetchMe();
    const ent = me.entitlement || {};
    planName = ent.plan_name || "Free";
    planId = ent.plan_id || "free";
    // The signed assertion of that plan. A release build of the proxy requires it
    // and ignores `plan_id`; only a build with no entitlement key falls back.
    planToken = typeof me.planToken === "string" ? me.planToken : null;
  } catch (_e) { /* keep Free defaults */ }

  await pushPlan(planId, planToken);
  renderPlan(doc, planName, planId);

  const isFree = planId === "free";
  const planBtn = doc.getElementById("acct-plan-btn");
  if (planBtn) {
    planBtn.textContent = isFree ? "Choose plan" : "Manage billing";
    planBtn.onclick = () => mod.openExternal(mod.SITE_URL + (isFree ? "/#pricing" : "/account"));
  }
}

/// Open a URL in the user's browser. Mirrors `auth.js`'s helper but works without
/// it, so the pricing links stay live even when accounts aren't configured.
function openSite(mod, path) {
  const base = (mod && mod.SITE_URL) || "https://drifterr.app";
  if (mod && mod.openExternal) return void mod.openExternal(base + path);
  try {
    window.open(base + path, "_blank", "noopener");
  } catch (_e) { /* no browser to open (headless) — nothing to do */ }
}

export async function initAccounts(doc) {
  // Wire the pricing links FIRST, synchronously, before awaiting anything.
  //
  // Two reasons this cannot wait for the auth module: the upgrade nudge is driven
  // by the proxy's entitlement (see `renderEffectivePlan`), which knows nothing
  // about whether accounts are configured; and `authMod()` imports the Supabase
  // client from a CDN, so on a slow or offline launch the await can outlast the
  // moment the user reaches for the button. Either way the result was a visible
  // button that did nothing.
  //
  // `openSite(null, …)` falls back to the canonical site URL, which is what
  // `auth.js` would have supplied anyway.
  for (const id of ["upgrade-btn", "acct-plans", "trial-upgrade-btn"]) {
    const el = doc.getElementById(id);
    if (el) el.addEventListener("click", () => openSite(_authReady, "/#pricing"));
  }

  const mod = await authMod();
  if (!mod || !mod.configured) {
    // Accounts are off, or the auth module couldn't load (e.g. offline / CDN
    // hiccup). The panel is already fully usable; just make sure the opt-in sheet
    // stays shut and the Account block doesn't advertise a sign-in that cannot
    // work.
    closeGate(doc);
    return;
  }
  wireGate(doc, mod);

  const block = doc.getElementById("account-block");
  if (block) block.hidden = false;
  const signin = doc.getElementById("acct-signin");
  if (signin) signin.addEventListener("click", () => openGate(doc));
  const signout = doc.getElementById("acct-signout");
  if (signout) signout.addEventListener("click", async () => { await mod.signOut(); await refreshAuth(doc, mod); });

  if (mod.supabase) mod.supabase.auth.onAuthStateChange(() => refreshAuth(doc, mod));
  await refreshAuth(doc, mod);
}

// --- first-run onboarding --------------------------------------------------
//
// A stepped, animated tour shown once on first launch: welcome → pick provider
// → connect your tool → ready. Dismissal is remembered in localStorage.

const ONB_STEPS = 5;
let onbStep = 0;

function updateOnboarding(doc) {
  const track = doc.getElementById("onb-track");
  if (track) track.style.transform = `translateX(-${onbStep * 100}%)`;
  const dots = doc.getElementById("onb-dots");
  if (dots) {
    dots.innerHTML = "";
    for (let i = 0; i < ONB_STEPS; i++) {
      const d = doc.createElement("span");
      d.className = "onb-dot" + (i === onbStep ? " on" : "");
      dots.appendChild(d);
    }
  }
  const back = doc.getElementById("onb-back");
  if (back) back.style.visibility = onbStep === 0 ? "hidden" : "visible";
  const next = doc.getElementById("onb-next");
  if (next) next.textContent = onbStep === ONB_STEPS - 1 ? "Get started" : "Next";
}

export function finishOnboarding(doc) {
  try { localStorage.setItem("drifterr_onboarded", "1"); } catch (_e) { /* private mode */ }
  // Persist the intent the user declared during onboarding (seeds their next
  // session). Best-effort — a proxy hiccup never blocks finishing the tour.
  const goal = (doc.getElementById("onb-goal-input")?.value || "").trim();
  const constraints = (doc.getElementById("onb-constraints-input")?.value || "")
    .split("\n").map((s) => s.trim()).filter(Boolean);
  if (goal || constraints.length) {
    fetch(apiBase() + "/intent", withAuth({
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ goal, constraints }),
    })).then(() => loadIntent(doc)).catch(() => { /* proxy not up yet */ });
  }
  const o = doc.getElementById("onboarding");
  if (o) o.hidden = true;
}

function setupOnboarding(doc) {
  const next = doc.getElementById("onb-next");
  if (next) next.addEventListener("click", () => {
    if (onbStep >= ONB_STEPS - 1) finishOnboarding(doc);
    else { onbStep++; updateOnboarding(doc); }
  });
  const back = doc.getElementById("onb-back");
  if (back) back.addEventListener("click", () => { if (onbStep > 0) { onbStep--; updateOnboarding(doc); } });
  const skip = doc.getElementById("onb-skip");
  if (skip) skip.addEventListener("click", () => finishOnboarding(doc));

  for (const el of doc.querySelectorAll("#onboarding .onb-copy")) {
    el.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(el.dataset.copy || "");
        const b = el.querySelector(".onb-copy-btn");
        if (b) { const t = b.textContent; b.textContent = "Copied!"; setTimeout(() => (b.textContent = t), 1200); }
      } catch (_e) { /* clipboard blocked */ }
    });
  }
}

/// Show the tour on first launch (unless already dismissed).
export function maybeOnboard(doc) {
  let done = null;
  try { done = localStorage.getItem("drifterr_onboarded"); } catch (_e) { /* ignore */ }
  if (done) return;
  const o = doc.getElementById("onboarding");
  if (!o) return;
  o.hidden = false;
  onbStep = 0;
  updateOnboarding(doc);
  loadProviders(doc, undefined, "onb-provider-select");
}

// --- launch splash ---------------------------------------------------------
// Branded intro shown on load and replayed each time the tray opens the panel
// (the native shell emits `window://opened`). Self-removing and fail-safe, so it
// can never trap the panel behind the overlay. Skipped under reduced motion.
function playSplash(el) {
  try {
    [el._d, el._h, el._s1, el._s2].forEach((t) => t && clearTimeout(t));
    el.hidden = false;
    el.classList.remove("sp-done", "sp-play");
    void el.offsetWidth; // restart the CSS animations
    el.classList.add("sp-play");
    const st = el.querySelector(".sp-status");
    if (st) {
      st.textContent = "initializing…";
      el._s1 = setTimeout(() => (st.textContent = "calibrating signals…"), 600);
      el._s2 = setTimeout(() => (st.textContent = "ready"), 1200);
    }
    el._d = setTimeout(() => el.classList.add("sp-done"), 1650);
    el._h = setTimeout(() => { el.hidden = true; }, 2200);
  } catch (_e) {
    el.hidden = true;
  }
}

export function setupSplash(doc) {
  const el = doc.getElementById("splash");
  if (!el) return;
  const reduce =
    typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;
  if (reduce) { el.hidden = true; return; }
  // Play once on cold launch only. Reopening from the tray must RESUME where the
  // user left off (same view/scroll/settings) — the window is hidden, not
  // reloaded — so replaying the splash on every open would wrongly feel like a
  // restart. Hence no "window://opened" replay.
  playSplash(el);
}

// --- auto-update (Tauri app only) ------------------------------------------
//
// The native shell checks for updates on launch and emits `update://available`
// with the new version. We show an in-app banner; clicking Update invokes the
// `install_update` command, which downloads + installs + relaunches — no manual
// reinstall. In the browser dashboard `window.__TAURI__` is absent, so this is
// a no-op. All best-effort: a failure just leaves the app running as-is.

export function setupUpdater(doc) {
  const T = typeof window !== "undefined" ? window.__TAURI__ : null;
  if (!T || !T.event || !T.core) return; // not the native app

  const banner = doc.getElementById("update-banner");
  const sub = doc.getElementById("upd-sub");
  const title = doc.getElementById("upd-title");
  const btn = doc.getElementById("upd-install");
  const bar = doc.getElementById("upd-bar");
  const fill = doc.getElementById("upd-bar-fill");
  if (!banner || !btn) return;

  T.event.listen("update://available", (e) => {
    if (sub) sub.textContent = e.payload ? `Version ${e.payload}` : "A new version is ready";
    banner.hidden = false;
  });
  T.event.listen("update://none", () => { banner.hidden = true; });
  T.event.listen("update://progress", (e) => {
    if (bar) bar.hidden = false;
    const pct = Math.max(0, Math.min(100, Math.round(Number(e.payload) || 0)));
    if (fill) fill.style.width = pct + "%";
    if (title) title.textContent = pct >= 100 ? "Installing…" : "Downloading update…";
  });

  btn.addEventListener("click", async () => {
    btn.disabled = true;
    btn.textContent = "Updating…";
    if (title) title.textContent = "Downloading update…";
    if (bar) bar.hidden = false;
    try {
      // Resolves only if there was nothing to install; on success the app
      // relaunches into the new version and this page goes away.
      await T.core.invoke("install_update");
    } catch (_e) {
      if (title) title.textContent = "Update failed";
      if (sub) sub.textContent = "Please try again later.";
      btn.disabled = false;
      btn.textContent = "Retry";
      if (bar) bar.hidden = true;
    }
  });
}

// --- polling loop (browser only) -------------------------------------------

export async function poll(doc, fetchImpl) {
  const f = fetchImpl || fetch;
  try {
    const res = await f(apiBase() + "/status", withAuth({ cache: "no-store" }));
    if (!res.ok) throw new Error("HTTP " + res.status);
    render(doc, await res.json());
    // Keep the intent card fresh (goal/constraints can change mid-session), but
    // never overwrite the form while the user is editing it.
    if (!intentEditorOpen(doc)) await loadIntent(doc, f);
    await loadJournal(doc, f);
    // Cheap localhost read; keeps the "Watching Claude Code" chip and the live
    // config (provider/version) fresh on the main panel without opening settings.
    await loadConfig(doc, f);
  } catch (_e) {
    renderError(doc, "Drifterr proxy not reachable (is it running on " + apiBase() + "?)");
  }
}

if (typeof document !== "undefined" && typeof window !== "undefined" && !window.__DRIFTERR_NO_AUTOSTART) {
  // Under the Tauri shell the window is transparent + borderless, so only the
  // rounded panel should paint — the desktop shows through everywhere else. In
  // the browser dashboard the same CSS needs an opaque dark canvas (otherwise the
  // white page shows behind the panel). A single class flips between the two.
  if (window.__TAURI__ || window.__TAURI_INTERNALS__) {
    document.documentElement.classList.add("tauri");
  }
  // The panel is always mounted and fully usable. `initAccounts` only fills in
  // the Account block and the plan pill; it can never hide the body, so a slow,
  // failed or entirely absent auth load just leaves the app in local-only mode.
  setupSplash(document);
  setupUi(document);
  setupOnboarding(document);
  setupUpdater(document);
  applySavedProvider();
  // Pair first, then poll. Everything after this line talks to an authenticated
  // control API, so the token has to be in hand before the first request rather
  // than one render later.
  ensureToken().then(() => {
    setupPairingToken(document);
    setupDataControls(document);
    setupDiagnostics(document);
    initAccounts(document);
    maybeOnboard(document);
    poll(document);
    window.setInterval(() => poll(document), 1500);
  });
}
