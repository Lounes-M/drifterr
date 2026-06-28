# Fixtures

Hand-annotated transcripts used to validate the engine **without any real
channel attached**. This is the M1 go/no-go: if detection is not convincing
here, nothing downstream matters.

Each `*.json` file is one case:

```jsonc
{
  "name": "human-readable case name",
  "baseline": { "goal": "...", "constraints": [...], "decisions": [...] },
  "conversation": { "sessionId": "...", "model": "...", "turns": [...],
                    "context": {...}, "source": "proxy|file|browser" },
  "expect": {
    "state": "green|amber|red",       // instantaneous worst state
    "triggeringSignal": "constraint|saturation|null",
    "triggeringConstraint": "c1"      // optional: id expected to fire
  }
}
```

The integration test `crates/engine/tests/fixtures.rs` loads every file here,
runs `evaluate`, and asserts the expectation. Add a file to add a case — no
test code changes needed.
