# Contributing to Drifterr

Thanks for your interest. Drifterr is source-available but **not open-source**
(see [`LICENSE`](LICENSE)) — you're welcome to read the code, evaluate it, and
propose improvements. This guide keeps contributions smooth and consistent with
the project's non-negotiables.

## Licensing of contributions

By submitting a contribution (a pull request, patch, or suggestion), you agree
that your contribution is licensed to Drifterr under the same terms as the
project, and that Drifterr may relicense the combined work. Don't submit code
you don't have the right to license.

## Ground rules (architecture non-negotiables)

These come from [`CLAUDE.md`](CLAUDE.md) and are enforced in review:

- **The engine is channel-agnostic.** It only ever sees the normalized
  `Conversation`. Adding a source = writing an adapter that emits that shape —
  never special-casing a channel inside the engine.
- **Signals stay separate, each with evidence.** Never fuse them into one opaque
  score. Every signal carries its own state + evidence (turn index, constraint
  id, offending span) so the UI can *name* the cause.
- **Hard vs soft.** Only hard signals (constraints, saturation) may drive RED.
  Soft signals cap at AMBER unless several converge. **A hard signal that cries
  wolf is worse than one that stays quiet** — prefer under-claiming to false
  positives. The eval gate (`--gate`) enforces zero hard-signal false positives.
- **Local-first is sacred.** No chat content ever leaves the machine. If your
  change touches the network, it must not carry conversation content anywhere
  but the user's configured provider — the egress test
  (`crates/proxy/tests/egress.rs`) will fail otherwise, by design.
- **The proxy must never break a user's request.** Streaming is byte-for-byte
  passthrough; detection runs off the response path; parsing never panics.

## Development

Requires a recent stable Rust toolchain and Node 18+.

```bash
cargo test --workspace                        # engine, store, proxy, e2e, egress
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cargo run -p drifterr-engine --example eval -- eval/   # detection metrics + gate

cd apps/desktop/ui && npm install && npm test  # headless menubar UI (Playwright)
```

The Tauri desktop shell (`apps/desktop/src-tauri`) is excluded from the
workspace; build it on a dev machine with `cargo tauri dev`.

## Pull requests

- **One PR per milestone / logical change.** Keep mechanical reformatting in its
  own commit, separate from feature work.
- **Branch naming:** `claude/<short-topic>` (e.g. `claude/m3-intervention`).
- **Never push directly to `main`.** Everything merges via a PR.
- Add tests for behavior changes. Detection changes must keep the eval gate
  green (`cargo run -p drifterr-engine --example eval -- eval/ --gate`).
- Keep the README honest: if you ship or change a capability, update the
  **Status** table so no claim overshoots what actually runs.

## Reporting bugs & ideas

- **Security issues:** do not open a public issue — see [`SECURITY.md`](SECURITY.md).
- **Bugs / features:** open a GitHub issue with clear repro steps or a concrete
  proposal.

## Adding a detection signal or channel

- A new **channel** is an adapter that emits the normalized `Conversation` +
  `ContextState`. Put it under `crates/adapters` and add fixtures.
- A new **signal** lives in `crates/engine/src/signals`, carries its own
  evidence, and respects hard/soft. Add annotated cases to `eval/` (and, if it's
  deterministic and must always be right, to `fixtures/`). See
  [`eval/SCHEMA.md`](eval/SCHEMA.md).
