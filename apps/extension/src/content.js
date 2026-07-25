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

  /// Record why extraction produced nothing, so the popup can tell the user.
  ///
  /// These scrapers read the DOM internals of sites we don't control, so a redesign
  /// silently breaking them is the expected steady state, not an edge case. Without
  /// this the failure is invisible: Drifterr reports no drift forever and the user
  /// concludes detection is useless rather than blind. Stored rather than sent, since
  /// nothing here needs to leave the browser.
  function recordHealth(diag) {
    try {
      chrome.storage?.local?.set({
        drifterrHealth: {
          reason: diag.reason,
          host: diag.host,
          turns: diag.turns || 0,
          at: Date.now(),
        },
      });
      // Log the breakage once per page so it shows up in a bug report.
      if (diag.reason === "selectors_stale" && !window.__drifterrWarned) {
        window.__drifterrWarned = true;
        console.warn(
          "[Drifterr] Could not read this conversation — " +
            diag.host +
            " has likely changed its layout. Drift is NOT being tracked on this page."
        );
      }
    } catch (_e) {
      /* storage unavailable — the console warning above is still useful */
    }
  }

  function tick() {
    try {
      const parse = window.DrifterrParse;
      if (!parse) return;
      // Diagnose first: this is what distinguishes "nothing to read" from "we can no
      // longer read it", which `extract()` alone cannot express.
      const diag = parse.diagnose ? parse.diagnose() : null;
      if (diag) recordHealth(diag);

      const result = parse.extract();
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
