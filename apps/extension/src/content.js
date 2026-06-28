// Content script: periodically scrape the conversation and forward it to the
// background service worker (which posts to the local Drifterr proxy). Runs in
// the page, so it cannot reach localhost directly under the page CSP — hence the
// hop through the background worker.

(function () {
  let lastSignature = "";

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

  setInterval(tick, 2500);
  tick();
})();
