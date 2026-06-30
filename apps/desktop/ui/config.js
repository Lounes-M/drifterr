// Public config for the menubar webview. The anon key is safe to expose (Row
// Level Security protects data). Fill these after creating your Supabase project
// (see supabase/README.md). Until they're set, the app skips the login gate and
// runs exactly as before — accounts are purely additive.
//
// Set the same values you put in apps/landing/config.js.
window.DRIFTERR_SUPABASE_URL = "https://YOUR-PROJECT.supabase.co";
window.DRIFTERR_SUPABASE_ANON_KEY = "YOUR_PUBLIC_ANON_KEY";

// Where the user manages their plan in the browser (Stripe checkout/portal live
// on the web). Defaults to the production site.
window.DRIFTERR_SITE_URL = "https://drifterr.app";
