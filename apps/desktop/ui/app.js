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

function signalRow(doc, s) {
  const li = doc.createElement("li");
  li.className = "signal-row";
  const dot = doc.createElement("span");
  dot.className = "mini " + (stateInfo(s.state).cls);
  const body = doc.createElement("div");
  body.className = "signal-body";
  const name = doc.createElement("span");
  name.className = "signal-name";
  name.textContent = signalLabel(s.signal);
  const detail = doc.createElement("span");
  detail.className = "signal-detail";
  detail.textContent = s.detail || "";
  body.appendChild(name);
  body.appendChild(detail);
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
    const res = await f(apiBase() + "/journal", { cache: "no-store" });
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
    list.appendChild(li);
  }
}

export async function loadHistory(doc, fetchImpl) {
  const f = fetchImpl || fetch;
  try {
    const res = await f(apiBase() + "/history", { cache: "no-store" });
    if (!res.ok) throw new Error("HTTP " + res.status);
    renderHistory(doc, await res.json());
  } catch (_e) {
    renderHistory(doc, []);
  }
}

export async function loadConfig(doc, fetchImpl) {
  const f = fetchImpl || fetch;
  try {
    const res = await f(apiBase() + "/config", { cache: "no-store" });
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
    const res = await f(apiBase() + "/judge", { cache: "no-store" });
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
    const res = await f(apiBase() + "/judge", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ apiKey, model }),
    });
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
    const res = await f(apiBase() + "/auto-intent", { cache: "no-store" });
    if (!res.ok) throw new Error("HTTP " + res.status);
    renderAutoIntent(doc, await res.json());
  } catch (_e) {
    renderAutoIntent(doc, null);
  }
}

async function setAutoIntent(doc, on, fetchImpl) {
  const f = fetchImpl || fetch;
  try {
    const res = await f(apiBase() + "/auto-intent", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ on }),
    });
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
    await f(apiBase() + "/intent-shift", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ accept }),
    });
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
    const res = await f(apiBase() + "/prefs", { cache: "no-store" });
    if (!res.ok) throw new Error("HTTP " + res.status);
    renderPrefs(doc, await res.json());
  } catch (_e) {
    renderPrefs(doc, null);
  }
}

async function setDnd(doc, muted, fetchImpl) {
  const f = fetchImpl || fetch;
  try {
    const res = await f(apiBase() + "/prefs", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ notificationsMuted: muted }),
    });
    if (!res.ok) throw new Error("HTTP " + res.status);
    renderPrefs(doc, await res.json());
  } catch (_e) {
    await loadPrefs(doc);
  }
}

// --- updates (Tauri only): current version + manual check ------------------

/// Copy the running version (loadConfig populates #cfg-version) into the Updates row.
function syncUpdateVersion(doc) {
  const row = doc.getElementById("upd-version");
  const v = doc.getElementById("cfg-version");
  if (row && v) row.textContent = v.textContent || "—";
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
    try { const r = await f(apiBase() + path, { cache: "no-store" }); return r.ok ? await r.json() : null; }
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

function setupExtras(doc) {
  const dnd = doc.getElementById("dnd-toggle");
  if (dnd) dnd.addEventListener("change", () => setDnd(doc, dnd.checked));
  setupUpdates(doc);
  const report = doc.getElementById("report-copy");
  if (report) report.addEventListener("click", () => copySessionReport(doc));
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
    const res = await f(apiBase() + "/providers", { cache: "no-store" });
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
    const res = await fetch(apiBase() + "/provider", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id }),
    });
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
    await fetch(apiBase() + "/provider", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id }),
    });
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
    const res = await f(apiBase() + "/reanchor", { cache: "no-store" });
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
function checkableBadge(c) {
  return c && c.checkable === "deterministic" ? "Hard" : "Soft";
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
      badge.className = "intent-badge " + (c.checkable === "deterministic" ? "hard" : "soft");
      badge.textContent = checkableBadge(c);
      const text = doc.createElement("span");
      text.className = "intent-constraint-text";
      text.textContent = c.text;
      li.appendChild(badge);
      li.appendChild(text);
      // Retire (remove) a constraint the user no longer wants enforced.
      if (c.id) {
        const rm = doc.createElement("button");
        rm.type = "button";
        rm.className = "intent-remove";
        rm.title = "Remove this constraint";
        rm.setAttribute("aria-label", "Remove constraint");
        rm.textContent = "×";
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
    const res = await f(apiBase() + "/intent", { cache: "no-store" });
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
    const res = await f(apiBase() + "/intent", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ goal, constraints }),
    });
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
    const res = await f(apiBase() + "/intent/retire", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id }),
    });
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
    const res = await f(apiBase() + "/auto-reanchor", { cache: "no-store" });
    if (!res.ok) throw new Error("HTTP " + res.status);
    renderAutoReanchor(doc, await res.json());
  } catch (_e) {
    renderAutoReanchor(doc, null);
  }
}

async function setAutoReanchor(doc, on, fetchImpl) {
  const f = fetchImpl || fetch;
  try {
    const res = await f(apiBase() + "/auto-reanchor", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ on }),
    });
    if (!res.ok) throw new Error("HTTP " + res.status);
    renderAutoReanchor(doc, await res.json());
  } catch (_e) {
    await loadAutoReanchor(doc); // resync to the real state on failure
  }
}

