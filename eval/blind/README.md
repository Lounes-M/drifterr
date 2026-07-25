# Blind holdout set

Held-out annotated cases. **Never inspected while tuning** — that is the entire
point of the split, and looking at a failure here converts a holdout into a
development set permanently.

Run the release gate against it:

```bash
cargo run -p drifterr-engine --example eval -- eval/blind/ --gate
```

Once this directory has real cases, require them in CI so no accuracy figure can
rest on the set it was tuned against:

```bash
cargo run -p drifterr-engine --example eval -- eval/blind/ --gate --require-blind 30
```

## Currently empty — and that is the honest state

There are no real annotated sessions yet, here or in `../`. Every case in the dev
set is synthetic and written by the same person who wrote the engine, so it proves
the *mechanism* works and proves nothing about accuracy on real traffic.

Until this directory has cases, the eval harness prints `NOT PUBLISHABLE` and no
percentage from it belongs in the README, on the site, or in a changelog.

To fill it, see the "Adding real sessions" section of [`../SCHEMA.md`](../SCHEMA.md)
— `cargo run -p drifterr-store --example annotate` does the mechanical part.
