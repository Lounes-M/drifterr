-- Make the Stripe webhook safe to deliver twice, and safe to deliver late.
--
-- Stripe guarantees at-least-once delivery and does not guarantee order. The
-- handler assumed exactly-once and in-order, which is two real bugs against a
-- paying customer's account:
--
--   * A retry (our 5xx, a timeout, Stripe's own redelivery) re-ran the whole
--     sync. Idempotent for a plain upsert, but not once the handler needs to do
--     anything conditional, and impossible to audit after the fact.
--   * `customer.subscription.updated` events could arrive out of order. The
--     older one wins on a last-write-wins upsert, so a customer who upgraded
--     could be silently put back on the plan they upgraded *from* — and nothing
--     in the system would say why.
--
-- Two additions fix both, and neither needs application state.

-- ---------------------------------------------------------------------------
-- 1. Every event we have processed, so a redelivery is a no-op.
-- ---------------------------------------------------------------------------
create table if not exists public.stripe_events (
  -- Stripe's event id (evt_...). The primary key IS the idempotency guarantee:
  -- a concurrent redelivery loses the insert race rather than double-applying.
  id           text primary key,
  type         text not null,
  -- Stripe's own `created`, in epoch seconds. Kept for ordering and for
  -- answering "when did this actually happen" during a billing dispute.
  event_at     bigint not null,
  processed_at timestamptz not null default now()
);

comment on table public.stripe_events is
  'Processed Stripe webhook event ids. Insert-first; a duplicate insert means the event was already handled and must be skipped.';

-- Held for a year: long enough to cover any redelivery window and any dispute,
-- short enough that the table does not grow without bound.
create index if not exists stripe_events_processed_at_idx
  on public.stripe_events (processed_at);

-- ---------------------------------------------------------------------------
-- 2. Ordering: never let an older event overwrite a newer one.
-- ---------------------------------------------------------------------------
alter table public.subscriptions
  add column if not exists last_event_at bigint not null default 0;

comment on column public.subscriptions.last_event_at is
  'Stripe `created` of the newest event applied to this row. The webhook refuses to apply anything older, so out-of-order delivery cannot downgrade a customer.';

-- ---------------------------------------------------------------------------
-- 3. Neither table is client-readable.
-- ---------------------------------------------------------------------------
-- The webhook runs with the service role, which bypasses RLS. Enabling RLS with
-- no policy therefore means "the service role only" — which is exactly right for
-- a ledger of billing events. Without this, RLS being off would leave the table
-- readable through PostgREST by anyone with the anon key.
alter table public.stripe_events enable row level security;
