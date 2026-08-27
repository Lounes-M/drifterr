// The webhook's behaviour under Stripe's real delivery guarantees.
//
//   deno test --allow-env supabase/functions/
//
// These are not schema tests. Each one drives a sequence Stripe actually
// produces — a redelivery, an out-of-order pair, a handler that throws — against
// a fake ledger, and asserts the customer ends up on the right plan. The two bugs
// this code was written to fix both looked fine on inspection and only appeared
// under exactly these sequences, which is why reading the handler was never
// enough.

import { assertEquals } from "jsr:@std/assert@1";
import {
  type ClaimOutcome,
  handleEvent,
  type Ledger,
  type SubscriptionRow,
} from "./subscription_sync.ts";

// --- a fake ledger ----------------------------------------------------------
//
// Small enough to read in one go, which is the point: if the fake needed its own
// tests, the interface would be wrong.

interface Fake extends Ledger {
  rows: Map<string, SubscriptionRow>;
  claims: Set<string>;
  upserts: number;
}

function fakeLedger(opts: {
  prices?: Record<string, string>;
  customers?: Record<string, string>;
  subscriptions?: Record<string, unknown>;
  onUpsert?: () => void;
} = {}): Fake {
  const rows = new Map<string, SubscriptionRow>();
  const claims = new Set<string>();
  const f: Fake = {
    rows,
    claims,
    upserts: 0,

    claimEvent(id): Promise<ClaimOutcome> {
      if (claims.has(id)) return Promise.resolve("duplicate");
      claims.add(id);
      return Promise.resolve("claimed");
    },
    releaseEvent(id) {
      claims.delete(id);
      return Promise.resolve();
    },
    lastEventAt(userId) {
      const row = rows.get(userId);
      return Promise.resolve(row ? row.last_event_at : null);
    },
    upsertSubscription(row) {
      opts.onUpsert?.();
      f.upserts++;
      rows.set(row.user_id, row);
      return Promise.resolve();
    },
    planForPrice(priceId) {
      return Promise.resolve(priceId ? (opts.prices?.[priceId] ?? null) : null);
    },
    userForCustomer(customerId) {
      return Promise.resolve(opts.customers?.[customerId] ?? null);
    },
    // deno-lint-ignore no-explicit-any
    fetchSubscription(id): Promise<any> {
      const sub = opts.subscriptions?.[id];
      if (!sub) return Promise.reject(new Error(`no such subscription ${id}`));
      // deno-lint-ignore no-explicit-any
      return Promise.resolve(sub as any);
    },
  };
  return f;
}

// --- fixtures ---------------------------------------------------------------

function subscription(opts: {
  id?: string;
  user?: string;
  customer?: string;
  price?: string;
  status?: string;
  quantity?: number;
  interval?: string;
  // deno-lint-ignore no-explicit-any
} = {}): any {
  return {
    id: opts.id ?? "sub_1",
    status: opts.status ?? "active",
    customer: opts.customer ?? "cus_1",
    metadata: opts.user === undefined ? { supabase_user_id: "user-1" } : { supabase_user_id: opts.user },
    cancel_at_period_end: false,
    current_period_end: 1_800_000_000,
    items: {
      data: [{
        quantity: opts.quantity ?? 1,
        price: { id: opts.price ?? "price_pro_m", recurring: { interval: opts.interval ?? "month" } },
      }],
    },
  };
}

// deno-lint-ignore no-explicit-any
function event(type: string, created: number, object: unknown, id = "evt_1"): any {
  return { id, type, created, data: { object } };
}

const PRICES = { price_pro_m: "pro", price_team_m: "team" };

// --- 1. the ordering guard --------------------------------------------------

Deno.test("an out-of-order event never downgrades a customer", async () => {
  const ledger = fakeLedger({ prices: PRICES });

  // The upgrade lands first: Free → Team at t=200.
  await handleEvent(
    event("customer.subscription.updated", 200, subscription({ price: "price_team_m" })),
    ledger,
  );
  assertEquals(ledger.rows.get("user-1")?.plan_id, "team");

  // Then the OLDER event for the plan they upgraded away from arrives late.
  // Stripe does this; last-write-wins would silently undo the purchase.
  const result = await handleEvent(
    event("customer.subscription.updated", 100, subscription({ price: "price_pro_m" }), "evt_2"),
    ledger,
  );

  assertEquals(result, { kind: "skipped", reason: "stale" });
  assertEquals(
    ledger.rows.get("user-1")?.plan_id,
    "team",
    "a stale event must not move the customer off the plan they bought",
  );
});

Deno.test("an event at the same timestamp still applies", async () => {
  // `>` not `>=`: two events can share a second, and refusing the second would
  // drop a real change rather than an out-of-order one.
  const ledger = fakeLedger({ prices: PRICES });
  await handleEvent(event("customer.subscription.updated", 500, subscription()), ledger);
  const result = await handleEvent(
    event("customer.subscription.updated", 500, subscription({ price: "price_team_m" }), "evt_2"),
    ledger,
  );
  assertEquals(result.kind, "applied");
  assertEquals(ledger.rows.get("user-1")?.plan_id, "team");
});

// --- 2. idempotency ---------------------------------------------------------

