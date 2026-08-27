// POST /functions/v1/stripe-webhook
//
// Stripe → us. This is the ONLY place a paid plan is granted. We verify the
// signature, claim the event, then hand it to the decision logic in
// `_shared/subscription_sync.ts`. Deploy with `verify_jwt = false` (Stripe does
// not send a Supabase JWT) — see supabase/config.toml. Signature verification is
// what authenticates the request instead.
//
// # Delivery is at-least-once and unordered
//
// Stripe retries on any non-2xx and on a timeout, and makes no ordering promise.
// Both failures land directly on a paying customer: a retry would re-apply the
// event, and an out-of-order `customer.subscription.updated` would put someone
// back on the plan they had just upgraded from, with nothing explaining why.
//
// So: claim the event id before any work (the primary key is the lock), and
// refuse anything older than what the row already reflects. The decisions live
// next door behind an interface so they can be driven by a test rather than only
// read — see `_shared/subscription_sync_test.ts`, which replays those exact
// sequences.

import Stripe from "npm:stripe@17.5.0";
import { adminClient, stripe } from "../_shared/clients.ts";
import {
  type ClaimOutcome,
  handleEvent,
  type Ledger,
  type SubscriptionRow,
} from "../_shared/subscription_sync.ts";

const WEBHOOK_SECRET = Deno.env.get("STRIPE_WEBHOOK_SECRET")!;

/** The production ledger: Postgres via the service role, plus Stripe for lookups. */
function postgresLedger(): Ledger {
  const admin = adminClient();
  return {
    async claimEvent(id, type, eventAt): Promise<ClaimOutcome> {
      const { error } = await admin
        .from("stripe_events")
        .insert({ id, type, event_at: eventAt });
      if (!error) return "claimed";
      // 23505 = unique_violation: somebody already handled this event. A
      // concurrent redelivery loses this race rather than double-applying.
      if (error.code === "23505") return "duplicate";
      console.error("webhook: could not claim event", error);
      return "error";
    },

    async releaseEvent(id) {
      await admin.from("stripe_events").delete().eq("id", id);
    },

    async lastEventAt(userId) {
      const { data } = await admin
        .from("subscriptions")
        .select("last_event_at")
        .eq("user_id", userId)
        .maybeSingle();
      if (!data) return null;
      return Number(data.last_event_at ?? 0);
    },

    async upsertSubscription(row: SubscriptionRow) {
      await admin.from("subscriptions").upsert(row, { onConflict: "user_id" });
    },

    async planForPrice(priceId) {
      if (!priceId) return null;
      // Two `.eq()` filters rather than an interpolated `.or()` string: the price
      // id is signature-verified so it is not attacker-controlled, but building a
      // filter expression by concatenation is the wrong shape to leave in a
      // billing path, and PostgREST's `or` syntax has escaping rules nothing here
      // was applying.
      for (const column of ["stripe_price_monthly", "stripe_price_yearly"]) {
        const { data } = await admin.from("plans").select("id").eq(column, priceId).maybeSingle();
        if (data?.id) return data.id as string;
      }
      return null;
    },

    async userForCustomer(customerId) {
      const { data } = await admin
        .from("profiles")
        .select("id")
        .eq("stripe_customer_id", customerId)
        .maybeSingle();
      return (data?.id as string | undefined) ?? null;
    },

    async fetchSubscription(id) {
      return await stripe.subscriptions.retrieve(id);
    },
  };
}

Deno.serve(async (req) => {
  if (req.method !== "POST") return new Response("method not allowed", { status: 405 });

  const signature = req.headers.get("stripe-signature");
  if (!signature) return new Response("missing signature", { status: 400 });

  const raw = await req.text();
  let event: Stripe.Event;
  try {
    event = await stripe.webhooks.constructEventAsync(raw, signature, WEBHOOK_SECRET);
  } catch (e) {
    console.error("webhook signature verification failed", e);
    return new Response("bad signature", { status: 400 });
  }

  const ledger = postgresLedger();

  // Claim before doing any work. Claiming *afterwards* would leave a crash
  // mid-handler looking like a completed event, and a subscription change
  // dropped on the floor is worse than one applied twice.
  const claim = await ledger.claimEvent(event.id, event.type, event.created);
  if (claim === "duplicate") {
    // Answer 200 so Stripe stops retrying something we already did.
    return new Response(JSON.stringify({ received: true, duplicate: true }), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    });
  }
  if (claim === "error") {
    // We could not record the claim, so we must not act on the event either —
    // 500 asks Stripe to send it again.
    return new Response("could not record event", { status: 500 });
  }

  try {
    const result = await handleEvent(event, ledger);
    if (result.kind === "skipped") {
      console.log(`webhook: ${event.type} skipped (${result.reason})`);
    }
  } catch (e) {
    console.error("webhook handler error", e);
    // Release the claim so the retry can actually retry. Leaving it would turn
    // one transient failure into a permanently dropped subscription change.
    await ledger.releaseEvent(event.id);
    return new Response("handler error", { status: 500 });
  }

  return new Response(JSON.stringify({ received: true }), {
    status: 200,
    headers: { "Content-Type": "application/json" },
  });
});
