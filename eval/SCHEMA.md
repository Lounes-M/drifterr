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

## Calibrating the goal signal (`--sweep`)

The goal-alignment signal has four tunables. They used to be three hand-picked
constants, one of which — an **absolute** cosine floor (`recent < 0.5`) — was
structurally wrong: absolute cosine scale is a property of the *embedder*, not of
drift, so no single floor can be correct for both the lexical bag embedder and the
ONNX sentence model. Worse, it was ANDed onto the decline test, so a reply could
fall a long way off the goal and still be ignored for having started high. Recall
was near zero as a result.

The test is now purely about *change* — an absolute drop **and** a drop
proportional to the alignment the session had established. The proportional half is
the scale-free part, and it is what makes one set of thresholds meaningful under
either embedder.

To calibrate rather than argue:

```bash
cargo run -p drifterr-engine --example eval -- eval/ --sweep
```

That walks a grid of `min_drop` × `min_rel_drop` × `recent_window` and reports
precision / recall / F1 at each point, marking the current defaults. Case
accounting is explicit:

| polarity | which cases | expectation |
|---|---|---|
| positive | `triggeringSignal: "goal_alignment"` | the signal should fire |
| negative | `state: "green"` | the signal must **not** fire |
| ignored | any other non-green cause | goal may legitimately also be amber |

Override without a rebuild — the same variables the sweep explores:

```bash
DRIFTERR_GOAL_MIN_DROP  DRIFTERR_GOAL_MIN_REL_DROP
DRIFTERR_GOAL_RECENT_WINDOW  DRIFTERR_GOAL_MIN_TURNS
```

**A grid winner on a handful of cases is a hypothesis, not a default.** With one
positive case every grid point scores 1.00, which tells you nothing. The sweep
prints this warning itself; ship a new default only once `eval/blind/` is large
enough to confirm it out of sample.

## Adding real sessions

The synthetic cases prove the *mechanism*. The **product** is proven only on real
annotated sessions with a genuine turn-level label. There is no shortcut here and no
way to synthesise one: a corpus is the single thing in this repo that cannot be
written, only collected.

### 1. Export your own sessions

```bash
cargo run -p drifterr-store --example annotate -- \
  --db ~/.drifterr/drifterr.db --out eval/inbox
```

That writes one schema-valid case per stored session. Sessions with fewer than three
turns, or with no stated intent, are skipped — neither can illustrate drift.

**Stubs are not pre-labelled, on purpose.** Each arrives with `expect.state: "TODO"`
and the harness *refuses to load it*. It would be trivial to run the engine over each
session and write its own verdict into `expect`; it would also make the corpus
worthless, because the engine would then be graded against its own output and score
100% by construction. A human has to decide what the right answer was.

### 2. Harvest false positives, which are free ground truth

```bash
cargo run -p drifterr-store --example annotate -- \
  --feedback ~/.drifterr/feedback.jsonl --out eval/inbox
```

Every "this wasn't drift" report is a case where the truth is already known — the
user looked at an alert and said it was wrong. These come out pre-labelled `green` on
*their* authority, not the engine's, and they're the most valuable cases the project
can collect: zero hard-signal false positives is the metric the whole release gate is
built around. (The feedback record doesn't carry the turns, so paste those in from
the session before using the case.)

### 3. Annotate, then split ~70/30

Fill in the state, the cause, and the onset turn; delete the `_annotation` block;
move the file into `eval/` (development) or `eval/blind/` (holdout). Keep the ratio
around 70/30 and grow both together.

> **Privacy.** These files contain your conversations verbatim. They're written
> locally and nothing is uploaded, but they are exactly what you would be sharing if
> you contributed a corpus upstream. Read them before sharing; redact anything you
> wouldn't post publicly.

### What the numbers may be used to claim

Every run prints a **CORPUS MATURITY** block, because a percentage next to eight
self-authored cases reads exactly like a percentage backed by hundreds of real ones.
This repo has made that mistake already — the README once advertised "100% accuracy"
on a set with an empty holdout.

While the set is small or the holdout empty, the block says `NOT PUBLISHABLE` and the
numbers are for regression detection only. Once a real corpus exists, turn on the
out-of-sample requirement in CI so no figure can rest on the set it was tuned
against:

```bash
cargo run -p drifterr-engine --example eval -- eval/blind/ --gate --require-blind 30
```

## `provenance` — who decided the right answer

Optional, one of:

| Value | Meaning |
| --- | --- |
| `engine-author` | Written and graded by whoever wrote the engine. The default reading when the field is absent. |
| `third-party` | Someone other than the engine's author decided the expected label. |
| `real-session` | Drawn from a real session (via `drifterr-store --example annotate`) rather than composed to illustrate a case. |

Only `third-party` and `real-session` count toward the corpus-maturity gate, and the
statistical gates stay off until at least one exists.

That is the whole point of the field. Case *count* is gameable by exactly the person
most motivated to game it: the engine's author can write thirty more fixtures in an
afternoon and switch the gates on without having learned anything about accuracy. A
set one person both wrote and graded measures their agreement with themselves,
however large it gets.

Absent is read as `engine-author` on purpose — assume the least, not the most. A case
that forgets the field cannot quietly promote itself into evidence.
