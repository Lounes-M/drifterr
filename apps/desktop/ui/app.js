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
/// origin is not http(s), so we fall back to the default localhost port. A
/// `window.DRIFTERR_API` override wins over both.
export function apiBase() {
  if (typeof window !== "undefined" && window.DRIFTERR_API) return window.DRIFTERR_API;
  if (typeof location !== "undefined" && location.origin && location.origin.startsWith("http")) {
    return location.origin;
  }
  return "http://127.0.0.1:8788";
}

/// Render a full status payload into the document. Pure w.r.t. the network:
/// give it a document and data, it mutates the DOM.
export function render(doc, data) {
  hide(doc, "error");
  const cur = data && data.current;

  if (!cur) {
    showEmpty(doc, true);
    setText(doc, "state-label", "No session");
    setText(doc, "blurb", "Waiting for activity.");
    setClass(doc, "dot", "dot unknown");
    setText(doc, "meta", "");
    return;
  }
  showEmpty(doc, false);

  const info = stateInfo(cur.state);
  setClass(doc, "dot", "dot " + info.cls);
  setText(doc, "state-label", info.label);
  setText(doc, "blurb", info.blurb);

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
  return willShow;
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

function setupUi(doc) {
  const gear = doc.getElementById("gear");
  if (gear) {
    gear.addEventListener("click", async () => {
      const showing = toggleSettings(doc);
      if (showing) await loadConfig(doc);
    });
  }

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

  const copyBtn = doc.getElementById("reanchor-copy");
  if (copyBtn) {
    copyBtn.addEventListener("click", async () => {
      const text = currentReanchor ? currentReanchor.snapshot : "";
      try {
        await navigator.clipboard.writeText(text);
        copyBtn.textContent = "Copied!";
      } catch (_e) {
        copyBtn.textContent = "Copy failed";
      }
      setTimeout(() => (copyBtn.textContent = "Copy"), 1500);
    });
  }
}

// --- polling loop (browser only) -------------------------------------------

export async function poll(doc, fetchImpl) {
  const f = fetchImpl || fetch;
  try {
    const res = await f(apiBase() + "/status", { cache: "no-store" });
    if (!res.ok) throw new Error("HTTP " + res.status);
    render(doc, await res.json());
  } catch (_e) {
    renderError(doc, "Drifterr proxy not reachable (is it running on " + apiBase() + "?)");
  }
}

if (typeof document !== "undefined" && typeof window !== "undefined" && !window.__DRIFTERR_NO_AUTOSTART) {
  setupUi(document);
  poll(document);
  window.setInterval(() => poll(document), 1500);
}
