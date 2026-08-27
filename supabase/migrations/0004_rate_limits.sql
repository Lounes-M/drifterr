-- Durable rate limiting for the authenticated edge functions.
--
-- `stripe-checkout` is authenticated, but a signed-in caller could create
-- unlimited Checkout Sessions: each one is a Stripe API call against our account
-- and our rate limit, and a loop costs the caller nothing. `stripe-portal` is the
-- same shape.
--
-- This lives in Postgres rather than in memory because edge functions scale out —
-- an in-process counter gives an attacker one bucket per instance, which is a
-- cost control rather than a limit. Here the limit is real, because there is one
-- row per (subject, action) no matter how many instances are running.
--
-- Unlike the marketing site's limiter (apps/landing/api/_ratelimit.js), keying
-- per-caller is fine here: these endpoints already require a JWT, so the
-- identifier exists whether or not we count with it. No new identity is created.

create table if not exists public.rate_limits (
  -- The authenticated user, and what they are doing. Separate actions get
  -- separate budgets so hitting the portal cannot lock someone out of checkout.
  subject     uuid   not null,
  action      text   not null,
  -- Start of the current window, and how many calls have landed inside it.
  window_start timestamptz not null default now(),
  count       integer not null default 0,
  primary key (subject, action)
);

comment on table public.rate_limits is
  'Per-user call budgets for the authenticated edge functions. Service-role only; see _shared/rate_limit.ts.';

-- Service role only. RLS on with no policy means exactly that, and without it
-- the table would be readable through PostgREST by anyone with the anon key —
-- which would leak who is buying what and when.
alter table public.rate_limits enable row level security;

-- Consume one unit of budget, atomically.
--
-- Doing this in a function rather than as read-then-write in the edge function is
-- the point: two concurrent calls would both read the old count and both write
-- count+1, so a limit of five would let ten through under exactly the load it
-- exists to handle. `insert ... on conflict do update` with the comparison inside
-- the statement leaves no window between the two.
--
-- Returns true when the call is allowed.
create or replace function public.consume_rate_limit(
  p_subject uuid,
  p_action  text,
  p_limit   integer,
  p_window  interval
) returns boolean
language plpgsql
security definer
set search_path = public
as $$
declare
  v_count integer;
begin
  insert into public.rate_limits (subject, action, window_start, count)
  values (p_subject, p_action, now(), 1)
  on conflict (subject, action) do update
    set
      -- Expired window: start a new one. Otherwise increment inside it.
      window_start = case
        when public.rate_limits.window_start < now() - p_window then now()
        else public.rate_limits.window_start
      end,
      count = case
        when public.rate_limits.window_start < now() - p_window then 1
        else public.rate_limits.count + 1
      end
  returning count into v_count;

  return v_count <= p_limit;
end;
$$;

revoke all on function public.consume_rate_limit(uuid, text, integer, interval) from public, anon, authenticated;
