# Store listing copy

Everything a Chrome Web Store / Firefox Add-ons submission asks for, prepared so the
only remaining step is having a developer account. Keep it in sync with the site — a
listing that overclaims is worse than no listing.

## Name

Drifterr — AI drift detection

## Summary (132 char limit)

> Warns you when an AI chat drifts from the goal and constraints you set. Runs locally
> — your conversations never leave the browser.

## Description

Long AI chats quietly slide away from what you asked for. The model reintroduces an
approach you rejected, ignores a rule you set, or fills its context window until quality
drops. It feels like the model got worse. It didn't — the conversation drifted.

Drifterr measures that drift against a ground truth you own: the goal and constraints
**you** stated. When something breaks, it names the cause and shows the offending line,
then offers a one-click re-anchor that restates your intent in the thread.

**Setup**

Drifterr runs as a small local app; the extension is the browser's window into it.
After installing both, open the Drifterr panel, copy the pairing token from
Settings → Browser extension, and paste it into this extension's popup. That is
the whole setup, and it happens once.

The pairing exists because the local app is authenticated — without it, any
website you had open could read your sessions out of it.

**What it checks**

- Rules you set, verified deterministically — no new dependencies, no `console.log`, no
  TypeScript `any`, protected files and directories, server-side-only work, length caps.
  A flag comes with the exact line that broke it.
- Context saturation — the leading indicator of quality dropping.
- Goal drift, decision incoherence and degradation, as advisory signals only.

**What it will not do**

- It will not cry wolf. Only hard, provable signals can raise a red alert; fuzzy ones
  cap at "watch". Ambiguous cases stay quiet, so expect Drifterr to say nothing most of
  the time.
- It will not send your conversations anywhere. Detection runs on your machine.

**Requires the Drifterr desktop app**, which does the analysis locally. This extension
only reads the visible conversation and hands it to the app over localhost.

## Category

Developer Tools

## Permission justifications

Reviewers reject vague answers here, so each is specific.

| Permission | Why it is needed |
|---|---|
| `storage` | Stores whether the content script can currently read the page, so the popup can warn the user when a site redesign has broken extraction. No conversation content and no identifiers are stored. |
| `host_permissions: http://localhost:8788/*`, `http://127.0.0.1:8788/*` | The only network destination. The locally-installed Drifterr app listens here; it performs the analysis. Nothing is sent to any remote server. |
| `content_scripts` on claude.ai, chatgpt.com, chat.openai.com, gemini.google.com, copilot.microsoft.com, perplexity.ai | Reads the visible conversation text on the supported assistants, which is the data being analysed, and injects the re-anchor text into the composer when the user clicks the re-anchor button. Limited to exactly these hosts. |

No `tabs`, no `<all_urls>`, no remote code, no analytics SDK.

## Privacy answers

- **Does it collect personally identifiable information?** No.
- **Does it collect health, financial or authentication information?** No.
- **Does it collect personal communications?** The conversation text is read from the
  page and sent to the user's **own machine** (`localhost:8788`) for analysis. It is not
  transmitted to the developer or to any third party, and is not stored by the
  extension.
- **Does it collect location, web history or user activity?** No.
- **Is user data sold or transferred for purposes unrelated to the item's core
  functionality?** No.
- **Is user data used to determine creditworthiness or for lending?** No.
- **Remote code:** none. All logic ships in the package.

Privacy policy URL: https://drifterr.app/privacy
Additional detail on the local-first boundary: https://drifterr.app/proof

## Single purpose statement

Detect when an AI chat conversation diverges from the goal and constraints the user
stated, and let the user restate them in one click.

## Screenshots to capture (1280×800)

1. A drifting session in the panel with a named constraint violation and the offending
   span visible.
2. The in-page re-anchor pill on a supported assistant.
3. The popup showing live session state.
4. The popup showing the "can't read this page" warning — worth including, because
   honesty about the failure mode is part of the pitch.

## Pre-submission checklist

- [ ] Bump `version` in `manifest.json`.
- [ ] `npm test` passes, including the selector-health checks.
- [ ] `scripts/package.sh` regenerates icons and produces the zip.
- [ ] Selectors verified against the live pages for every listed host.
- [ ] The site's "works today" row still labels the browser channel beta until the
      listing is actually live.
