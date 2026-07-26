-- Drifterr teams: shared rule packs and metadata-only rule counts.
--
-- READ THIS BEFORE ADDING A COLUMN
--
-- This is the first Drifterr feature that puts anything from a user's working
-- session on a server, so the schema itself is where the local-first line is
-- drawn. Exactly two categories are permitted here:
--
--   1. CONFIG the user wrote or forked — rule packs. Text like
--      "Never use `any` types". This is not chat content; it is a rule.
--   2. COUNTS keyed by a pack-scoped rule id —
--      ('tight-scope:no-new-deps', 7). Nothing about *what* triggered it.
--
-- Categorically forbidden, and each for a specific reason:
--
--   * spans / offending text  — a literal excerpt of the model's output.
--   * goals                   — derived from the user's own first message.
--   * prompts, turns, replies — obviously.
--   * session ids             — a stable handle on one piece of work.
--   * file paths, repo names, branch names, model names — a fingerprint of
--                               what the person is building.
--   * session-inferred rule ids ('c1', 'c2') — these name a constraint the
--                               engine MINED FROM THE USER'S MESSAGES, so even
--                               publishing the id with a count reveals that
--                               they said something. `team_rule_stats.rule_id`
--                               therefore carries a CHECK constraint requiring
--                               the pack-scoped 'pack:rule' shape, and the
--                               client-side filter in crates/proxy/src/team.rs
--                               drops anything else before it is ever sent.
--   * timestamps finer than a day — a day bucket describes a rule's
--                               usefulness; a minute bucket describes a
--                               person's working rhythm.
--
-- If a proposed column does not fit category 1 or 2, it does not belong in this
-- database. Put it in the user's local SQLite.

-- ---------------------------------------------------------------------------
-- teams: one row per team. Created by its owner, who must be on a team plan.
-- ---------------------------------------------------------------------------
create table if not exists public.teams (
  id          uuid primary key default gen_random_uuid(),
  name        text not null check (length(trim(name)) between 1 and 80),
  owner_id    uuid not null references auth.users(id) on delete cascade,
  created_at  timestamptz not null default now(),
  updated_at  timestamptz not null default now()
);

create index if not exists teams_owner_idx on public.teams (owner_id);

-- ---------------------------------------------------------------------------
-- team_members: membership. 'admin' may manage packs and membership.
-- ---------------------------------------------------------------------------
create table if not exists public.team_members (
  team_id    uuid not null references public.teams(id) on delete cascade,
  user_id    uuid not null references auth.users(id) on delete cascade,
  role       text not null default 'member' check (role in ('admin', 'member')),
  joined_at  timestamptz not null default now(),
  primary key (team_id, user_id)
);

create index if not exists team_members_user_idx on public.team_members (user_id);

-- ---------------------------------------------------------------------------
-- team_packs: shared rule packs. `pack` is the exact JSON documented in
-- crates/engine/src/pack.rs — natural-language rules, never compiled regexes,
-- so a teammate can read and review it in a pull request.
--
-- `slug` is the id a member types (`--pack tight-scope`), unique per team.
-- ---------------------------------------------------------------------------
create table if not exists public.team_packs (
  id          uuid primary key default gen_random_uuid(),
  team_id     uuid not null references public.teams(id) on delete cascade,
  slug        text not null check (slug ~ '^[a-z0-9][a-z0-9._-]{0,63}$'),
  pack        jsonb not null,
  shared_by   uuid references auth.users(id) on delete set null,
  created_at  timestamptz not null default now(),
  updated_at  timestamptz not null default now(),
  unique (team_id, slug),
  -- Enforce the pack's shape at the boundary. A malformed pack that reached a
  -- teammate's machine would be untrusted input arriving with the authority of
  -- "your team set this rule".
  constraint team_packs_shape check (
    pack ? 'drifterrPack'
    and pack ? 'name'
    and jsonb_typeof(pack -> 'rules') = 'array'
    and jsonb_array_length(pack -> 'rules') <= 200
  )
);

-- ---------------------------------------------------------------------------
-- team_rule_stats: how often a shared rule fired, per member, per day.
--
-- The whole value of this table is answering "which of our rules actually earn
-- their keep, and which just nag?" — a question no individual can answer alone.
-- It needs counts and nothing else, which is why there is nothing else here.
--
-- `rule_id` is CHECK-constrained to the pack-scoped shape: the database refuses
-- a session-local id even if a client is compromised or buggy. Defence in depth
-- over the client-side filter, not instead of it.
-- ---------------------------------------------------------------------------
create table if not exists public.team_rule_stats (
  team_id   uuid not null references public.teams(id) on delete cascade,
  user_id   uuid not null references auth.users(id) on delete cascade,
  -- 'pack-slug:rule-id'. Two non-empty lowercase slugs, one colon.
  rule_id   text not null check (rule_id ~ '^[a-z0-9][a-z0-9._-]{0,63}:[a-z0-9][a-z0-9._-]{0,63}$'),
  -- Day bucket. Deliberately a date, not a timestamp.
  day       date not null,
  flagged   integer not null default 0 check (flagged >= 0),
  primary key (team_id, user_id, rule_id, day)
);

