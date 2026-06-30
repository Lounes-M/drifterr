-- Drifterr accounts & billing schema.
--
-- Design notes (read these before touching the tables):
--   * Local-first is preserved. Conversations NEVER live here — they stay in the
--     user's local SQLite. This database only ever holds identity (who you are)
--     and billing (what plan you're on). Nothing about a chat reaches it.
--   * `plans` is public, read-only reference data. `profiles` and `subscriptions`
--     are per-user and locked down with RLS: you can read your own row and
--     nothing else. Only the service role (the Stripe edge functions) writes
--     subscription state — clients can never grant themselves a plan.
--   * Entitlements live in `plans.features` as plain JSON. For now nothing is
--     enforced (scaffolding first); the column exists so gating later is a data
--     change, not a migration.

-- ---------------------------------------------------------------------------
-- plans: the catalog. Three tiers — free, pro, team. Seeded below.
-- ---------------------------------------------------------------------------
create table if not exists public.plans (
  id                    text primary key,            -- 'free' | 'pro' | 'team'
  name                  text not null,
  blurb                 text not null default '',
  price_monthly_cents   integer not null default 0,
  price_yearly_cents    integer not null default 0,
  -- Stripe Price IDs (price_...). Null for free / not-yet-created. Filled in
  -- once you create the products in Stripe (see supabase/README.md).
  stripe_price_monthly  text,
  stripe_price_yearly   text,
  features              jsonb  not null default '{}'::jsonb,
  sort                  integer not null default 0,
  created_at            timestamptz not null default now()
);

-- ---------------------------------------------------------------------------
-- profiles: one row per auth user. Created automatically on sign-up.
-- ---------------------------------------------------------------------------
create table if not exists public.profiles (
  id                  uuid primary key references auth.users(id) on delete cascade,
  email               text,
  full_name           text,
  stripe_customer_id  text unique,
  created_at          timestamptz not null default now(),
  updated_at          timestamptz not null default now()
);

-- ---------------------------------------------------------------------------
-- subscriptions: the current plan + Stripe state for a user. One row per user.
-- Defaults to the free plan the moment they sign up; the webhook upgrades it.
-- ---------------------------------------------------------------------------
create table if not exists public.subscriptions (
  user_id                 uuid primary key references auth.users(id) on delete cascade,
  plan_id                 text not null default 'free' references public.plans(id),
  -- 'active' | 'trialing' | 'past_due' | 'canceled' | 'incomplete' | 'free'
  status                  text not null default 'free',
  stripe_subscription_id  text unique,
  stripe_price_id         text,
  interval                text,                        -- 'month' | 'year' | null
  seats                   integer not null default 1,
  cancel_at_period_end    boolean not null default false,
  current_period_end      timestamptz,
  updated_at              timestamptz not null default now()
);

-- ---------------------------------------------------------------------------
-- Row Level Security.
-- ---------------------------------------------------------------------------
alter table public.plans         enable row level security;
alter table public.profiles      enable row level security;
alter table public.subscriptions enable row level security;

-- Plans are public reference data — anyone (even anon) may read them so the
-- pricing page can render. Nobody but the service role may write.
drop policy if exists "plans are readable by everyone" on public.plans;
create policy "plans are readable by everyone"
  on public.plans for select
  using (true);

-- A user may read (and patch profile fields on) only their own profile.
drop policy if exists "read own profile" on public.profiles;
create policy "read own profile"
  on public.profiles for select
  using (auth.uid() = id);

drop policy if exists "update own profile" on public.profiles;
create policy "update own profile"
  on public.profiles for update
  using (auth.uid() = id)
  with check (auth.uid() = id);

-- A user may read only their own subscription. Writes are service-role only
-- (the Stripe edge functions) — there is deliberately no insert/update policy
-- for authenticated users, so RLS denies client writes outright.
drop policy if exists "read own subscription" on public.subscriptions;
create policy "read own subscription"
  on public.subscriptions for select
  using (auth.uid() = user_id);

-- ---------------------------------------------------------------------------
-- New-user bootstrap: create a profile + free subscription on sign-up.
-- ---------------------------------------------------------------------------
create or replace function public.handle_new_user()
returns trigger
language plpgsql
security definer
set search_path = public
as $$
begin
  insert into public.profiles (id, email, full_name)
  values (new.id, new.email, new.raw_user_meta_data ->> 'full_name')
  on conflict (id) do nothing;

  insert into public.subscriptions (user_id, plan_id, status)
  values (new.id, 'free', 'free')
  on conflict (user_id) do nothing;

  return new;
end;
$$;

drop trigger if exists on_auth_user_created on auth.users;
create trigger on_auth_user_created
  after insert on auth.users
  for each row execute function public.handle_new_user();

-- Keep updated_at honest.
create or replace function public.touch_updated_at()
returns trigger
language plpgsql
as $$
begin
  new.updated_at = now();
  return new;
end;
$$;

drop trigger if exists touch_profiles on public.profiles;
create trigger touch_profiles before update on public.profiles
  for each row execute function public.touch_updated_at();

drop trigger if exists touch_subscriptions on public.subscriptions;
create trigger touch_subscriptions before update on public.subscriptions
  for each row execute function public.touch_updated_at();

-- ---------------------------------------------------------------------------
-- entitlements view: a single, RLS-respecting read for clients (desktop app,
-- web account page). Returns the caller's plan + features in one round-trip.
-- ---------------------------------------------------------------------------
create or replace view public.my_entitlement
with (security_invoker = true) as
  select
    s.user_id,
    s.plan_id,
    p.name        as plan_name,
    s.status,
    s.interval,
    s.seats,
    s.cancel_at_period_end,
    s.current_period_end,
    p.features
  from public.subscriptions s
  join public.plans p on p.id = s.plan_id
  where s.user_id = auth.uid();
