# Security Policy

Drifterr is **local-first**: your conversations, prompts, signals and drift
scores never leave your machine. The only server-side component is accounts &
billing (Supabase + Stripe), which holds identity (email, plan, subscription
status) and nothing else. Security reports matter to us — privacy is the product.

## The local-first boundary (what to hold us to)

- **Chat content never leaves the machine.** It lives in local SQLite. Model
  calls (the relay, the optional judge) go through **your own provider** with
  **your own key** — never through a Drifterr server.
- **Team sharing is the one deliberate exception, and it is config only.** On a
  Team plan a user may share **rule packs** (configuration text they wrote) and
  **counts keyed by a pack-scoped rule id**. Spans, goals, prompts, replies,
  session ids, file paths, model names, and any timestamp finer than a day are
  excluded — as is a session-inferred rule id, because it was derived from the
  user's own messages. Enforced in three independent places: the payload builder
  ([`crates/proxy/src/team.rs`](crates/proxy/src/team.rs)), a `CHECK` constraint
  in [`supabase/migrations/0002_teams.sql`](supabase/migrations/0002_teams.sql),
  and a CI test that drives a real violation through the engine and fails if any
  of it appears. Anything reaching that payload which is not a shared pack or a
  rule count is a **critical** report.
- Only two components in this repository make network calls: the **proxy**
  (relays your request to the provider you configured) and the **judge** (calls
  your own OpenRouter key, opt-in). Both are covered by an enforced egress test:
  see `crates/proxy/tests/egress.rs`, run on every CI build. The crates that
  hold or parse chat content (`engine`, `store`, `adapters`, `tokenizer`,
  `embeddings`, `intervention`) have **zero** network dependencies.
- The proxy is on the request path, so it is treated as safety-critical:
  streaming is byte-for-byte passthrough, detection runs off the response path,
  and parsing is best-effort and never panics.

If you find a way to make chat content leave the machine to anywhere but the
user's configured provider, that is a **critical** report — please tell us.

## Reporting a vulnerability

**Please do not open a public issue for security problems.**

- Use **GitHub → Security → Report a vulnerability** (private advisory) on this
  repository, or
- contact the maintainers privately via https://drifterr.app.

Include: a description, affected component/version, reproduction steps, and the
impact you see. A proof-of-concept helps.

### What to expect

- **Acknowledgement:** within 3 business days.
- **Assessment & triage:** within 10 business days, with a severity and a plan.
- **Fix & disclosure:** coordinated. We'll credit you (if you want) once a fix
  ships. Please give us reasonable time before any public disclosure.

## Scope

In scope: the proxy, engine, store, adapters, judge, the desktop app, the
browser extension, and the accounts/billing boundary (that identity-only data is
all that reaches the server, and chat never does).

Out of scope: vulnerabilities in third-party model providers, your own API keys'
handling outside Drifterr, and social-engineering attacks.

## Supported versions

Security fixes target the **latest released** desktop version. Older versions
are not maintained — update to the latest build.