create index if not exists team_rule_stats_team_day_idx
  on public.team_rule_stats (team_id, day desc);

-- ---------------------------------------------------------------------------
-- Row Level Security.
--
-- Membership is checked through a security-definer helper rather than a
-- subquery on team_members inside team_members' own policy, which would
-- recurse.
-- ---------------------------------------------------------------------------
create or replace function public.is_team_member(t uuid)
returns boolean
language sql
security definer
set search_path = public
stable
as $$
  select exists (
    select 1 from public.team_members m
    where m.team_id = t and m.user_id = auth.uid()
  );
$$;

create or replace function public.is_team_admin(t uuid)
returns boolean
language sql
security definer
set search_path = public
stable
as $$
  select exists (
    select 1 from public.team_members m
    where m.team_id = t and m.user_id = auth.uid() and m.role = 'admin'
  );
$$;

alter table public.teams           enable row level security;
alter table public.team_members    enable row level security;
alter table public.team_packs      enable row level security;
alter table public.team_rule_stats enable row level security;

-- Teams: members read; only an admin renames; only the service role (after the
-- plan check) creates or deletes, so a Free account cannot mint a team.
drop policy if exists "members read their team" on public.teams;
create policy "members read their team"
  on public.teams for select
  using (public.is_team_member(id));

drop policy if exists "admins update their team" on public.teams;
create policy "admins update their team"
  on public.teams for update
  using (public.is_team_admin(id))
  with check (public.is_team_admin(id));

-- Membership: a member sees the roster (that is the point of a team); only an
-- admin changes it.
drop policy if exists "members read the roster" on public.team_members;
create policy "members read the roster"
  on public.team_members for select
  using (public.is_team_member(team_id));

drop policy if exists "admins manage the roster" on public.team_members;
create policy "admins manage the roster"
  on public.team_members for all
  using (public.is_team_admin(team_id))
  with check (public.is_team_admin(team_id));

-- Packs: every member reads them (they govern everyone's work); admins write.
drop policy if exists "members read team packs" on public.team_packs;
create policy "members read team packs"
  on public.team_packs for select
  using (public.is_team_member(team_id));

drop policy if exists "admins manage team packs" on public.team_packs;
create policy "admins manage team packs"
  on public.team_packs for all
  using (public.is_team_admin(team_id))
  with check (public.is_team_admin(team_id));

-- Stats: a member writes only their OWN rows, and reads the team's aggregate.
-- Writing someone else's counts would let one member fabricate another's
-- record; there is no reason to allow it and a clear reason not to.
drop policy if exists "members read team stats" on public.team_rule_stats;
create policy "members read team stats"
  on public.team_rule_stats for select
  using (public.is_team_member(team_id));

drop policy if exists "members write own stats" on public.team_rule_stats;
create policy "members write own stats"
  on public.team_rule_stats for insert
  with check (public.is_team_member(team_id) and user_id = auth.uid());

drop policy if exists "members update own stats" on public.team_rule_stats;
create policy "members update own stats"
  on public.team_rule_stats for update
  using (public.is_team_member(team_id) and user_id = auth.uid())
  with check (public.is_team_member(team_id) and user_id = auth.uid());

drop trigger if exists touch_teams on public.teams;
create trigger touch_teams before update on public.teams
  for each row execute function public.touch_updated_at();

drop trigger if exists touch_team_packs on public.team_packs;
create trigger touch_team_packs before update on public.team_packs
  for each row execute function public.touch_updated_at();

-- ---------------------------------------------------------------------------
-- team_rule_leaderboard: the one question this feature exists to answer.
--
-- "Which of our shared rules actually catch things, and which only nag?" —
-- aggregated across the team, per rule, with no per-member attribution in the
-- output. A rule with zero flags across the whole team over a month is a rule
-- to delete, and knowing that is worth more than any dashboard of counts.
-- ---------------------------------------------------------------------------
create or replace view public.team_rule_leaderboard
with (security_invoker = true) as
  select
    s.team_id,
    s.rule_id,
    sum(s.flagged)::bigint       as flagged,
    count(distinct s.user_id)    as members_affected,
    min(s.day)                   as first_seen,
    max(s.day)                   as last_seen
  from public.team_rule_stats s
  group by s.team_id, s.rule_id;
