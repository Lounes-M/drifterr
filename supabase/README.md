# Drifterr accounts & billing (Supabase + Stripe)

This directory is the backend for accounts and subscriptions. It is the **only**
server-side state in Drifterr, and it deliberately holds **just identity and
billing** — never conversations. Chats stay in the user's local SQLite; the
local-first promise is intact.

```
supabase/
  config.toml                 Supabase CLI project config (auth + functions)
  migrations/0001_*.sql        profiles / plans / subscriptions + RLS + triggers
  seed.sql                     the three plans (free / pro / team)
  functions/
    _shared/                   CORS + Supabase/Stripe client helpers
    stripe-checkout/           POST → Stripe Checkout URL (start a subscription)
    stripe-portal/             POST → Stripe Billing Portal URL (manage/cancel)
    stripe-webhook/            Stripe → us; the ONLY place a plan is granted
    me/                        GET → caller's profile + entitlement (1 call)
```

## Data model

- **plans** — public, read-only catalog. Seeded from `seed.sql`. `features` is a
  JSON blob of entitlements (not enforced yet — scaffolding first).
- **profiles** — one per auth user, auto-created on sign-up. Holds the Stripe
  customer id.
- **subscriptions** — one per user, defaults to `free`. Only the service role
  (the webhook) writes it; clients can read their own row via RLS and nothing
  else. `my_entitlement` is a security-invoker view that joins plan + sub for
  the caller in a single query.

## One-time setup

### 1. Create the project & link the CLI

```bash
npm i -g supabase            # or: brew install supabase/tap/supabase
supabase login
supabase link --project-ref <your-project-ref>
```

### 2. Apply the schema + seed

```bash
supabase db push                       # runs migrations/
# then run the seed (SQL editor, or):
supabase db execute --file supabase/seed.sql
```

### 3. Create the Stripe products

In the Stripe dashboard (Test mode first), create two **products** with a
recurring **monthly** and **yearly** price each:

| Plan | Monthly | Yearly |
| ---- | ------- | ------ |
| Pro  | $9.00   | $81.00 |
| Team | $16.00 (per seat) | $144.00 (per seat) |

Copy the four `price_...` ids and put them in the catalog (`supabase/seed.sql`,
in the `plans` rows) — price ids are not secret, so they live in the database,
not in function secrets. Re-run the seed after editing.

### 4. Wire the secrets

Only two secrets are needed (the price ids come from the catalog, the Supabase
keys are injected automatically):

```bash
cp supabase/functions/.env.example supabase/functions/.env
# edit .env: SITE_URL, STRIPE_SECRET_KEY  (STRIPE_WEBHOOK_SECRET after step 6)
supabase secrets set --env-file supabase/functions/.env
```

Or set them in the dashboard: Project → Edge Functions → Manage secrets.
Note: GitHub repo/Actions secrets do **not** reach the edge functions — they
must be set here, in Supabase.

### 5. Deploy the functions

```bash
supabase functions deploy stripe-checkout stripe-portal me
supabase functions deploy stripe-webhook --no-verify-jwt
```

### 6. Register the Stripe webhook

In Stripe → Developers → Webhooks, add an endpoint pointing at:

```
https://<project-ref>.functions.supabase.co/stripe-webhook
```

Subscribe to: `checkout.session.completed`,
`customer.subscription.created`, `customer.subscription.updated`,
`customer.subscription.deleted`. Copy the signing secret (`whsec_...`) into
`.env` as `STRIPE_WEBHOOK_SECRET` and re-run `supabase secrets set ...`.

### 7. (Optional) OAuth providers

Enable Google/GitHub in Supabase Auth and set
`GOOGLE_CLIENT_ID/SECRET`, `GITHUB_OAUTH_CLIENT_ID/SECRET`.

## Local development

```bash
supabase start                         # local stack (Postgres, Auth, Studio)
supabase functions serve --env-file supabase/functions/.env
stripe listen --forward-to localhost:54321/functions/v1/stripe-webhook
```

## What the clients call

| Endpoint | Method | Auth | Purpose |
| -------- | ------ | ---- | ------- |
| `/functions/v1/me` | GET | user JWT | profile + current plan |
| `/functions/v1/stripe-checkout` | POST | user JWT | `{plan, interval, quantity?}` → checkout URL |
| `/functions/v1/stripe-portal` | POST | user JWT | → billing portal URL |
| `/functions/v1/stripe-webhook` | POST | Stripe sig | grant/sync plan |

The web (`apps/landing`) and desktop (`apps/desktop`) integrations are wired in
follow-up PRs; this PR is the backend foundation.
