# Rule packs

A **pack** is a portable set of constraints in the user's own words. It is the one
Drifterr artefact that composes over time: rules you have settled on, in a file you
own, that move between projects, get shared with a teammate, and survive changing
tools.

The three packs here are the ones Drifterr ships with, exported so they can be read,
diffed and forked without running the app. See
[`crates/engine/src/pack.rs`](../crates/engine/src/pack.rs) for the format's rationale.

| Pack | What it stops |
| --- | --- |
| [`typescript-strict`](typescript-strict.json) | An agent loosening types or leaving debug code behind |
| [`tight-scope`](tight-scope.json) | An agent widening the blast radius of a small change |
| [`security-basics`](security-basics.json) | Hardcoded secrets and `eval` |

## Using one

```bash
# In CI, over a diff — no install needed
git diff origin/main... | drifterr-proxy check --pack tight-scope

# From a file you wrote or forked
drifterr-proxy check --pack-file packs/tight-scope.json --input patch.diff
```

In the app: **Packs** in the panel applies one to the current session, and can splice
it into the project's `CLAUDE.md` / `AGENTS.md` / `.cursor/rules` so the *agent* is
told the rules too — which prevents more violations than watching for them does.

## Writing one

```json
{
  "drifterrPack": 1,
  "name": "My rules",
  "description": "Optional, for humans.",
  "rules": [
    { "id": "no-any", "text": "Never use `any` types", "why": "it defeats the type checker" }
  ]
}
```

Rules are stored as **natural language, never compiled regexes**. The check is
re-derived on load, which means a pack stays reviewable, keeps improving as inference
improves, and round-trips through a rules file without loss.

Two rules to respect if you contribute a pack here:

1. **Every rule must be one the engine can actually check.** Run
   `drifterr-proxy check --pack-file yours.json --input /dev/null` — anything reported
   as SKIPPED is advisory, and a curated pack full of unenforceable aspirations is
   worse than no pack. (Advisory rules are fine in *your own* pack; they still show up
   in the anchor and in a re-anchor. They just don't belong in a shipped one.)
2. **Keep it reviewable.** Hard cap is 200 rules; good packs have under a dozen.

## Licence

This directory is **CC BY 4.0** — use, share and adapt freely, including
commercially, with attribution. See [`LICENSE`](LICENSE). The engine that reads these
files is not; see the repository [`LICENSE`](../LICENSE).
