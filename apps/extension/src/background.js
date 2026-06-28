// Background service worker: receives scraped conversations from content scripts
// and POSTs them to the local Drifterr control API. Running here (not in the
// page) avoids the page's CSP; fetch to http://localhost is allowed because
// localhost is treated as a secure context.

const ENDPOINT = "http://localhost:8788/ingest";

chrome.runtime.onMessage.addListener((msg) => {
  if (!msg || msg.type !== "drifterr-ingest") return;
  fetch(ENDPOINT, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(msg.payload),
  }).catch(() => {
    // Proxy not running / unreachable — ignore silently.
  });
});
