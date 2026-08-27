// POST /functions/v1/stripe-checkout
//
// Body: { plan: "pro" | "team", interval: "month" | "year", quantity?: number }
// Auth: Bearer <supabase access token> (required).
//
// Creates a Stripe Checkout Session for the caller and returns its URL. The
// caller's identity comes from their JWT, never from the body — you can only
// buy a plan for yourself. The actual plan grant happens later, in the webhook,
// once Stripe confirms payment.

import { json, preflight } from "../_shared/cors.ts";
import { allow } from "../_shared/rate_limit.ts";
import {
  adminClient,
  ensureStripeCustomer,
  getUser,
  stripe,
  userClient,
} from "../_shared/clients.ts";

const SITE_URL = Deno.env.get("SITE_URL") ?? "https://drifterr.app";

Deno.serve(async (req) => {
  const pre = preflight(req);
  if (pre) return pre;
  const origin = req.headers.get("origin");

  if (req.method !== "POST") {
    return json({ error: "method not allowed" }, 405, origin);
  }

  const user = await getUser(req);
  if (!user) return json({ error: "unauthorized" }, 401, origin);

  // Authenticated is not the same as unlimited: every call below reaches Stripe,
  // against our account and its rate limit, at no cost to the caller.
  if (!await allow(adminClient(), user.id, "checkout")) {
    return json(
      { error: "too many requests — please wait a moment and try again" },
      429,
      origin,
    );
  }

  let body: { plan?: string; interval?: string; quantity?: number };
  try {
    body = await req.json();
  } catch {
    return json({ error: "invalid JSON body" }, 400, origin);
  }

  const plan = body.plan ?? "";
  const interval = body.interval === "year" ? "year" : "month";
  const quantity = Math.max(1, Math.min(500, Number(body.quantity) || 1));

  if (plan !== "pro" && plan !== "team") {
    return json({ error: "unknown plan" }, 400, origin);
  }

  // Resolve the Stripe price from the public catalog (plans are RLS-readable),
  // so the price ids live in the database, not in function secrets.
  const { data: planRow } = await userClient(req)
    .from("plans")
    .select("stripe_price_monthly, stripe_price_yearly")
    .eq("id", plan)
    .single();
  const priceId = interval === "year"
    ? planRow?.stripe_price_yearly
    : planRow?.stripe_price_monthly;
  if (!priceId) {
    return json({ error: "price not configured for this plan" }, 500, origin);
  }

  try {
    const customer = await ensureStripeCustomer(user.id, user.email);

    const session = await stripe.checkout.sessions.create({
      mode: "subscription",
      customer,
      line_items: [{
        price: priceId,
        quantity: plan === "team" ? quantity : 1,
      }],
      allow_promotion_codes: true,
      client_reference_id: user.id,
      subscription_data: { metadata: { supabase_user_id: user.id, plan } },
      success_url: `${SITE_URL}/account?checkout=success`,
      cancel_url: `${SITE_URL}/account?checkout=cancelled`,
    });

    return json({ url: session.url }, 200, origin);
  } catch (e) {
    console.error("checkout error", e);
    return json({ error: "could not start checkout" }, 500, origin);
  }
});
