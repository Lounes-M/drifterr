// Public front-end config. The publishable/anon key is SAFE to expose in the
// browser — it's designed to be public and Row Level Security is what actually
// protects data. (See supabase/README.md.)
//
// `??=` so a host (e.g. the hermetic test harness) can pre-set these before this
// script runs; in production nothing pre-sets them, so the real values apply.
window.DRIFTERR_SUPABASE_URL ??= "https://osjwjlyeqshhesunnite.supabase.co";
window.DRIFTERR_SUPABASE_ANON_KEY ??= "sb_publishable_dGHpPqr3leDKfgB5ufztMw_51peO0pr";

// Funnel analytics for this **website only** — first-party, cookieless, no
// visitor identifier, and no PII (see analytics.js). Set to "" to switch it off
// entirely; the test harness pre-sets it empty so runs stay hermetic. Nothing in
// the desktop app reports anything, ever.
window.DRIFTERR_ANALYTICS_URL ??= "/api/event";
