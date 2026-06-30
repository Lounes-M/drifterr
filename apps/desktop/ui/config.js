// Public config for the menubar webview. The publishable/anon key is safe to
// expose (Row Level Security protects data). Keep these in sync with
// apps/landing/config.js. (See supabase/README.md.)
//
// `??=` so a host (e.g. the hermetic test harness) can pre-set these before this
// script runs; in production nothing pre-sets them, so the real values apply.
window.DRIFTERR_SUPABASE_URL ??= "https://osjwjlyeqshhesunnite.supabase.co";
window.DRIFTERR_SUPABASE_ANON_KEY ??= "sb_publishable_dGHpPqr3leDKfgB5ufztMw_51peO0pr";

// Where the user manages their plan in the browser (Stripe checkout/portal live
// on the web). Change this if your site is on a different domain.
window.DRIFTERR_SITE_URL ??= "https://drifterr.app";
