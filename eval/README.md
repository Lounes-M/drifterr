# eval/ — detection-quality set

A larger, **harder** annotated set than `fixtures/`. Where `fixtures/` is the
go/no-go gate (the engine must get every one right, enforced by
`crates/engine/tests/fixtures.rs`), this directory is for **measurement**: cases
the engine may get wrong, so we can quantify precision/recall and watch it move
as the engine improves.

Run:

```bash
cargo run -p drifterr-engine --example eval -- eval/
```

You get a state confusion matrix and per-signal precision / recall / F1.

## Current set (8 cases)

| case | expects | exercises |
|---|---|---|
| e1 | amber · goal_alignment | semantic goal drift — **needs ONNX** (lexical misses it) |
| e2 | green | on-goal paraphrase (false-positive guard) |
| e3 | green | TS-only honored (`.ts`, no `.js`) |
| e4 | *(skipped)* | reintroduced rejected decision — judge signal |
| e5 | red · constraint | no-JS constraint violated (`auth.js`) |
| e6 | red · saturation | context ~95% full |
| e7 | red · constraint | word-limit rule **inferred** from "under 30 words" |
| e8 | green | FR "pas de commentaires" honored (FR + no false positive) |

Deterministic accuracy is 6/7 today; the one miss is e1 (semantic drift), which
the lexical embedder can't separate — it flips to caught with the ONNX embedder
(`--features onnx`, `DRIFTERR_EMBED_MODEL` set).

## Annotating

Each file is one case, same shape as `fixtures/`:

```json
{
  "name": "short human description",
  "baseline": { "goal": "...", "constraints": [...], "decisions": [...] },
  "conversation": { "sessionId": "...", "model": "...", "turns": [...],
                    "context": { "windowSize": 200000, "usedTokens": 0,
                                 "exact": true, "toolCallCount": 0 },
                    "source": "proxy" },
  "expect": { "state": "green|amber|red",
              "triggeringSignal": "constraint|saturation|goal_alignment|degradation|null",
              "triggeringConstraint": "c1 (optional)" }
}
```

Annotate with the **human-true** label — what *should* happen — never what the
engine currently outputs. A case the engine gets wrong is the point: it's what
the number is measuring.

## Scope

The harness measures the pure-engine signals (constraint, saturation, goal
alignment, degradation). Cases whose expected cause is a **judge** signal
(decision coherence, fuzzy constraints) are reported as `skipped` — those run in
the proxy against a live model and are evaluated separately.

## The real win

This seed set is synthetic-but-harder. The score jumps in value the moment you
drop **real, messy sessions** in here (scrub anything sensitive first — these
files live in the repo). That is what turns "well-built" into "measured".
