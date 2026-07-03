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
    const res = await fetch("http://localhost:8788/status", { cache: "no-store" });
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
  } catch (_e) {
    document.getElementById("dot").className = "dot";
    set("state", "App not running");
    set("sub", "");
    const off = document.getElementById("off");
    if (off) off.hidden = false;
    const detail = document.getElementById("detail");
    if (detail) detail.hidden = true;
  }
}

refresh();
