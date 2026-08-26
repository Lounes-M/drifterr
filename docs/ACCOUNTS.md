# Accounts & billing architecture

Drifterr is free to download and runs fully locally. Accounts exist so people
can pick a plan (Free / Pro / Team) and so paid features can be unlocked later.
This document explains the boundary and the flow; the setup steps live in
[`supabase/README.md`](../supabase/README.md).

## The bright line

> The server knows **who you are** and **what plan you're on**. It never knows
> **what you talked about.**

Conversations, prompts, signals, drift scores and re-anchor snapshots stay in
local SQLite on the user's machine. The backend (Supabase + Stripe) stores only:

- `profiles` — user id, email, Stripe customer id
- `subscriptions` — current plan, status, period end
- `plans` — the public catalog

No chat content, ever. If a feature would require sending conversation data to
the server, it does not belong here — it belongs in the local engine.

## What entitlement enforcement can and cannot do

Drifterr runs on the user's machine, so this needs stating plainly rather than
implied.

The accounts backend signs a short-lived assertion of the plan — *this user, this
plan, expiring then* — with a key only it holds, and the proxy verifies it against
a public key compiled into the release build (`crates/proxy/src/plan_token.rs`).
Until that existed the panel simply *told* the proxy which plan to apply and the
proxy stored it, so a single unauthenticated request granted Team.

What signing buys is real: a plan cannot be asserted, only presented; a
cancellation stops mattering when the token lapses rather than at the next
reinstall; and there is one place in the system that answers "what plan is this?".

What it does not buy, and cannot: tamper-proofing. The user owns the machine, the
source is public, and a determined person can patch the binary or the embedded key.
That is a property of local software, not a gap to be closed, and no amount of
obfuscation changes it — it only makes the code worse. We treat paid plans the way
we already treat the local trial: a boundary that stops accidents and scripts, not
one that stops the machine's owner.

A build with no key configured (a development build, a self-hoster) accepts the
plain assertion and reports `"verified": false` at `GET /entitlement`, so
"Pro" and "Pro, unverified" are never the same string.


## Components

```
┌─────────────────────┐         ┌──────────────────────────────┐
│ Desktop app (Tauri) │         │ Landing site (Vercel, static) │
│  menubar webview    │         │  pricing / auth / account     │
└──────────┬──────────┘         └───────────────┬──────────────┘
           │ supabase-js (auth + me)            │ supabase-js
           ▼                                    ▼
        ┌──────────────────────────────────────────────┐
        │ Supabase: Auth + Postgres (RLS) + Edge funcs  │
        │  me / stripe-checkout / stripe-portal         │
        └───────────────┬──────────────────────────────┘
                        │ Stripe API + webhook
                        ▼
                 ┌──────────────┐
                 │    Stripe    │  (subscriptions, billing portal)
                 └──────────────┘
```

## Flows

**Sign up / log in.** The web and the desktop app both use Supabase Auth
(email + OAuth). On first `auth.users` insert a trigger creates a `profiles` row
and a free `subscriptions` row, so every account starts on Free.

**Choose a paid plan.** Client calls `stripe-checkout` with `{plan, interval}`;
gets a Checkout URL; Stripe collects payment.

**Plan is granted.** Stripe fires `checkout.session.completed` /
`customer.subscription.*` to `stripe-webhook`, which verifies the signature and
upserts the `subscriptions` row (service role). This is the **only** place a paid
plan is granted — a client can never grant itself one (RLS denies client writes).

**Manage / cancel.** Client calls `stripe-portal` → Stripe Billing Portal.

**Read entitlement.** Client calls `me` (or selects `my_entitlement`) to render
the current plan. Entitlements are defined in `plans.features` but not yet
enforced — gating is a follow-up once the plumbing is proven.

## Roadmap

1. ✅ **Backend foundation** — schema, RLS, edge functions, setup guide.
2. ✅ **Web** — pricing wired to checkout, `/login` `/signup` `/account` pages.
3. ✅ **Desktop** — login gate on launch, account view, plan pill + upgrade
   nudge; plan changes open the hosted web checkout/portal in the browser.
   Configure via `apps/desktop/ui/config.js` (same keys as the web).
4. ✅ **Gating** — paid capabilities are enforced locally in the proxy from the
   plan the app reports (identity only — no chat content involved). The plan →
   capability mapping is the single source of truth in
   [`crates/proxy/src/entitlement.rs`](../crates/proxy/src/entitlement.rs):

   | Capability | Free | Pro | Team |
   | --- | :-: | :-: | :-: |
   | Local signals + manual re-anchor | ✅ | ✅ | ✅ |
   | Tracked sessions at once | 1 | ∞ | ∞ |
   | Drift map (history) | 🔒 | ✅ | ✅ |
   | Auto-re-anchor (proxy) | 🔒 | ✅ | ✅ |
   | Shared rule packs · team rule counts | 🔒 | 🔒 | ✅ |
   | SSO | 🔒 | 🔒 | roadmap |

   The desktop app `POST`s the plan to the proxy's `/entitlement` after `/me`;
   `/status` then returns the active `entitlement` so the menubar can lock
   features and prompt to upgrade. Enforcement of session cap, drift map and
   auto-re-anchor is live; **team sharing / SSO** are the next hookups.

5. ✅ **Teams** — shared rule packs and metadata-only rule counts
   ([`supabase/migrations/0002_teams.sql`](../supabase/migrations/0002_teams.sql)).

   This is the one feature that puts anything from a working session on a
   server, so the boundary is enforced in three independent places rather than
   documented once:

   * **The client filter** ([`crates/proxy/src/team.rs`](../crates/proxy/src/team.rs))
     builds the payload and drops everything that is not a shared pack or a
     count keyed by a pack-scoped rule id. What it drops is counted and shown to
     the user, never silently swallowed.
   * **The database** `CHECK`-constrains `team_rule_stats.rule_id` to the
     `pack:rule` shape, so a buggy or compromised client still cannot write a
     session-local id.
   * **CI** (`crates/proxy/tests/egress.rs`) drives a real violation through the
     engine and asserts the payload contains no span, goal, session id, model
     name or prompt text.

   The subtle exclusion worth stating explicitly: a session-inferred rule id
   (`c1`) names a constraint the engine **mined from the user's own messages**,
   so publishing even the id-with-count would reveal that they said something.
   Only pack-scoped ids — which refer to config both sides already have — are
   shareable.

   `GET /team/share-preview` returns the exact payload, so the user reads it
   before anything is uploaded. The upload itself is performed by the layer that
   holds the account session; the crate that can see chat content deliberately
   carries no backend hostname, and CI enforces that too.

   What a team gets for it is the one question an individual cannot answer:
   **which of our rules actually catch things, and which only nag?**
   `team_rule_leaderboard` aggregates counts across the team with no per-member
   attribution. A rule with zero flags team-wide for a month is a rule to
   delete.

   The judge (decision-coherence signal) always runs through the **user's own**
   model provider — Drifterr never hosts model calls, so no chat content ever
   passes through Drifterr's infrastructure. That keeps the local-first line
   bright and means no per-customer model cost.

Until `config.js` is filled with a real Supabase project, both clients run
**accounts-free** — the desktop app skips the login gate, the proxy defaults to
the Free entitlement, and the site points at the free download, so nothing
breaks before the backend is provisioned.
