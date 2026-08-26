// Toolbar popup: shows the current session state from the local Drifterr proxy.
// Extension pages can reach http://localhost directly (host_permissions grant it
// and localhost is a secure context), so no background hop is needed here.

const LABELS = {
  green: { state: "Aligned", sub: "On track with your intent." },
  amber: { state: "Watch", sub: "Starting to drift — keep an eye on it." },
  red: { state: "Drifting", sub: "Off track. Consider re-anchoring." },
};
const SIGNALS = {
  constraint: "Constraints",
  saturation: "Saturation",
  goal_alignment: "Goal alignment",
  decision_coherence: "Decision coherence",
  degradation: "Degradation",
};

function set(id, text) {
  const el = document.getElementById(id);
  if (el) el.textContent = text;
}

async function refresh() {
  try {
    const res = await drifterrFetch("/status", { cache: "no-store" });
    if (!res.ok) throw new Error("http " + res.status);
    const data = await res.json();
    const cur = data && data.current;
    const dot = document.getElementById("dot");
    const off = document.getElementById("off");
    const detail = document.getElementById("detail");

    if (!cur) {
      dot.className = "dot";
      set("state", "No active session");
      set("sub", "Point your tool at the proxy and start chatting.");
      if (detail) detail.hidden = true;
      if (off) off.hidden = true;
      return;
    }
    const info = LABELS[cur.state] || { state: "Unknown", sub: "" };
    dot.className = "dot " + cur.state;
    set("state", info.state);
    set("sub", info.sub);
    if (detail) {
      if (cur.triggering && cur.triggering.detail) {
        detail.hidden = false;
        detail.textContent =
          (SIGNALS[cur.triggering.signal] || cur.triggering.signal) + " — " + cur.triggering.detail;
      } else {
        detail.hidden = true;
      }
    }
    if (off) off.hidden = true;
  } catch (e) {
    document.getElementById("dot").className = "dot";
    const detail = document.getElementById("detail");
    if (detail) detail.hidden = true;
    const off = document.getElementById("off");
    const pair = document.getElementById("pair");
    // Two different failures that used to look identical. "Not paired" is the
    // user's to fix in ten seconds; "not running" is not. Saying the wrong one
    // sends them to the wrong place.
    if (drifterrIsUnpaired(e)) {
      set("state", "Not paired yet");
      set("sub", "");
      if (off) off.hidden = true;
      if (pair) pair.hidden = false;
      return;
    }
    set("state", "App not running");
    set("sub", "");
    if (off) off.hidden = false;
    if (pair) pair.hidden = true;
  }
}

/// Wire the pairing form: paste the token from the panel, save, retry.
function setupPairing() {
  const form = document.getElementById("pair");
  const input = document.getElementById("pair-token");
  if (!form || !input) return;
  form.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    await drifterrSetToken(input.value);
    input.value = "";
    const pair = document.getElementById("pair");
    if (pair) pair.hidden = true;
    set("state", "Connecting…");
    refresh();
  });
}

/// Explain the scraper's health on this page.
///
/// The failure this surfaces is the dangerous one: a site changes its DOM, every
/// selector stops matching, and Drifterr reports "no drift" indefinitely. That looks
/// identical to working correctly, so the user never learns they aren't protected —
/// they just conclude the tool doesn't do much. A blind scraper has to say so.
///
/// Exported for tests; also called on popup open.
function renderHealth(el, health) {
  if (!el) return;
  if (!health || !health.reason || health.reason === "ok") {
    el.hidden = true;
    return;
  }
  const MESSAGES = {
    unpaired: "Drifterr isn't paired with this browser yet — paste the token above.",
    unsupported_host: "Drifterr doesn't watch this site.",
    not_a_chat_page: "No chat on this page yet.",
    no_conversation_yet: "Chat is open — send a message and Drifterr will start watching.",
    selectors_stale:
      "Drifterr can't read this page. " +
      (health.host || "This site") +
      " has likely changed its layout, so drift is NOT being tracked here. Please report it.",
  };
  const msg = MESSAGES[health.reason];
  if (!msg) {
    el.hidden = true;
    return;
  }
  el.hidden = false;
  el.className = "health" + (health.reason === "selectors_stale" ? " broken" : "");
  el.textContent = msg;
}

async function refreshHealth() {
  const el = document.getElementById("health");
  try {
    const got = await chrome.storage.local.get("drifterrHealth");
    renderHealth(el, got && got.drifterrHealth);
  } catch (_e) {
    if (el) el.hidden = true;
  }
}

if (typeof module !== "undefined") module.exports = { renderHealth };

setupPairing();
refresh();
refreshHealth();