Deno.test("a redelivered event is claimed once and applied once", async () => {
  const ledger = fakeLedger({ prices: PRICES });
  const e = event("customer.subscription.updated", 300, subscription());

  assertEquals(await ledger.claimEvent(e.id, e.type, e.created), "claimed");
  await handleEvent(e, ledger);
  assertEquals(ledger.upserts, 1);

  // Stripe retries the identical event. The claim is what stops it.
  assertEquals(
    await ledger.claimEvent(e.id, e.type, e.created),
    "duplicate",
    "a redelivery must lose the claim race",
  );
  assertEquals(ledger.upserts, 1, "and must not apply the change a second time");
});

Deno.test("a failed handler releases its claim so the retry can retry", async () => {
  let calls = 0;
  const ledger = fakeLedger({
    prices: PRICES,
    onUpsert: () => {
      // Fail the first attempt the way a transient database error would.
      if (++calls === 1) throw new Error("connection reset");
    },
  });
  const e = event("customer.subscription.updated", 400, subscription());

  assertEquals(await ledger.claimEvent(e.id, e.type, e.created), "claimed");
  let threw = false;
  try {
    await handleEvent(e, ledger);
  } catch {
    threw = true;
    await ledger.releaseEvent(e.id);
  }
  assertEquals(threw, true);

  // Without the release, the retry would see a duplicate and drop the change
  // permanently — one transient failure becoming a lost subscription.
  assertEquals(await ledger.claimEvent(e.id, e.type, e.created), "claimed");
  await handleEvent(e, ledger);
  assertEquals(ledger.rows.get("user-1")?.plan_id, "pro");
});

// --- 3. what each event type means ------------------------------------------

Deno.test("cancellation moves the customer to free, not to a stale plan", async () => {
  const ledger = fakeLedger({ prices: PRICES });
  await handleEvent(event("customer.subscription.updated", 100, subscription()), ledger);
  await handleEvent(
    event(
      "customer.subscription.deleted",
      200,
      subscription({ status: "canceled" }),
      "evt_2",
    ),
    ledger,
  );
  const row = ledger.rows.get("user-1")!;
  assertEquals(row.plan_id, "free");
  assertEquals(row.status, "free");
});

Deno.test("a declined card marks past_due and keeps the plan", async () => {
  // Stripe keeps retrying and the customer keeps their plan through dunning.
  // The point of handling this at all is that the account page can say why.
  const ledger = fakeLedger({
    prices: PRICES,
    subscriptions: { sub_1: subscription({ status: "past_due" }) },
  });
  await handleEvent(event("customer.subscription.updated", 100, subscription()), ledger);

  const result = await handleEvent(
    event("invoice.payment_failed", 200, { subscription: "sub_1" }, "evt_2"),
    ledger,
  );

  assertEquals(result.kind, "applied");
  const row = ledger.rows.get("user-1")!;
  assertEquals(row.status, "past_due");
  assertEquals(row.plan_id, "pro", "a declined card must not strip the plan mid-dunning");
});

Deno.test("checkout completion applies the plan immediately", async () => {
  const ledger = fakeLedger({
    prices: PRICES,
    subscriptions: { sub_9: subscription({ id: "sub_9", price: "price_team_m", quantity: 5 }) },
  });
  const result = await handleEvent(
    event("checkout.session.completed", 100, { subscription: "sub_9" }),
    ledger,
  );
  assertEquals(result.kind, "applied");
  const row = ledger.rows.get("user-1")!;
  assertEquals(row.plan_id, "team");
  assertEquals(row.seats, 5);
});

// --- 4. the edges that must not grant anything ------------------------------

Deno.test("an unknown price never grants a paid plan", async () => {
  // A price that is not in our catalogue means we do not know what was bought.
  // Free is the only safe answer; guessing would grant a plan nobody paid for.
  const ledger = fakeLedger({ prices: PRICES });
  await handleEvent(
    event("customer.subscription.updated", 100, subscription({ price: "price_unknown" })),
    ledger,
  );
  assertEquals(ledger.rows.get("user-1")?.plan_id, "free");
});

Deno.test("an event we cannot attribute to a user changes nothing", async () => {
  const ledger = fakeLedger({ prices: PRICES });
  const orphan = subscription({ user: undefined, customer: "cus_unknown" });
  orphan.metadata = {};
  const result = await handleEvent(
    event("customer.subscription.updated", 100, orphan),
    ledger,
  );
  assertEquals(result, { kind: "skipped", reason: "no-user" });
  assertEquals(ledger.upserts, 0);
});

Deno.test("the customer id is a fallback when metadata is missing", async () => {
  const ledger = fakeLedger({ prices: PRICES, customers: { cus_7: "user-7" } });
  const sub = subscription({ customer: "cus_7" });
  sub.metadata = {};
  await handleEvent(event("customer.subscription.updated", 100, sub), ledger);
  assertEquals(ledger.rows.get("user-7")?.plan_id, "pro");
});

Deno.test("event types we do not handle are ignored, not applied", async () => {
  const ledger = fakeLedger({ prices: PRICES });
  const result = await handleEvent(
    event("customer.created", 100, { id: "cus_1" }),
    ledger,
  );
  assertEquals(result, { kind: "skipped", reason: "ignored-type" });
  assertEquals(ledger.upserts, 0);
});
