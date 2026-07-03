# Drifterr browser extension (MV3)

The browser channel: a content script reads the visible conversation on
claude.ai / ChatGPT / Gemini from the **DOM** (structured text, never an image),
and a background service worker posts it to the local Drifterr proxy
(`POST http://localhost:8788/ingest`) — feeding the same engine as the proxy and
file channels. 100% local; nothing leaves your machine except your own provider
calls.

It also closes the **re-anchor loop** in the browser: when the local engine
reports the session is drifting, the background worker fetches the re-anchor
preamble and the content script shows a one-click **⚓ Re-anchor** pill that
injects it into the chat composer (`DrifterrParse.inject`). The toolbar popup
shows live session state.

```
src/parse.js       per-host DOM → normalized turns + composer inject (window.DrifterrParse)
src/content.js     scrape loop + in-page re-anchor pill
src/background.js  POST to the local proxy; fetch /reanchor on drift (off the page CSP)
src/popup.html/js  toolbar popup: live session state
manifest.json      MV3 manifest
icons/             toolbar/store icons (generated: scripts/gen_icons.py)
scripts/           gen_icons.py, package.sh (store zip)
tests/             headless parser + inject verification (Playwright)
```

Supported hosts: claude.ai, ChatGPT, Gemini, Copilot, Perplexity.

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

## Package for the store

```
apps/extension/scripts/package.sh   # → drifterr-extension-<version>.zip
```

Bundles only the runtime files (manifest, `src/*`, `icons/`) — no `node_modules`,
tests or scripts — and regenerates the icons first. Upload the zip to the
**Chrome Web Store** (chrome.google.com/webstore/devconsole) or **Firefox
Add-ons** (addons.mozilla.org/developers). Bump `version` in `manifest.json`
before each submission.

## Tuning selectors

The per-host selectors in `src/parse.js` are a **best-effort starting point** —
these sites change their DOM often. The Playwright test pins the *logic*
(ordering, role mapping, session id, model); verify and adjust the selectors
against the live pages. In a sandbox that pins a browser, set `CHROMIUM_PATH`.
