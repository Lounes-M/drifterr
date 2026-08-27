# CLAUDE.md — working agreements for this repo

Guidance for any AI assistant (and humans) working in Drifterr.

## What this is

Drifterr is a **local-first** copilot that detects when an AI chat session
drifts from the user's stated intent (goal + constraints) and warns before the
wall, with one-click re-anchoring. See `README.md` for the product overview and
`docs/ACCOUNTS.md` for the accounts/billing architecture.

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
- **The control API is authenticated.** It serves conversation-derived data on
  localhost, and localhost is not a boundary against the user's own browser. Every
  route but `/health` and the dashboard's assets requires the per-install token, and
  only first-party origins may read a response. A new endpoint inherits this; a new
  *public* one needs a reason in `crates/proxy/src/auth.rs`.
- **A hard signal must rest on something the user asked for.** A rule inferred from
  a document they did not address to us is `proposed`: checked, named, and capped at
  AMBER until confirmed. Deterministic checking is not the same as knowing the rule
  was wanted.
- **Local-first.** Conversations live in local SQLite; **no chat content ever
  leaves the machine.** Model calls (judge) go through the user's own provider.
  The one server-side component is **accounts & billing** (Supabase + Stripe,
  see `supabase/` and `docs/ACCOUNTS.md`): it holds identity (email, plan,
  subscription status) and nothing else. Conversations, prompts, signals and
  drift scores are never sent there. When adding to the backend, keep that line
  bright — if it touches chat content, it does not belong in Supabase.

## Model provider

All of Drifterr's own AI calls (the judge, richer baseline extraction) go
through **OpenRouter** (OpenAI-compatible). The proxy defaults its OpenAI
upstream to OpenRouter; see `.env.example`.

## Layout

```
crates/engine      channel-agnostic detection core
crates/embeddings  local text embeddings (pluggable) for the soft signals
crates/judge       pluggable fail-safe judge (OpenRouter) + decision coherence
crates/intervention re-anchor snapshot + preamble
crates/store       local SQLite persistence
crates/proxy       local API proxy channel + control API + dashboard
crates/tokenizer   token estimation + context-window map
crates/adapters    channel adapters (Claude Code file watcher)
apps/desktop/ui    menubar panel (no build step) + Playwright tests
apps/desktop/src-tauri  native tray shell (excluded from workspace)
apps/extension     browser channel (MV3) — DOM scraper → /ingest
fixtures/          annotated transcripts (engine validation set)
```