// --- accounts: login gate + plan ------------------------------------------
//
// auth.js (and the Supabase client it pulls from a CDN) is imported lazily so
// it can never block the core panel. When accounts aren't configured (the
// shipped default until config.js is filled), none of this runs and the panel
// behaves exactly as before.

let _auth = null;
function authMod() {
  return (_auth ??= import("./auth.js").catch(() => null));
}

let gateMode = "signin"; // "signin" | "signup"

function applyGateMode(doc) {
  const signup = gateMode === "signup";
  setText(doc, "gate-title", signup ? "Create your account" : "Sign in to Drifterr");
  setText(doc, "gate-sub", signup
    ? "Free to start. Pick a plan after you sign in."
    : "Connect your account to start tracking drift.");
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
      if (data.session) await refreshAuth(doc, mod);
      else gateMessage(doc, "Check your email to confirm, then sign in.", "ok");
    } else {
      await mod.signIn(email, password);
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
  applyGateMode(doc);
}

/// Show the gate (logged out) or the panel body (logged in).
export async function refreshAuth(doc, mod) {
  const user = await mod.currentUser();
  const gate = doc.getElementById("gate");
  const body = doc.getElementById("app-body");
  if (!user) {
    if (gate) gate.hidden = false;
    if (body) body.hidden = true;
    return;
  }
  if (gate) gate.hidden = true;
  if (body) body.hidden = false;
  await loadEntitlement(doc, mod, user);
}

async function loadEntitlement(doc, mod, user) {
  const block = doc.getElementById("account-block");
  if (block) block.hidden = false;
  setText(doc, "acct-email", user.email || "");

  let isFree = true, planName = "Free", planId = "free";
  try {
    const me = await mod.fetchMe();
    const ent = me.entitlement || {};
    planName = ent.plan_name || "Free";
    planId = ent.plan_id || "free";
    isFree = planId === "free";
  } catch (_e) { /* keep Free defaults */ }

  // Tell the local proxy which plan to enforce (identity only — no chat content).
  // The proxy derives the capability flags from the plan id itself.
  try {
    await fetch(apiBase() + "/entitlement", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ plan: planId }),
    });
  } catch (_e) { /* proxy not reachable yet — status poll will still reflect Free */ }

  setText(doc, "acct-plan", planName);
  const pill = doc.getElementById("plan-pill");
  if (pill) { pill.hidden = false; pill.textContent = planName; pill.classList.toggle("free", isFree); }
  const nudge = doc.getElementById("upgrade-nudge");
  if (nudge) nudge.hidden = !isFree;

  const planBtn = doc.getElementById("acct-plan-btn");
  if (planBtn) {
    planBtn.textContent = isFree ? "Choose plan" : "Manage billing";
    planBtn.onclick = () => mod.openExternal(mod.SITE_URL + (isFree ? "/#pricing" : "/account"));
  }
}

export async function initAccounts(doc) {
  const mod = await authMod();
  if (!mod || !mod.configured) {
    // Accounts are off, or the auth module couldn't load (e.g. offline / CDN
    // hiccup). Either way, never leave the panel blank: undo the pre-hide and
    // fall back to the accounts-free experience.
    const body = doc.getElementById("app-body");
    if (body) body.hidden = false;
    const gate = doc.getElementById("gate");
    if (gate) gate.hidden = true;
    return;
  }
  wireGate(doc, mod);

  const signout = doc.getElementById("acct-signout");
  if (signout) signout.addEventListener("click", async () => { await mod.signOut(); await refreshAuth(doc, mod); });
  const upgrade = doc.getElementById("upgrade-btn");
  if (upgrade) upgrade.addEventListener("click", () => mod.openExternal(mod.SITE_URL + "/#pricing"));

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
    fetch(apiBase() + "/intent", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ goal, constraints }),
    }).then(() => loadIntent(doc)).catch(() => { /* proxy not up yet */ });
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
  playSplash(el);
  // Replay each time the tray opens the panel.
  const T = typeof window !== "undefined" ? window.__TAURI__ : null;
  if (T && T.event) {
    T.event.listen("window://opened", () => playSplash(el));
  }
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
    const res = await f(apiBase() + "/status", { cache: "no-store" });
    if (!res.ok) throw new Error("HTTP " + res.status);
    render(doc, await res.json());
    // Keep the intent card fresh (goal/constraints can change mid-session), but
    // never overwrite the form while the user is editing it.
    if (!intentEditorOpen(doc)) await loadIntent(doc, f);
    await loadJournal(doc, f);
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
  // The panel starts visible and `initAccounts` swaps in the login gate only
  // once auth confirms there's no session. This way a slow/failed auth load (or
  // an offline launch) can never leave a blank panel — worst case the user just
  // sees the (data-free) panel until the gate resolves.
  setupSplash(document);
  setupUi(document);
  setupOnboarding(document);
  setupUpdater(document);
  applySavedProvider();
  initAccounts(document);
  maybeOnboard(document);
  poll(document);
  window.setInterval(() => poll(document), 1500);
}
