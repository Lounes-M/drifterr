# Drifterr — FAQ

### What does Drifterr actually do?

Long AI chat sessions quietly slide away from what you first asked — the model
reintroduces an approach you rejected, ignores a constraint you set, or fills its
context window until quality drops. Drifterr measures that drift against a ground
truth **you** own (your goal + constraints), **names the cause**, and lets you
re-anchor in one click. It runs as a quiet menubar app.

### Is it really local-first? Does my chat leave my machine?

No chat content ever leaves your machine. Conversations, prompts, signals and
drift scores live in local SQLite. Model calls go through **your own provider**
with **your own key**. The only server-side component is accounts & billing
(email + plan). This isn't just a promise — it's [enforced in CI](../crates/proxy/tests/egress.rs)
and laid out at [drifterr.app/proof](https://drifterr.app/proof).

### Do I need an API key?

- **Claude Code:** no. Drifterr auto-watches your local sessions
  (`~/.claude/projects`) — zero config, no keys.
- **Other tools:** you point the tool at Drifterr's local proxy and use **your
  own** provider key. Nothing is sent to a Drifterr server. Run
  `drifterr-proxy init` and it prints the exact setup.

### Which providers work?

The proxy defaults to OpenRouter but connects directly to OpenAI, Anthropic,
Google Gemini, Groq, Mistral, DeepSeek, xAI (Grok) or Together with a single
setting (`DRIFTERR_PROVIDER=…`), or any custom endpoint (`OPENAI_UPSTREAM=…`).

### Does it slow down my requests?

No. Streaming is byte-for-byte passthrough; the proxy tees a copy and runs
detection **off** the response path, after the stream ends. Added latency ≈ 0.

### The "judge" and "auto-intent" cost money?

They're **opt-in** and **BYOK** — they call **your own** OpenRouter key, so any
cost is on your provider bill, not ours. Auto-intent is rate-limited (at most
once every few turns), skips unchanged transcripts, and is capped per session
(`DRIFTERR_JUDGE_MAX_SYNTH_PER_SESSION`, default 30). With the judge off (the
default), Drifterr makes no AI calls of its own.

### Why is the "Auto-intent" toggle greyed out?

It needs the judge, which needs your OpenRouter key. Add it in Settings → Judge,
and the toggle unlocks.

### Is it free?

Yes, free to use. Pro and Team plans add unlimited sessions, the hosted fail-safe
judge, the drift map and team features — see [pricing](https://drifterr.app/#pricing).

### macOS says Drifterr is from an "unidentified developer".

The build is currently **unsigned**. Right-click the app → **Open** → **Open**
the first time (or `xattr -dr com.apple.quarantine "/Applications/Drifterr.app"`).
See [TROUBLESHOOTING](TROUBLESHOOTING.md).

### Is it open source?

Source-available, **not** open-source — you can read and evaluate the code, but
using/redistributing it requires a license. See [LICENSE](../LICENSE).

### How do I know detection actually works?

Run the eval harness over the annotated set:
`cargo run -p drifterr-engine --example eval -- eval/`. It reports per-signal
precision/recall, the hard-signal false-positive rate (must be zero), alert
delay and the uplift over a naive baseline. Add your own annotated sessions with
the [annotation helper](../eval/SCHEMA.md).
