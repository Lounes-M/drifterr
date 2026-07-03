// Content script: periodically scrape the conversation and forward it to the
// background service worker (which posts to the local Drifterr proxy). Runs in
// the page, so it cannot reach localhost directly under the page CSP — hence the
// hop through the background worker.
//
// It also renders a small in-page "re-anchor" pill when the background worker
// says the session is drifting, and injects the preamble into the composer on
// click — the browser channel's one-click re-anchor.

(function () {
  let lastSignature = "";
  let pendingPreamble = "";

  function tick() {
    try {
      const result = window.DrifterrParse && window.DrifterrParse.extract();
      if (!result || !result.turns.length) return;
      // Only send when the conversation actually changed.
      const signature = result.turns.map((t) => t.role[0] + t.content.length).join("|");
      if (signature === lastSignature) return;
      lastSignature = signature;
      chrome.runtime.sendMessage({ type: "drifterr-ingest", payload: result });
    } catch (_e) {
      // Never throw into the page.
    }
  }

  // --- in-page re-anchor pill ----------------------------------------------

  function pillEl() {
    let el = document.getElementById("drifterr-reanchor-pill");
    if (el) return el;
    el = document.createElement("button");
    el.id = "drifterr-reanchor-pill";
    el.type = "button";
    el.setAttribute(
      "style",
      [
        "position:fixed", "right:20px", "bottom:20px", "z-index:2147483647",
        "display:none", "align-items:center", "gap:8px",
        "padding:10px 14px", "border:none", "border-radius:999px",
        "background:linear-gradient(90deg,#ff7847,#ff3d81)", "color:#fff",
        "font:600 13px/1 -apple-system,Segoe UI,Roboto,sans-serif",
        "box-shadow:0 6px 20px rgba(255,61,129,0.4)", "cursor:pointer",
      ].join(";")
    );
    el.addEventListener("click", () => {
      const ok = window.DrifterrParse && window.DrifterrParse.inject(pendingPreamble);
      el.textContent = ok ? "✓ Re-anchored" : "Couldn’t find the message box";
      setTimeout(hidePill, ok ? 1400 : 2400);
    });
    document.body.appendChild(el);
    return el;
  }

  function showPill(detail) {
    const el = pillEl();
    el.textContent = "⚓ Drifting — Re-anchor";
    el.title = detail || "This session is drifting from your intent.";
    el.style.display = "inline-flex";
  }

  function hidePill() {
    const el = document.getElementById("drifterr-reanchor-pill");
    if (el) el.style.display = "none";
  }

  chrome.runtime.onMessage.addListener((msg) => {
    if (!msg) return;
    if (msg.type === "drifterr-reanchor") {
      pendingPreamble = msg.preamble || "";
      showPill(msg.detail);
    } else if (msg.type === "drifterr-clear") {
      hidePill();
    }
  });

  setInterval(tick, 2500);
  tick();
})();
