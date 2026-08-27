// The public endpoints' load shedding.
//
//   node tests/ratelimit.test.mjs
//
// Small surface, worth pinning: a limiter with an off-by-one refills too fast and
// protects nothing, or too slowly and throttles real traffic. Neither shows up
// until production.
//
// Time is injected rather than slept through. The first version of this file used
// real `setTimeout` delays and then asserted on the token count — which made every
// assertion a race between the refill rate and how fast the machine got through
// three lines. It passed locally and failed on a slower CI runner, which is a
// flaky test in the file whose entire job is to behave predictably under load. A
// fake clock removes the race rather than widening the margin.

import { bucket } from "../api/_ratelimit.js";

let failures = 0;
const check = (c, m) => (c ? console.log("  ✓ " + m) : (failures++, console.error("  ✗ " + m)));

/** A clock the test moves by hand. */
function clock(start = 1_000_000) {
  let t = start;
  return { now: () => t, advance: (ms) => (t += ms) };
}

// A burst up to capacity is normal traffic — a page fires a view plus a couple of
// clicks at once — and a limiter that punished it would be measuring nothing.
{
  const c = clock();
  const b = bucket({ capacity: 3, perSecond: 1, now: c.now });
  check(b.take() && b.take() && b.take(), "a burst up to capacity passes");
  check(!b.take(), "the next one is dropped");
  check(b.drainDropped() === 1, "the drop is counted");
  check(b.drainDropped() === 0, "and the counter resets, so a log line is not repeated forever");
}

// Refill is time-based: a steady trickle within the rate is never throttled.
{
  const c = clock();
  const b = bucket({ capacity: 2, perSecond: 10, now: c.now }); // one token / 100ms
  b.take();
  b.take();
  check(!b.take(), "empty at capacity");
  c.advance(100);
  check(b.take(), "one token back after exactly its refill interval");
  check(!b.take(), "and only one — refill is a rate, not a reset");
}

// Idling must not bank unlimited burst, or a quiet period would let a flood
// through the moment traffic resumes and the cap would mean nothing.
{
  const c = clock();
  const b = bucket({ capacity: 2, perSecond: 10, now: c.now });
  b.take();
  b.take();
  c.advance(60_000); // ten minutes' worth of refill — 600 tokens, uncapped
  check(b.take() && b.take(), "refills up to capacity after a long idle");
  check(!b.take(), "but never banks more than capacity");
}

// A partial interval is not a free token: fractional accrual must not round up.
{
  const c = clock();
  const b = bucket({ capacity: 1, perSecond: 1, now: c.now }); // one token / second
  check(b.take(), "the first token is available");
  c.advance(999);
  check(!b.take(), "999ms of a 1s interval is not a token");
  c.advance(1);
  check(b.take(), "the full interval is");
}

if (failures) {
  console.error(`\nratelimit.test: ${failures} check(s) failed`);
  process.exit(1);
}
console.log("\nratelimit.test: all checks passed");
