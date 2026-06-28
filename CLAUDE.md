# CLAUDE.md — working agreements for this repo

Guidance for any AI assistant (and humans) working in Drifterr.

## What this is

Drifterr is a **local-first** copilot that detects when an AI chat session
drifts from the user's stated intent (goal + constraints) and warns before the
wall, with one-click re-anchoring. See `docs/BRIEF.md` for the full vision and
`README.md` for current status.

## Workflow (non-negotiable)

- **One PR per milestone.** Each milestone (or transverse work batch) goes on its
  own branch and is merged via a pull request into `main`.
- **Never push directly to `main`.** No exceptions.
- Branch naming: `claude/<short-topic>` (e.g. `claude/m3-intervention`).
- Keep mechanical reformatting in its own commit, separate from feature work.

## Build & test

```bash
cargo test --workspace          # Rust: engine, store, proxy, fixtures, e2e
cargo clippy --workspace --all-targets   # must be warning-free
cargo fmt --all -- --check      # formatting is enforced in CI
cargo run -p drifterr-store --example demo   # detection end-to-end (console)

cd apps/desktop/ui && npm install && npm test   # headless UI (Playwright)
```

CI (`.github/workflows/ci.yml`) runs all of the above on every push/PR.

## Architecture rules (don't break these)

- **The engine is channel-agnostic.** It only ever sees the normalized
  `Conversation`. Adding a source = writing an adapter that emits that shape —
  never special-casing a channel inside the engine.
- **Signals stay separate, each with evidence.** Never fuse them into one opaque
  score. Every signal carries its own state + evidence (turn index, constraint
  id, offending span) so the UI can *name* the cause.
- **Hard vs soft.** Only hard signals (constraints, saturation) may drive RED.
  Soft signals (goal, decisions, degradation) are support-only and cap at AMBER
  unless several converge. A hard signal that cries wolf is worse than one that
  stays quiet — prefer under-claiming to false positives.
- **Never lie about precision.** `ContextState.exact` is true only via the proxy.
- **The proxy must never break a user's request.** Streaming is byte-for-byte
  passthrough via tee; detection runs off the response path; parsing is
  best-effort and never panics.
- **Local-first.** Conversations live in local SQLite; nothing goes to a
  Drifterr server. Model calls (judge) go through the user's own provider.

## Model provider

All of Drifterr's own AI calls (the judge, richer baseline extraction) go
through **OpenRouter** (OpenAI-compatible). The proxy defaults its OpenAI
upstream to OpenRouter; see `.env.example`.

## Layout

```
crates/engine      channel-agnostic detection core
crates/store       local SQLite persistence
crates/proxy       local API proxy channel + control API + dashboard
crates/tokenizer   token estimation + context-window map
apps/desktop/ui    menubar panel (no build step) + Playwright tests
apps/desktop/src-tauri  native tray shell (excluded from workspace)
fixtures/          annotated transcripts (engine validation set)
```
