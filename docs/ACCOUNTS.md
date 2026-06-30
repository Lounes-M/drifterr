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

1. **Backend foundation** (this) — schema, RLS, edge functions, setup guide.
2. **Web** — pricing wired to checkout, `/login` `/signup` `/account` pages.
3. **Desktop** — login gate on first launch, plan selector, account view.
4. **Gating** — enforce `plans.features` (decide what Pro/Team actually unlock).
