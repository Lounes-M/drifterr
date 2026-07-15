<div align="center">

<img src="assets/brand/mark-512.png" alt="Drifterr" width="96" height="96" />

# Drifterr

**Your model didn't change. Your conversation did.**

Drifterr is a local-first copilot that detects when an AI chat drifts from what
you originally asked — and warns you before you lose an hour, with one-click
re-anchoring.

[**Download**](https://drifterr.app/download) · [Website](https://drifterr.app) · [Pricing](https://drifterr.app/#pricing)

[![CI](https://github.com/Lounes-M/drifterr/actions/workflows/ci.yml/badge.svg)](https://github.com/Lounes-M/drifterr/actions/workflows/ci.yml)

</div>

---

## What it is

Long AI chat sessions quietly slide away from your intent — the model
reintroduces an approach you rejected, ignores a constraint you set, or fills its
context window until quality drops. It *feels* like the model got worse. It
didn't. **The conversation drifted.**

Drifterr measures that drift against a ground truth you own — the **goal and
constraints you set yourself** — names the cause, and lets you re-anchor in one
click. It runs as a quiet menubar app alongside the tools you already use.

## Why it's trustworthy

- **Local-first.** Conversations live in local SQLite. **No chat content ever
  leaves your machine.** The only server-side component is accounts & billing
  (identity + plan, nothing else). Not a promise — [enforced in CI](crates/proxy/tests/egress.rs)
  and laid out at [drifterr.app/proof](https://drifterr.app/proof).
- **Named causes, not a black box.** Each signal carries its own evidence (turn,
  constraint, offending span) so the UI can tell you *what* drifted — signals are
  never fused into one opaque score.
- **Never cries wolf.** Only hard signals (constraint violations, context
  saturation) can raise a red alert; soft signals stay advisory. Under-claiming
  beats false positives.
- **The proxy never breaks your request.** Streaming is byte-for-byte
  passthrough; detection runs off the response path and never panics.

## How it works

```
Channels (proxy │ Claude Code files │ browser extension)
        │  → one normalized Conversation + ContextState
        ▼
   Detection engine
   Baseline → 5 separate signals (each with evidence) → state machine
        ▼
   Menubar UI  +  one-click Re-anchor  +  cross-session standing orders
```

The engine is **channel-agnostic**: every channel emits the same normalized
shape, and the engine is written once against it. Adding a source means writing
an adapter — never touching the engine.

**The signals**

| Signal | Type | What it catches |
|---|---|---|
| Constraint adherence | hard | Latest reply breaks a rule you set (deterministic, can drive RED) |
| Context saturation | hard | Window filling up — the leading quality indicator (exact via the proxy) |
| Goal alignment | soft | Replies trending away from your goal (local embeddings) |
| Decision coherence | soft (judge) | A decision you explicitly rejected creeps back in |
| Degradation | soft | Looping, verbosity blow-ups, hedging spikes |

**Re-anchor.** When a session drifts, Drifterr generates a paste-ready reset
snapshot (goal + active constraints + held/rejected decisions) and a short
in-thread preamble — pure functions of your baseline, no LLM, no network.

**Standing orders.** Constraints you repeat across sessions can be promoted so
they auto-apply to every new session — your rules stick without restating them.

## Install

Download for macOS, Windows or Linux at **[drifterr.app/download](https://drifterr.app/download)**.
Free to use; Pro and Team plans add unlimited sessions, the hosted fail-safe
judge, the drift map and team features — see [Pricing](https://drifterr.app/#pricing).

**Using Claude Code? Nothing to set up.** Drifterr auto-watches your local
sessions (`~/.claude/projects`) — no keys, no env vars, fully local. Just declare
your intent in the panel and keep coding; it warns at the exact turn a reply
breaks one of your rules. (Override the watched dir with `DRIFTERR_WATCH_DIR`.)

For any other tool, run **`drifterr-proxy init`** — it detects your tool and
provider key and prints the exact config — or set it by hand:

```bash
export OPENAI_BASE_URL=http://localhost:8787/v1
export OPENAI_API_KEY=...                 # your own provider key
# Anthropic-style tools:
export ANTHROPIC_BASE_URL=http://localhost:8787
```

**Plug into any major provider.** The proxy defaults to OpenRouter, but you can
connect directly to OpenAI, Anthropic, Google Gemini, Groq, Mistral, DeepSeek,
xAI (Grok) or Together with a single setting — use your own key, nothing is sent
to a Drifterr server:

```bash
DRIFTERR_PROVIDER=openai     # or: anthropic | gemini | groq | mistral | …
# …or point at a fully custom endpoint:
# OPENAI_UPSTREAM=https://my-host/v1
```

## Repository layout

```
crates/engine        channel-agnostic detection core (baseline, signals, state machine)
crates/embeddings    local text embeddings for the soft signals
crates/judge         pluggable fail-safe judge (OpenRouter) + decision coherence
crates/intervention  re-anchor snapshot + preamble
crates/store         local SQLite persistence
crates/proxy         local API proxy channel + control API + dashboard
crates/tokenizer     token estimation + context-window map
crates/adapters      channel adapters (Claude Code file watcher)
apps/desktop         menubar app: no-build panel UI + Tauri 2 tray shell
apps/extension       browser channel (MV3) — DOM scraper → /ingest
apps/landing         marketing site + accounts/pricing/download (Vercel)
supabase             accounts & billing (Supabase + Stripe) — identity only
assets/brand         logo / icon source of truth
fixtures             annotated transcripts (engine validation set)
```

## Develop

Requires a recent stable Rust toolchain and Node 18+.

```bash
cargo test --workspace                        # engine, store, proxy, e2e
cargo clippy --workspace --all-targets        # warning-free
cargo fmt --all -- --check                    # formatting is enforced in CI
cargo run -p drifterr-proxy                    # proxy :8787, control + panel :8788
cargo run -p drifterr-store --example demo     # detection end-to-end (console)

cd apps/desktop/ui && npm install && npm test  # headless menubar UI (Playwright)
```

CI ([`.github/workflows/ci.yml`](.github/workflows/ci.yml)) runs all of the above
on every push and PR.

## Status

Honest state of each capability — no claim here overshoots what actually runs.

| Area | State | Notes |
|---|---|---|
| Proxy channel (relay + exact saturation) | ✅ shipped | Byte-for-byte SSE passthrough, detection off the response path |
| Claude Code channel (file watch) | ✅ shipped | **Zero-config** — auto-watches `~/.claude/projects`, no keys |
| Constraint signal (deterministic, hard) | ✅ shipped | EN/FR phrasings; code rules (no JS/TODO/console.log/`any`/eval/secrets, no new deps, protected files, word & line caps) |
| Saturation signal (hard) | ✅ shipped | Exact only via the proxy |
| Degradation signal (soft) | ✅ shipped | Looping, verbosity, hedging (EN/FR) |
| Goal-alignment signal (soft) | ✅ shipped | Local **ONNX semantic** model (bge-small, 384-dim, ~127MB) bundled by default since 0.2.5; lexical fallback. Goal↔drift separation 0.07→0.22; eval accuracy 67%→100% |
| Decision-coherence judge (soft) | ✅ shipped | Opt-in, BYOK (your OpenRouter key); fail-safe (degrades, never blocks) |
| Auto-intent (AI infers goal + constraints) | ✅ shipped | Opt-in, BYOK; continuous re-baseline |
| Re-anchor (snapshot + preamble) | ✅ shipped | Copy or auto-inject (proxy channel) |
| Standing orders (cross-session rules) | ✅ shipped | |
| Menubar app (tray + panel) | ✅ shipped | Auto-hide on blur, resumes state |
| Detection eval harness + release gate | ✅ shipped | Metrics + zero-hard-FP gate, tunable thresholds ([`eval/thresholds.conf`](eval/thresholds.conf), [`eval/SCHEMA.md`](eval/SCHEMA.md)) |
| Egress guarantee (CI-enforced) | ✅ shipped | [`crates/proxy/tests/egress.rs`](crates/proxy/tests/egress.rs) |
| Browser extension (MV3) | 🚧 partial | Built; not store-published |
| Signed / notarized desktop builds | 📋 planned | Ships **unsigned** today (Gatekeeper right-click → Open on macOS) |
| Validated on a large real corpus | 📋 planned | The irreplaceable next step — drop annotated sessions into `eval/` |

## More docs

- [`docs/ACCOUNTS.md`](docs/ACCOUNTS.md) — accounts & billing architecture (the local-first boundary)
- [`supabase/README.md`](supabase/README.md) — Supabase + Stripe setup
- [`RELEASING.md`](RELEASING.md) — cutting a desktop release + versioning policy
- [`SECURITY.md`](SECURITY.md) — reporting a vulnerability; the local-first boundary
- [`CONTRIBUTING.md`](CONTRIBUTING.md) — how to contribute; architecture non-negotiables
- [`eval/SCHEMA.md`](eval/SCHEMA.md) — detection annotation schema + train/blind split
- [`docs/FAQ.md`](docs/FAQ.md) — common questions (keys, providers, privacy, cost)
- [`docs/TROUBLESHOOTING.md`](docs/TROUBLESHOOTING.md) — proxy, Claude Code, install warnings
- [`packaging/`](packaging/) — Homebrew cask + winget manifest

## License

Source-available, **not** open-source. © 2026 Drifterr. All rights reserved.
See [`LICENSE`](LICENSE): you may read and privately evaluate the code, but using,
redistributing, or hosting it requires a written license. The app binaries at
[drifterr.app](https://drifterr.app) have their own end-user terms.
