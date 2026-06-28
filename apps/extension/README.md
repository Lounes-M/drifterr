# Drifterr browser extension (MV3)

The browser channel: a content script reads the visible conversation on
claude.ai / ChatGPT / Gemini from the **DOM** (structured text, never an image),
and a background service worker posts it to the local Drifterr proxy
(`POST http://localhost:8788/ingest`) — feeding the same engine as the proxy and
file channels. 100% local; nothing leaves your machine except your own provider
calls.

```
src/parse.js       per-host DOM → normalized turns (window.DrifterrParse)
src/content.js     scrape loop in the page → message the background worker
src/background.js  POST to the local proxy (off the page CSP)
manifest.json      MV3 manifest
tests/             headless parser verification (Playwright)
```

## Load it (dev)

1. Run the proxy: `cargo run -p drifterr-proxy` (control API on `:8788`).
2. Chrome → `chrome://extensions` → enable Developer mode → **Load unpacked** →
   select `apps/extension`.
3. Open claude.ai / ChatGPT / Gemini and chat. Watch the panel at
   `http://localhost:8788/`.

## Test

```
cd apps/extension
npm install
npx playwright install   # first time
npm test
```

## Tuning selectors

The per-host selectors in `src/parse.js` are a **best-effort starting point** —
these sites change their DOM often. The Playwright test pins the *logic*
(ordering, role mapping, session id, model); verify and adjust the selectors
against the live pages. In a sandbox that pins a browser, set `CHROMIUM_PATH`.
