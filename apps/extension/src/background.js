// Background service worker: receives scraped conversations from content scripts
// and POSTs them to the local Drifterr control API. Running here (not in the
// page) avoids the page's CSP; fetch to http://localhost is allowed because
// localhost is treated as a secure context.
//
// It also closes the re-anchor loop for the browser channel: when the local
// engine reports the session is drifting (red), it fetches the re-anchor
// preamble and tells the content script to offer a one-click inject.

const BASE = "http://localhost:8788";

chrome.runtime.onMessage.addListener((msg, sender) => {
  if (!msg || msg.type !== "drifterr-ingest") return;
  const tabId = sender && sender.tab && sender.tab.id;
  fetch(BASE + "/ingest", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(msg.payload),
  })
    .then((r) => (r.ok ? r.json() : null))
    .then((status) => forwardReanchor(tabId, status))
    .catch(() => {
      // Proxy not running / unreachable — ignore silently.
    });
});

/// If the session is drifting, fetch the preamble and offer it to the page;
/// otherwise clear any standing offer. No-op without a tab to message.
function forwardReanchor(tabId, status) {
  if (tabId == null) return;
  const cur = status && status.current;
  if (!cur || cur.state !== "red") {
    chrome.tabs.sendMessage(tabId, { type: "drifterr-clear" }).catch(() => {});
    return;
  }
  const detail = cur.triggering ? cur.triggering.detail || "" : "";
  fetch(BASE + "/reanchor", { cache: "no-store" })
    .then((r) => (r.ok ? r.json() : null))
    .then((re) => {
      if (!re || !re.preamble) return;
      chrome.tabs
        .sendMessage(tabId, { type: "drifterr-reanchor", preamble: re.preamble, detail })
        .catch(() => {});
    })
    .catch(() => {});
}
