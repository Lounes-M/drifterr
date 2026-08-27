// Shared Supabase client + auth/billing helpers for the landing site.
//
// Everything here runs in the browser. The anon key is public by design; data
// is protected by Row Level Security on the server. The edge functions
// (stripe-checkout / stripe-portal / me) live in supabase/functions.

// Vendored, not fetched. See apps/desktop/ui/scripts/vendor-supabase.sh for why a
// desktop app must not pull executable code from a CDN at runtime, and how to
// rebuild this bundle.
import { createClient } from "./vendor/supabase.js";

const URL = window.DRIFTERR_SUPABASE_URL || "";
const ANON = window.DRIFTERR_SUPABASE_ANON_KEY || "";

/** False until you fill config.js with a real project — lets the UI degrade. */
export const configured = !!URL && !URL.includes("YOUR-PROJECT") && !!ANON && !ANON.includes("YOUR_");

export const supabase = configured
  ? createClient(URL, ANON, { auth: { persistSession: true, autoRefreshToken: true } })
  : null;

/** The functions base, derived from the project URL. */
function functionsBase() {
  // https://<ref>.supabase.co  ->  https://<ref>.functions.supabase.co
  return URL.replace(".supabase.co", ".functions.supabase.co");
}

/** Current logged-in user, or null. */
export async function currentUser() {
  if (!supabase) return null;
  const { data } = await supabase.auth.getUser();
  return data?.user ?? null;
}

/** Current access token for authorizing edge-function calls. */
async function accessToken() {
  if (!supabase) return null;
  const { data } = await supabase.auth.getSession();
  return data?.session?.access_token ?? null;
}

/** Call an edge function with the user's bearer token. Returns parsed JSON. */
export async function callFunction(name, { method = "POST", body } = {}) {
  const token = await accessToken();
  if (!token) throw new Error("not signed in");
  const res = await fetch(`${functionsBase()}/${name}`, {
    method,
    headers: {
      Authorization: `Bearer ${token}`,
      apikey: ANON,
      "Content-Type": "application/json",
    },
    body: body ? JSON.stringify(body) : undefined,
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
  return data;
}

/** Start Stripe Checkout for a plan; redirects the browser to Stripe. */
export async function startCheckout(plan, interval = "year", quantity = 1) {
  const { url } = await callFunction("stripe-checkout", { body: { plan, interval, quantity } });
  if (url) window.location.href = url;
}

/** Open the Stripe Billing Portal; redirects the browser. */
export async function openPortal() {
  const { url } = await callFunction("stripe-portal", {});
  if (url) window.location.href = url;
}

/** Fetch the caller's profile + current entitlement. */
export async function fetchMe() {
  return callFunction("me", { method: "GET" });
}

export async function signOut() {
  if (supabase) await supabase.auth.signOut();
}

/** Sign in with an OAuth provider, returning to `next` after the round-trip. */
export async function signInWithProvider(provider, next = "/account") {
  if (!supabase) throw new Error("accounts not configured");
  return supabase.auth.signInWithOAuth({
    provider,
    options: { redirectTo: `${window.location.origin}${next}` },
  });
}

/** Read a query param off the current URL. */
export function qp(name) {
  return new URLSearchParams(window.location.search).get(name);
}
