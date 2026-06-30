-- Seed the three plans. Prices here are the source of truth the UI reads; the
-- Stripe Price IDs are filled in after you create the products in Stripe (see
-- supabase/README.md). Re-running is safe — upsert on id.
--
-- Apply with `supabase db reset` (local) or `supabase db push` then run this
-- file via the SQL editor / `psql` in production.

insert into public.plans (id, name, blurb, price_monthly_cents, price_yearly_cents, stripe_price_monthly, stripe_price_yearly, features, sort)
values
  (
    'free',
    'Free',
    'For trying drift detection on one chat.',
    0, 0,
    null, null,
    '{
      "sessions": 1,
      "drift_score": true,
      "manual_reanchor": true,
      "constraint_alerts": false,
      "drift_map": false,
      "team_sharing": false,
      "sso": false,
      "support": "community"
    }'::jsonb,
    0
  ),
  (
    'pro',
    'Pro',
    'For people who live in AI chats.',
    900, 8100,   -- $9/mo, $81/yr (~25% off => $6.75/mo billed annually)
    'price_1To3SqCyJUAgn3MDYGqagelU',   -- pro monthly
    'price_1To3T6CyJUAgn3MDGlHSFYSD',   -- pro yearly
    '{
      "sessions": null,
      "drift_score": true,
      "manual_reanchor": true,
      "constraint_alerts": true,
      "drift_map": true,
      "all_assistants": true,
      "team_sharing": false,
      "sso": false,
      "support": "priority"
    }'::jsonb,
    1
  ),
  (
    'team',
    'Team',
    'Shared anchors for whole squads.',
    1600, 14400,  -- $16/seat/mo, $144/seat/yr (~25% off => $12/seat/mo annually)
    'price_1To3TVCyJUAgn3MDtfsNzJRF',   -- team monthly
    'price_1To3TsCyJUAgn3MDCjPXb5B3',   -- team yearly
    '{
      "sessions": null,
      "drift_score": true,
      "manual_reanchor": true,
      "constraint_alerts": true,
      "drift_map": true,
      "all_assistants": true,
      "team_sharing": true,
      "analytics": true,
      "sso": true,
      "support": "priority"
    }'::jsonb,
    2
  )
on conflict (id) do update set
  name                 = excluded.name,
  blurb                = excluded.blurb,
  price_monthly_cents  = excluded.price_monthly_cents,
  price_yearly_cents   = excluded.price_yearly_cents,
  stripe_price_monthly = excluded.stripe_price_monthly,
  stripe_price_yearly  = excluded.stripe_price_yearly,
  features             = excluded.features,
  sort                 = excluded.sort;
