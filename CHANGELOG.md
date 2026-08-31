# Changelog

What changed, and why it mattered. Versions are the desktop app's; see
[`RELEASING.md`](RELEASING.md) for how one is cut.

This file starts at the first release that has one. Anything under **Unreleased**
is merged and tested on `main` but is **not** in a binary you can download — the
README's status table marks those rows 🟡 rather than ✅, because conflating the two
told readers a feature was available when it wasn't.

## 0.3.0 — 2026-08-31

### Security

- **The local control API is authenticated.** It used to bind `127.0.0.1` and answer
  everything with `Access-Control-Allow-Origin: *` and no credential. Localhost is
  not a boundary against a browser — the browser is already inside it — so any page
  a user had open could `fetch("http://127.0.0.1:8788/anchor")` and read their goal
  verbatim, every constraint, and the offending span of every violation.
  `POST /entitlement` granted Team; `POST /judge` pointed the model calls at a key
  the attacker owned. Now: a per-install token (mode `0600`) plus a first-party
  origin allowlist, two layers because they stop different attacks. Replayed as
  tests in `crates/proxy/tests/control_auth.rs`.
- **No third-party script at runtime.** The panel imported the Supabase SDK from a
  CDN and the desktop CSP was widened to allow it — a compromise there would have
  run arbitrary code in a webview holding the user's session. The SDK is vendored
  and `script-src` is now `'self'`.
- **Dependency audit on every PR.** `cargo-deny` and `npm audit`, policy in
  `deny.toml`. Its first run found two real advisories (`h2` unbounded empty DATA
  frames, a `crossbeam-epoch` pointer dereference); both are patched.
- Workspace crates are marked `publish = false`, so proprietary code cannot reach
  crates.io by a mistyped command.

### Detection

- **The rules-file importer no longer invents constraints.** Pointed at this
  repository's own `CLAUDE.md` it produced exactly one rule, and nobody had written
  it: a hard-wrapped paragraph containing "backend" and "keep" became a
  `server-side only` constraint that flagged RED on any reply containing
  `document.getElementById`. Statements are now sentences rather than physical
  lines, and a layer pin has to be structurally bound ("keep it server-side",
  "backend only") rather than merely co-present.
- **Imported rules are proposals.** They are checked and named but cap at AMBER
  until confirmed in the panel. The rule check is deterministic either way; what an
  importer reading English cannot guarantee is that the user ever asked for the
  rule — so a parser mistake now costs a glance, not a red alert.
- The "rules I cannot check" list is whole sentences instead of headings and
  wrapped fragments. A list nobody can read is a list nobody reads.

### Privacy

- **You can delete your history.** A retention window that actually deletes (swept
  at startup), per-session and delete-everything controls, and `VACUUM` so the text
  is gone from the file rather than merely unlinked. The Free plan's "7 days of
  history" was a display filter; every turn stayed on disk forever.
- The database and its sidecars are mode `0600`. They hold the full text of every
  turn, and the ambient umask left them readable by every other account on a shared
  machine. Applied on every open, so an existing install is fixed by restarting.
- `GET /diagnostics` and a panel button: counts, versions and settings, never a
  goal, a prompt or a span — asserted in `crates/proxy/tests/egress.rs`. Drifterr
  still has no telemetry; this is the honest way to make a bug report actionable.

### Accounts and billing

- **Plans are verified, not asserted.** `/me` signs a short-lived Ed25519 assertion
  and the proxy verifies it, instead of the panel telling the proxy which plan to
  apply. `docs/ACCOUNTS.md` states what that does *not* buy: local software cannot
  be tamper-proof, and pretending otherwise would be the same overclaiming the
  detection eval refuses to do.
- The Stripe webhook survives Stripe's actual delivery guarantees. Events are
  claimed by primary key before any work, out-of-order events are refused rather
  than applied, a failed handler releases its claim so the retry can retry, and
  `invoice.payment_failed` marks `past_due` instead of leaving a customer to
  discover a declined card weeks later.

### Testing what was only reviewed

- **The billing logic has tests.** Nothing in CI touched `supabase/functions/`,
  so the webhook's idempotency and ordering fixes sat on top of customer money
  unverified. The decisions are now separated from the I/O behind a `Ledger`
  interface, and eleven tests drive the sequences Stripe actually produces — an
  out-of-order update that must not downgrade, a redelivery that must lose the
  claim race, a handler that throws and must release its claim so the retry can
  retry.
- **The desktop shell has tests.** It was compiled and clippy-checked but never
  run, so `notification_for` — the code deciding whether to interrupt you — was
  unverified. Twelve tests cover the escalation rules, including that a red
  session must not re-notify on every 1.5-second poll and that Do Not Disturb
  suppresses an alert without swallowing the escalation behind it.
- Coverage is measured and reported in CI, deliberately not gated.

### The web surface

- **Security headers on the marketing site**, which had none: no CSP, no
  HSTS, no frame or content-type protection, on the pages that hold a Supabase
  session. `script-src` is now `'self'`, which meant moving the inline module
  blocks out of account/login/signup into files. A test asserts the policy and the
  markup still agree, since Vercel applies the headers and nothing else could
  catch a drift between them.
- **Rate limits.** `stripe-checkout` was authenticated but unbounded — a signed-in
  caller could create unlimited Checkout Sessions against our Stripe account.
  That budget is now per-user and durable in Postgres, incremented in a single
  statement so two concurrent calls cannot both read the old count. The marketing
  endpoints get a *global* bucket instead: keying on IP would create exactly the
  per-visitor identifier `/api/event` promises does not exist.

### Also

- Releases publish `SHA256SUMS`, and `install.sh` verifies against it — it
  downloads a binary and runs it, which deserves more than "TLS said it came from
  GitHub". A release without the manifest is refused rather than installed
  unverified.
- The self-check CI job checks something real. It was reporting "1 rule checked"
  because of the phantom rule above; with that gone the honest count from
  `CLAUDE.md` is zero, so the job also carries the shipped `security-basics` pack.
- Sessions can be deleted one at a time from the history view, not only all at
  once. `POST /data/forget` had accepted a session id since it existed and nothing
  in the UI ever sent one.
- The extension's README and store listing explain the pairing step, which they
  predated — a first-time user would otherwise install both halves and meet "Not
  paired yet" with no explanation.
- The extension packaging script ships all of `src/` and verifies every referenced
  script is in the zip. The enumerated list would have shipped a bundle that broke
  on first load.

## 0.2.5 — 2026-07-16

The last published release. See the
[GitHub releases](https://github.com/Lounes-M/drifterr/releases) for earlier notes.
