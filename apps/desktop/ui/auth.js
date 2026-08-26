// Supabase auth for the menubar webview.
//
// The desktop app authenticates directly here (email + password) so the panel
// can be gated behind a login. The session persists in the webview's storage.
// Stripe checkout / billing are hosted web flows, so plan changes open the
// browser at the site's /account and /#pricing — we never embed Stripe in-app.

// Vendored, not fetched. See apps/desktop/ui/scripts/vendor-supabase.sh for why a
// desktop app must not pull executable code from a CDN at runtime, and how to
// rebuild this bundle.
import { createClient } from "./vendor/supabase.js";

const URL = (typeof window !== "undefined" && window.DRIFTERR_SUPABASE_URL) || "";
const ANON = (typeof window !== "undefined" && window.DRIFTERR_SUPABASE_ANON_KEY) || "";
export const SITE_URL = (typeof window !== "undefined" && window.DRIFTERR_SITE_URL) || "https://drifterr.app";

/** False until config.js is filled — lets the app run gate-free as before. */
export const configured = !!URL && !URL.includes("YOUR-PROJECT") && !!ANON && !ANON.includes("YOUR_");

export const supabase = configured
  ? createClient(URL, ANON, { auth: { persistSession: true, autoRefreshToken: true } })
  : null;

function functionsBase() {
  return URL.replace(".supabase.co", ".functions.supabase.co");
}

export async function currentUser() {
  if (!supabase) return null;
  const { data } = await supabase.auth.getUser();
  return data?.user ?? null;
}

export async function signIn(email, password) {
  const { error } = await supabase.auth.signInWithPassword({ email, password });
  if (error) throw error;
}

export async function signUp(email, password, fullName) {
  const { data, error } = await supabase.auth.signUp({
    email,
    password,
    options: { data: { full_name: fullName || "" } },
  });
  if (error) throw error;
  return data;
}

export async function signOut() {
  if (supabase) await supabase.auth.signOut();
}

export async function fetchMe() {
  const { data } = await supabase.auth.getSession();
  const token = data?.session?.access_token;
  if (!token) throw new Error("not signed in");
  const res = await fetch(`${functionsBase()}/me`, {
    headers: { Authorization: `Bearer ${token}`, apikey: ANON },
  });
  const body = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(body.error || `HTTP ${res.status}`);
  return body;
}

/** Open a URL in the user's real browser (works in Tauri and plain webviews). */
export async function openExternal(url) {
  try {
    // Tauri v2 opener plugin exposes `openUrl`; the older shell plugin used
    // `open`. In a Tauri webview `window.open` is a no-op (there's no browser),
    // so we MUST go through a plugin — hence trying every known shape first.
    const opener = window.__TAURI__?.opener;
    if (opener?.openUrl) return void opener.openUrl(url);
    if (opener?.open) return void opener.open(url);
    const shell = window.__TAURI__?.shell;
    if (shell?.open) return void shell.open(url);
  } catch (_e) { /* fall through */ }
  window.open(url, "_blank", "noopener");
}
