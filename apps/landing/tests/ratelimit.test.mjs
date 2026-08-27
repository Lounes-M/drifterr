// The public endpoints' load shedding.
//
//   node tests/ratelimit.test.mjs
//
// Small surface, but worth pinning: a limiter with an off-by-one refills too fast
// and protects nothing, or too slowly and throttles real traffic. Neither shows
// up until production.

import { bucket } from "../api/_ratelimit.js";

let failures = 0;
const check = (c, m) => (c ? console.log("  ✓ " + m) : (failures++, console.error("  ✗ " + m)));

// A burst up to capacity is normal traffic — a page fires a view plus a couple of
// clicks at once — and a limiter that punished it would be measuring nothing.
{
  const b = bucket({ capacity: 3, perSecond: 1 });
  check(b.take() && b.take() && b.take(), "a burst up to capacity passes");
  check(!b.take(), "the next one is dropped");
  check(b.drainDropped() === 1, "the drop is counted");
  check(b.drainDropped() === 0, "and the counter resets, so a log line is not repeated forever");
}

// Refill is time-based, so a steady trickle is never throttled.
{
  const b = bucket({ capacity: 2, perSecond: 1000 });
  b.take();
  b.take();
  check(!b.take(), "empty at capacity");
  await new Promise((r) => setTimeout(r, 25));
  check(b.take(), "refills over time");
}

// The bucket must not refill past capacity while idle, or a long quiet period
// would bank unlimited burst and the cap would mean nothing.
{
  const b = bucket({ capacity: 2, perSecond: 1000 });
  await new Promise((r) => setTimeout(r, 30));
  check(b.take() && b.take(), "two available after idling");
  check(!b.take(), "but never more than capacity");
}

if (failures) {
  console.error(`\nratelimit.test: ${failures} check(s) failed`);
  process.exit(1);
}
console.log("\nratelimit.test: all checks passed");
