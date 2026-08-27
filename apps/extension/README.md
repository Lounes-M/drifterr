# Drifterr browser extension (MV3)

The browser channel: a content script reads the visible conversation on
claude.ai / ChatGPT / Gemini from the **DOM** (structured text, never an image),
and a background service worker posts it to the local Drifterr proxy
(`POST http://localhost:8788/ingest`) — feeding the same engine as the proxy and
file channels. 100% local; nothing leaves your machine except your own provider
calls.

## Pairing (required, once)

The local control API is authenticated, so that a website you have open cannot
read your sessions out of it. Everything Drifterr launches itself finds the token
on disk; the extension cannot, so it is paired by hand:

1. Open the Drifterr panel → **Settings → Browser extension**
2. **Copy** the pairing token
3. Click the extension's toolbar icon and paste it into **Connect**

Until then the popup says **Not paired yet** rather than pretending there is no
drift — a monitoring tool that reports nothing because it is misconfigured is
worse than one that says it is misconfigured.

The token is stored in `chrome.storage.local`, not `sync`: it belongs to this
machine's Drifterr install, and syncing it to your other browsers would pair them
against a token that is not theirs.

It also closes the **re-anchor loop** in the browser: when the local engine
reports the session is drifting, the background worker fetches the re-anchor
preamble and the content script shows a one-click **⚓ Re-anchor** pill that
injects it into the chat composer (`DrifterrParse.inject`). The toolbar popup
shows live session state.

```
src/api.js         base URL, pairing token, and the 401 path (shared by worker + popup)
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

## Publishing status — not on the stores yet

The zip is store-ready; **the listings are not created.** Publishing needs a paid
Chrome Web Store developer account, a privacy-policy URL and justification for each
permission, plus a review turnaround. None of that is a code change, so it is tracked
here rather than pretended away.

**Until a listing exists, this is a manual "Load unpacked" install**, which is why the
site labels the browser channel *beta* and does not put ChatGPT/Gemini/Perplexity logos
in its "works today" row. Do not re-add them there before the listings are live.

Ready for submission when someone has the account:

- `scripts/package.sh` produces the zip.
- [`STORE_LISTING.md`](STORE_LISTING.md) holds the description, the per-permission
  justifications reviewers ask for, and the privacy answers.

## Selector health — a broken scraper says so

These selectors read the DOM internals of sites nobody here controls, so a redesign
breaking them is the **expected steady state**, not an edge case.

That created the worst possible failure mode. `extract()` returned `null` both when a
page had no conversation and when every selector had gone stale, so a breakage looked
exactly like "no drift": Drifterr would report all-clear indefinitely and the user
would conclude detection was useless rather than blind.

`DrifterrParse.diagnose()` separates the cases:

| reason | meaning |
|---|---|
| `ok` | turns extracted; reports how many |
| `unsupported_host` | not a site we watch |
| `not_a_chat_page` | no composer — settings page, logged out, 404 |
| `no_conversation_yet` | composer present, page essentially empty — a fresh chat |
| `selectors_stale` | **composer present and the page is full of text we cannot read** |

The last one is the breakage. The content script records it to `chrome.storage`, warns
once in the console (so it lands in a bug report), and the popup shows a red banner
saying drift is *not* being tracked on that page. The text threshold is deliberately
generous: a false "we're broken" is nearly as damaging as silent blindness.

## Tuning selectors

The per-host selectors in `src/parse.js` are a **best-effort starting point** —
these sites change their DOM often. The Playwright test pins the *logic*
(ordering, role mapping, session id, model, and the health diagnosis); verify and
adjust the selectors against the live pages. In a sandbox that pins a browser, set
`CHROMIUM_PATH`.

When a user reports `selectors_stale`, the fix is a selector update in
`src/parse.js` — add a new variant rather than replacing the old one, so the fallback
chain keeps working for users on an older page build.
