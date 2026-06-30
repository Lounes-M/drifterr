// POST /functions/v1/stripe-checkout
//
// Body: { plan: "pro" | "team", interval: "month" | "year", quantity?: number }
// Auth: Bearer <supabase access token> (required).
//
// Creates a Stripe Checkout Session for the caller and returns its URL. The
// caller's identity comes from their JWT, never from the body — you can only
// buy a plan for yourself. The actual plan grant happens later, in the webhook,
// once Stripe confirms payment.

import { preflight, json } from "../_shared/cors.ts";
import { getUser, ensureStripeCustomer, stripe } from "../_shared/clients.ts";

const SITE_URL = Deno.env.get("SITE_URL") ?? "https://drifterr.app";

// Map (plan, interval) → the Stripe Price env var. Keeps price ids out of code.
const PRICE_ENV: Record<string, Record<string, string>> = {
  pro: { month: "STRIPE_PRICE_PRO_MONTHLY", year: "STRIPE_PRICE_PRO_YEARLY" },
  team: { month: "STRIPE_PRICE_TEAM_MONTHLY", year: "STRIPE_PRICE_TEAM_YEARLY" },
};

Deno.serve(async (req) => {
  const pre = preflight(req);
  if (pre) return pre;
  const origin = req.headers.get("origin");

  if (req.method !== "POST") return json({ error: "method not allowed" }, 405, origin);

  const user = await getUser(req);
  if (!user) return json({ error: "unauthorized" }, 401, origin);

  let body: { plan?: string; interval?: string; quantity?: number };
  try {
    body = await req.json();
  } catch {
    return json({ error: "invalid JSON body" }, 400, origin);
  }

  const plan = body.plan ?? "";
  const interval = body.interval ?? "month";
  const quantity = Math.max(1, Math.min(500, Number(body.quantity) || 1));

  const priceEnv = PRICE_ENV[plan]?.[interval];
  if (!priceEnv) return json({ error: "unknown plan or interval" }, 400, origin);
  const priceId = Deno.env.get(priceEnv);
  if (!priceId) return json({ error: `price not configured (${priceEnv})` }, 500, origin);

  try {
    const customer = await ensureStripeCustomer(user.id, user.email);

    const session = await stripe.checkout.sessions.create({
      mode: "subscription",
      customer,
      line_items: [{ price: priceId, quantity: plan === "team" ? quantity : 1 }],
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
