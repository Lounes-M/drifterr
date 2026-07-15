# Drifterr eval — annotation schema (v1)

This directory is Drifterr's **detection validation set**: annotated conversations
the engine is run against so we can *measure* detection quality instead of
asserting it. The harness that consumes these files is
`crates/engine/examples/eval.rs` (`cargo run -p drifterr-engine --example eval`).

The go/no-go metric is **zero hard-signal false positives** — see the RELEASE
GATE section the harness prints, and run it with `--gate` in CI to enforce it.
The gate's numbers live in one editable place, [`thresholds.conf`](thresholds.conf)
— see [Release-gate thresholds](#release-gate-thresholds) below.

## File format

One JSON file per case. Schema version: **1**.

```jsonc
{
  "name": "TS-only constraint violated (assistant creates .js)",
  "baseline": {
    "goal": "Write the auth module in strict TypeScript",
    "constraints": [
      { "id": "c1", "text": "TypeScript only, no JS", "kind": "Tech",
        "checkable": "Deterministic", "active": true }
    ],
    "decisions": []
  },
  "conversation": {
    "sessionId": "e5",
    "model": "claude-opus-4",
    "source": "proxy",              // "proxy" ⇒ exact tokens; "file"/"extension" ⇒ estimated
    "turns": [
      { "role": "user",      "content": "Write the auth module in strict TypeScript, no JS" },
      { "role": "assistant", "content": "Sure — I'll start by scaffolding auth.js ..." }
    ],
    "context": { "used": 1200, "window": 200000, "exact": true }
  },
  "expect": {
    "state": "red",                 // green | amber | red  (the committed state)
    "triggeringSignal": "constraint", // constraint | saturation | goal_alignment |
                                      // decision_coherence | degradation  (omit if state=green)
    "triggeringConstraint": "c1",   // optional: which constraint (evidence check)
    "turn": 1                       // optional but recommended: 0-based turn where the
                                    //   drift the engine should catch begins. Powers the
                                    //   alert-delay metric (predicted-turn − this).
  }
}
```

### Field notes

- **`baseline`** — the user's stated intent for the session: `goal`, `constraints`,
  and explicitly-rejected `decisions`. This is what drift is measured *against*.
  Mirror the `Baseline` type in `crates/engine/src/baseline.rs`.
- **`conversation.source`** — must be `"proxy"` for a case that asserts exact
  saturation; `ContextState.exact` is only ever true on the proxy channel.
- **`expect.triggeringSignal`** — the *cause the engine should name*. The judge
  signals (`decision_coherence`, fuzzy constraints) run against a live model in
  the proxy, not the pure engine, so a case expecting one is reported `skipped`
  by this harness and validated separately.
- **`expect.turn`** — annotate the onset turn wherever you can; it's the only way
  to measure whether we warn at the *right moment* (too late = useless, too early
  = noise).

## Train / blind split

To avoid over-fitting the engine to the cases we look at:

- **`eval/`** (this directory) — the **development** set. Look at it, tune against it.
- **`eval/blind/`** — the **held-out** set. **Never** inspect individual failures
  while tuning. Run the gate against it before a release:

  ```bash
  cargo run -p drifterr-engine --example eval -- eval/blind/ --gate
  ```

  A green gate here — zero hard-signal false positives on cases you didn't tune
  on — is the release signal. Keep the split ratio ~70/30 (dev/blind) and grow
  both as real annotated sessions come in.

## Release-gate thresholds

The `--gate` numbers are **not** hard-coded in the harness. They live in
[`thresholds.conf`](thresholds.conf) — a flat `key = number` file (`#` comments
and blanks ignored) that `eval.rs` loads at startup, falling back to built-in
defaults for anything absent or unparseable. Edit that file to retune; no
recompile of the metric logic is needed.

The harness prints every gated metric next to its threshold and a verdict, in
**two enforcement tiers plus one claim gate**:

- **Non-negotiables** — always block the release, valid on *any* set size.
  Hard-signal false positives, false REDs, and premature hard alerts are fixed
  at **0** (a hard signal that cries wolf is the one unforgivable failure, per
  `CLAUDE.md`). The hard-signal **median alert delay** must be
  `≤ hard.max_median_delay` (default `0` — a deterministic rule is caught on the
  turn it happens).
- **Statistical** — soft-signal precision (`soft.min_precision`), soft median
  delay (`soft.max_median_delay`), and uplift over the naive saturation-only
  baseline (`baseline.min_uplift_pts`). These auto-become hard gates once the
  relevant case count reaches `min_cases` (default `30`); below that they're
  reported `n/a (need N cases)` so CI never fails for lack of data.
- **Claim gate** — `goal.min_recall` gates only the *"semantic goal detection
  works"* claim/label, **never** the release. Below it (or below `min_cases`
  goal cases), goal alignment stays labeled best-effort in the UI + README.

| key | default | tier | meaning |
|---|---|---|---|
| `min_cases` | 30 | — | case count at which statistical gates turn on |
| `hard.max_median_delay` | 0 | non-negotiable | max median turns late for a hard signal |
| `soft.min_precision` | 0.60 | statistical | min precision of soft (AMBER) predictions |
| `soft.max_median_delay` | 2 | statistical | max median turns late for a soft signal |
| `baseline.min_uplift_pts` | 10 | statistical | min state-accuracy points over naive baseline |
| `goal.min_recall` | 0.70 | claim only | recall to earn the "semantic goal" label |

## Adding real sessions

The synthetic `e1..e8` cases prove the mechanism. The *product* is proven only on
**real** annotated sessions (coding, writing, analysis) with a genuine
turn-level drift label. Capture your own Claude Code sessions and beta-tester
transcripts, annotate the onset turn and the true cause, and drop them here
(dev) or in `blind/` (holdout). The harness turns them straight into numbers.
