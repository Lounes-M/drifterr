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
  leaves your machine**, and the app sends nothing on its own — no telemetry, no
  analytics, no heartbeat. It runs with no account, in which case there is no
  Drifterr server in the picture whatsoever; the server-side components are
  accounts & billing (identity + plan) and, on a Team plan, the rule packs and
  rule counts **you explicitly choose to share** — never a span, a goal, a prompt,
  a session id or a file path, and the panel prints the exact payload first. Not a
  promise —
  [enforced in CI](crates/proxy/tests/egress.rs) — including the local control
  API, which is **authenticated** so that a website you have open cannot read your
  sessions out of it ([`crates/proxy/tests/control_auth.rs`](crates/proxy/tests/control_auth.rs))
  — and laid out at
  [drifterr.app/proof](https://drifterr.app/proof). (The marketing website does
  count its own page views — first-party, cookieless, no visitor id. The boundary
  is spelled out on that page.)
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

**No account required.** Detection, constraints, standing orders and re-anchor all
work signed out and offline — an account exists only to attach a paid plan. Every
install starts with **14 days of Pro**, tracked locally (no card, no signup). After
that, Free keeps the whole detection loop and unlimited sessions; Pro adds
unlimited history, the drift map and automatic re-anchor injection — see
[Pricing](https://drifterr.app/#pricing).

> **First launch:** releases are currently **unsigned**, so macOS Gatekeeper and
> Windows SmartScreen will warn you. On macOS: right-click the app → **Open** →
> **Open**. On Windows: **More info** → **Run anyway**. Full steps on
> [drifterr.app/download](https://drifterr.app/download) and in
> [`docs/TROUBLESHOOTING.md`](docs/TROUBLESHOOTING.md).

**Using Claude Code? Nothing to set up.** Drifterr auto-watches your local
sessions (`~/.claude/projects`) — no keys, no env vars, fully local. It warns at
the exact turn a reply breaks one of your rules. (Override the watched dir with
`DRIFTERR_WATCH_DIR`.)

**Let the agent check itself.** Run `drifterr-proxy mcp --install` (or
`claude mcp add drifterr -- drifterr-proxy mcp`) and the agent gets two tools:

| Tool | What the agent asks |
| --- | --- |
| `drifterr_anchor` | "What did you actually ask for, which rules are in force, and what did you already reject?" |
| `drifterr_check` | "Before I hand this back — does it break any of them?" |

`drifterr_check` runs the **same deterministic rules as the live engine and CI**, over
the engine's own rule objects rather than a re-parse of their labels, so a self-check,
a menubar warning and a CI failure can never disagree. It is local, needs no model
call, and reports rules it cannot verify as *unverified* rather than passing — an
agent is never told "clean" about something nobody looked at. This is the one path
where the violation simply doesn't happen: prevention costs one cheap local call,
detection costs a wasted turn plus your attention.

**Catch it *before* the reply, not after.** Run `drifterr-proxy hook --install` and
add the printed block to `~/.claude/settings.json`. Drifterr then restates your goal
and live constraints as context on a drifting turn — *before* the model answers —
instead of telling you about the broken rule afterwards. Prevention beats detection,
and on the file-watch channel there is no request to rewrite, so a hook is the only
honest way to do it. The hook can never break your prompt: every failure path
(Drifterr not running, a timeout, malformed input) exits silently and the turn
proceeds untouched.

**Your rules file is the anchor.** If the project has a `CLAUDE.md`, `AGENTS.md`
or `.cursor/rules`, Drifterr imports the rules it can check deterministically —
"no new dependencies", "never use `any`", "no `console.log`", "keep functions
under 50 lines" — so there is nothing to retype. Prose it cannot verify is left
alone rather than guessed at. Edit or retire anything it imported from the panel.

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

**✅ shipped** means it is in the latest published release. **🟡 on `main`** means
it is built, tested and merged, but not in a binary you can download yet — see
[`CHANGELOG.md`](CHANGELOG.md) for what is waiting on the next tag. The two used to
be conflated, which made a reader assume a downloadable feature that wasn't.

| Area | State | Notes |
|---|---|---|
| Proxy channel (relay + exact saturation) | ✅ shipped | Byte-for-byte SSE passthrough, detection off the response path |
| Claude Code channel (file watch) | ✅ shipped | **Zero-config** — auto-watches `~/.claude/projects`, no keys |
| Constraint signal (deterministic, hard) | ✅ shipped | EN/FR phrasings; code rules (no JS/TODO/console.log/`any`/eval/secrets, no new deps, protected files, word & line caps) |
| Saturation signal (hard) | ✅ shipped | Exact only via the proxy |
| Degradation signal (soft) | ✅ shipped | Looping, verbosity, hedging (EN/FR) |
| Goal-alignment signal (soft) | ✅ shipped | Scale-free trend test (relative + absolute decline), tunable via `DRIFTERR_GOAL_*` and calibratable with `--sweep`. Runs on the lexical embedder by default; an optional local **ONNX semantic** model (bge-small, ~127MB) is fetched **on demand**, not bundled. **Not yet calibrated on real sessions** — thresholds stay conservative |
| Decision-coherence judge (soft) | ✅ shipped | Opt-in, BYOK (your OpenRouter key); fail-safe (degrades, never blocks) |
| Auto-intent (AI infers goal + constraints) | ✅ shipped | Opt-in, BYOK; continuous re-baseline |
| Re-anchor (snapshot + preamble) | ✅ shipped | Copy anywhere; auto-inject on the proxy channel, or on Claude Code via `drifterr-proxy hook` (a `UserPromptSubmit` hook, so the reminder lands *before* the reply). Automatic injection is Pro |
| Re-anchor outcome tracking | 🟡 on `main` | Records whether the same cause stayed quiet afterwards — "held for 3 turns" / "broke again on turn 7". Undecided stays undecided |
| Weekly report | 🟡 on `main` | `GET /report`, or the panel's **Last 7 days**. Flags grouped by cause; generated locally, offline |
| Standing orders (cross-session rules) | ✅ shipped | |
| MCP server (agent self-checks) | 🟡 on `main` | `drifterr-proxy mcp` gives the agent `drifterr_anchor` + `drifterr_check`, so a violation can be prevented rather than reported. Same rule objects as the engine — a self-check, a menubar warning and a CI failure cannot disagree ([`crates/proxy/src/mcp.rs`](crates/proxy/src/mcp.rs)) |
| Rule packs (portable, shareable) | 🟡 on `main` | Natural-language rules, never compiled regexes, so a pack stays reviewable and improves as inference does. Apply to a session, or splice into `CLAUDE.md` so the agent is told too. Shipped packs live in [`packs/`](packs/) under CC BY ([`crates/engine/src/pack.rs`](crates/engine/src/pack.rs)) |
| CI mode (`drifterr-proxy check`) | 🟡 on `main` | Same deterministic rules over a diff, with GitHub annotations. Exits 2 — never 0 — when nothing was verifiable, so a misconfigured check can't read as a passing one ([`.github/actions/drifterr-check`](.github/actions/drifterr-check)) |
| Team sharing (packs + rule counts) | 🟡 on `main` | Shared packs and per-rule counts, gated on the Team plan. Rule names and counts only: no spans, goals, prompts, session ids, file paths or model names, and a session-inferred rule id is withheld because publishing even the id reveals that the user said something. Enforced in the client filter, a database `CHECK`, and CI ([`crates/proxy/src/team.rs`](crates/proxy/src/team.rs)) |
| Menubar app (tray + panel) | ✅ shipped | Auto-hide on blur, resumes state |
| Rules-file import (`CLAUDE.md`, `.cursor/rules`) | ✅ shipped | Checkable rules imported automatically from the project's own rules file — nothing to retype ([`crates/engine/src/rules_file.rs`](crates/engine/src/rules_file.rs)) |
| Exact saturation on non-proxy channels | ❌ **not possible** | Investigated and rejected with measurements. A Claude Code transcript's `usage` records are cumulative billing counters (one real session: 679,390 "prompt" tokens against a 200k window, 676/851 records over it), and the transcript itself outlives context compaction with no marker for where. Occupancy is unknowable from a file, so the channel reports a **lower bound** and the hard signal abstains from RED rather than crying wolf. Exactness stays proxy-only |
| Detection eval harness + release gate | ✅ shipped | Metrics + zero-hard-FP gate, tunable thresholds ([`eval/thresholds.conf`](eval/thresholds.conf), [`eval/SCHEMA.md`](eval/SCHEMA.md)) |
| Control-API access boundary | ✅ shipped | The local control API is authenticated (per-install token, mode `0600`) and answers only first-party origins, so a page you have open cannot read your goals, spans or re-anchor snapshots out of it. Replayed as tests in [`crates/proxy/tests/control_auth.rs`](crates/proxy/tests/control_auth.rs) |
| Data controls (retention, delete) | 🟡 on `main` | A retention window that actually **deletes**, per-session and delete-everything controls in the panel, and mode `0600` on the database. Previously the plan's "7 days of history" was a display filter and the text stayed on disk forever |
| Verified entitlements | 🟡 on `main` | The accounts backend signs a short-lived plan assertion and the proxy verifies it, instead of being told which plan to apply. [`docs/ACCOUNTS.md`](docs/ACCOUNTS.md) states plainly what that does *not* buy — local software cannot be tamper-proof |
| Dependency audit | 🟡 on `main` | `cargo-deny` (advisories, licences, sources) and `npm audit` on every PR, policy in [`deny.toml`](deny.toml). It found and fixed two real advisories on its first run |
| Egress guarantee (CI-enforced) | ✅ shipped | [`crates/proxy/tests/egress.rs`](crates/proxy/tests/egress.rs) |
| Browser extension (MV3) | 🚧 partial | Built and working; **not store-published**, so it's a manual install |
| Signed / notarized desktop builds | 🚧 partial | Pipeline is in place ([`.github/workflows/release.yml`](.github/workflows/release.yml)); **certificates not yet provisioned**, so releases still ship unsigned and the OS warns on first launch |
| Open evaluation corpus | ✅ shipped | [`fixtures/`](fixtures/), [`eval/`](eval/) and [`packs/`](packs/) are **CC BY 4.0** while the engine stays proprietary, so the accuracy claims are independently verifiable and a contributor's annotation work isn't locked to one vendor |
| Validated on a real corpus | 📋 planned | **The honest state:** detection is validated against 8 hand-written fixtures in [`eval/`](eval/), authored by the same person who wrote the engine, and [`eval/blind/`](eval/blind/) is still empty. That is enough to catch regressions and nowhere near enough to publish an accuracy number — so we don't. The next real milestone is a few hundred annotated sessions and a held-out blind split |

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
