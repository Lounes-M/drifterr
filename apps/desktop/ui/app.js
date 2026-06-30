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
  // The panel starts visible and `initAccounts` swaps in the login gate only
  // once auth confirms there's no session. This way a slow/failed auth load (or
  // an offline launch) can never leave a blank panel — worst case the user just
  // sees the (data-free) panel until the gate resolves.
  setupUi(document);
  initAccounts(document);
  poll(document);
  window.setInterval(() => poll(document), 1500);
}
