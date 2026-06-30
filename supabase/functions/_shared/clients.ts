// Shared client constructors for the edge functions.
//
// Two Supabase clients matter here:
//   * userClient(req): scoped to the caller's JWT, so RLS applies. Use it to
//     answer "who is this request?" without trusting client-supplied ids.
//   * adminClient(): the service role, bypasses RLS. Use ONLY for writes the
//     user isn't allowed to make directly (granting a plan after Stripe says so).

import Stripe from "npm:stripe@17.5.0";
import { createClient, type SupabaseClient } from "npm:@supabase/supabase-js@2.47.10";

const SUPABASE_URL = Deno.env.get("SUPABASE_URL")!;
const ANON_KEY = Deno.env.get("SUPABASE_ANON_KEY")!;
const SERVICE_ROLE_KEY = Deno.env.get("SUPABASE_SERVICE_ROLE_KEY")!;

export const stripe = new Stripe(Deno.env.get("STRIPE_SECRET_KEY")!, {
  apiVersion: "2024-12-18.acacia",
  httpClient: Stripe.createFetchHttpClient(),
});

/** Client bound to the caller's JWT — every query runs under their RLS. */
export function userClient(req: Request): SupabaseClient {
  const authorization = req.headers.get("Authorization") ?? "";
  return createClient(SUPABASE_URL, ANON_KEY, {
    global: { headers: { Authorization: authorization } },
    auth: { persistSession: false },
  });
}

/** Service-role client — bypasses RLS. Never expose its results blindly. */
export function adminClient(): SupabaseClient {
  return createClient(SUPABASE_URL, SERVICE_ROLE_KEY, {
    auth: { persistSession: false },
  });
}

/** Resolve the authenticated user from the request, or null if unauthenticated. */
export async function getUser(req: Request) {
  const supa = userClient(req);
  const { data, error } = await supa.auth.getUser();
  if (error || !data?.user) return null;
  return data.user;
}

/**
 * Return the Stripe customer id for a user, creating the customer (and saving
 * the id on their profile) on first use. Idempotent enough for our needs.
 */
export async function ensureStripeCustomer(userId: string, email: string | undefined): Promise<string> {
  const admin = adminClient();
  const { data: profile } = await admin
    .from("profiles")
    .select("stripe_customer_id")
    .eq("id", userId)
    .single();

  if (profile?.stripe_customer_id) return profile.stripe_customer_id;

  const customer = await stripe.customers.create({
    email,
    metadata: { supabase_user_id: userId },
  });

  await admin.from("profiles").update({ stripe_customer_id: customer.id }).eq("id", userId);
  return customer.id;
}
