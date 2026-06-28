# Drifterr

> Your model didn't change. Your conversation did. Drifterr warns you before the wall.

Drifterr is a **local-first copilot** that watches whether your in-progress AI
chat session is *drifting* from what you originally asked — and warns you before
you lose an hour, with one-click re-anchoring.

The key insight that makes it solid: this is not a fuzzy perception. It's a
**measurable** deviation from a **ground truth the user owns** — the goal and
constraints *they themselves set*. We never claim "the model got worse"
(perceptual, unverifiable). We detect "this session diverged from what you set"
(measurable, verifiable).

See [`docs/BRIEF.md`](docs/BRIEF.md) for the full strategic & technical brief.

---

## Where this repo is

This is the **foundation + hard-signal engine** — milestones **M0** and **M1**
from the build plan, the product's go/no-go. The detection core is implemented
and validated against hand-annotated fixtures, with **no real channel
attached** yet. If detection isn't convincing here, nothing downstream matters;
it is, so the channels (proxy, file watcher, browser) can be built on top.

| Milestone | Status |
|---|---|
| **M0** — monorepo, SQLite schema, normalized format, tokenizer | ✅ done |
| **M1** — engine: baseline, Signal 1 (constraints), Signal 4 (saturation), state machine | ✅ done |
| M2 — proxy channel + menubar UI | ⬜ next |
| M3 — soft signals (2,3,5) + intervention (re-anchor snapshot) | ⬜ |
| M4 — file watcher + browser extension channels | ⬜ |
| M5 — standing orders (the moat) + opt-in proxy auto-re-anchor | ⬜ |

## Architecture (the non-negotiable principle)

The engine is **channel-agnostic**. Every channel produces the same normalized
`Conversation`; the engine is written once and only ever sees that shape. Adding
a source = writing an adapter, never touching the engine. Everything is local.

```
Channels (proxy │ file │ browser)
        │  → normalized Conversation + ContextState
        ▼
   Detection engine
   Baseline → 5 signals (separate, with evidence) → state machine
        ▼
   UI  +  Intervention  +  Standing-orders learning
```

## Crates

| Crate | Responsibility |
|---|---|
| [`crates/engine`](crates/engine) | The channel-agnostic core: normalized format, baseline, signals, state machine. |
| [`crates/store`](crates/store) | Local SQLite persistence (load/persist a `Conversation`, baseline, signal events). |
| [`crates/tokenizer`](crates/tokenizer) | Provider-pluggable token estimation for the non-proxy channels. |
| `fixtures/` | Hand-annotated transcripts — the M1 validation set. |

Channels and the desktop app (`apps/desktop`, `apps/extension`, `crates/proxy`,
`crates/adapters`, `crates/intervention`) land in later milestones.

## The hard signals (what's implemented)

Signals are **never fused into one opaque score**. Each carries its own state
*and its own evidence* (turn index, constraint id, offending span), so the UI
can **name** the cause.

- **Signal 1 — constraint adherence.** Deterministic rules/regex on the latest
  assistant turn. 0 cost, 0 false positives. Allowed to drive RED on its own.
  Judge-checkable (fuzzy) constraints are deferred to a later milestone.
- **Signal 4 — context saturation.** Occupancy + per-turn fill rate + tool
  volume. Exact via the proxy, estimated elsewhere (we never lie about
  precision — see `ContextState::exact`). The leading indicator.

The **state machine** applies the brief's two rules: hard constraint violations
commit to RED immediately (they're facts), while threshold-based saturation
uses **hysteresis** (N consecutive confirmations up, N consecutive clears down)
to kill flicker.

A deliberate design choice worth flagging: a code-scoped rule (e.g. "no
comments") only fires inside fenced code blocks. If a message has no fences we
report *no violation* rather than guess that prose is code — a hard signal that
cries wolf is worse than one that stays quiet.

## Run it

```bash
cargo test                                   # all unit + fixture tests
cargo run -p drifterr-store --example demo   # end-to-end: detect → persist → reload
```

The demo builds a session that introduces an `auth.js` file against a
"TypeScript only" constraint and prints:

```
committed state: Red
triggered by Constraint: constraint c1 violated: "TypeScript only, no JS"
offending span: ".js"
```

## Acceptance, met

- **M0** — a `Conversation` loads from a JSON fixture and persists/reloads
  losslessly (`crates/store` round-trip tests).
- **M1** — on annotated transcripts the engine correctly flags deterministic
  constraint violations and saturation thresholds, with no real channel
  (`crates/engine/tests/fixtures.rs`).
