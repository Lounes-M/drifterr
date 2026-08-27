# Drifterr — Troubleshooting

The proxy is the #1 source of friction. Most issues are one of the below.

## The panel says "proxy not reachable" / everything is offline

The panel talks to the local control server on `http://127.0.0.1:8788`.

- **Using the desktop app?** The proxy runs *inside* it. If the panel can't
  reach it, quit and reopen the app. Check nothing else is bound to ports
  **8787** (relay) or **8788** (control) — see below.
- **Running the standalone proxy?** Start it: `cargo run -p drifterr-proxy`
  (or the release binary). `drifterr-proxy init` checks reachability for you.

### Ports 8787 / 8788 already in use

Something else owns the port. Free it, or move Drifterr:

```bash
export DRIFTERR_PROXY_ADDR=127.0.0.1:8790
export DRIFTERR_CONTROL_ADDR=127.0.0.1:8791
# then point your tool at http://localhost:8790 and the app's DRIFTERR_CONTROL to :8791
```

## Claude Code sessions aren't showing up

- Drifterr watches `~/.claude/projects` automatically. Confirm it exists and has
  `.jsonl` files (start a Claude Code session first).
- Watching a different location? Set `DRIFTERR_WATCH_DIR=/path/to/projects`.
- The header shows a green **"Watching Claude Code"** chip when the channel is
  live. No chip → the directory wasn't found at launch; set `DRIFTERR_WATCH_DIR`
  and reopen.
- Sessions appear after the **first assistant reply** in a conversation.

## No drift is ever detected / no alerts

- **Declare your intent.** Drift is measured against *your* goal + constraints.
  With no intent set, there's little to measure. Set it in the panel (or turn on
  Auto-intent with your OpenRouter key).
- **Constraints must be stated to be caught.** The deterministic signal catches
  rules you actually wrote ("no JS", "TypeScript only", "no console.log",
  "no TODOs", "no `any` type", "no new dependencies", "no eval",
  "no hardcoded secrets", "don't touch package.json", a word or line limit…).
  Vague intent → fewer catches, by design (we under-claim rather than cry wolf).
- **Saturation is only exact via the proxy.** On the file/extension channels
  it's estimated; the panel marks whether it's exact.

## macOS: "Drifterr can't be opened" / blocked on first launch

The build is unsigned, so Gatekeeper blocks it:

1. Finder → Applications → **right-click Drifterr → Open → Open**, or
2. `xattr -dr com.apple.quarantine "/Applications/Drifterr.app"`

Make sure the app is in **/Applications** (not run from the DMG).

## macOS: the app icon is a grey box, or has a shadow "halo"

Both come from the unsigned/first-launch state and are fixed in current builds.
If an old install shows a grey icon, rebuild the icon cache:
`sudo rm -rf /Library/Caches/com.apple.iconservices.store && killall Dock Finder`
(or just reboot once).

## macOS: "Update failed" when I click Update

Auto-updating an **unsigned** app is unreliable on macOS (Gatekeeper resists
replacing/relaunching it). For now, download the new DMG and install it
manually. This goes away once the app is signed + notarized (see
[RELEASING.md](../RELEASING.md)). Windows and Linux auto-update fine.

## Windows: SmartScreen warns on install

Unsigned installer → click **More info → Run anyway**. Goes away with code
signing.

## Settings shows the wrong version (e.g. v0.0.1)

Fixed in current builds — the app now reports its real version via
`DRIFTERR_APP_VERSION`. If you see it, you're on an old build; update.

## The proxy relayed my request but detection didn't fire

Detection runs **after** the response stream finishes, off the response path, so
there's a brief lag after the reply completes. It never delays or alters the
reply itself. If detection still doesn't appear, check the control server is
reachable (top of this doc).

---

Still stuck? Open an issue with your OS, version (Settings → About), and what you
expected vs saw. **Security issues:** don't open a public issue — see
[SECURITY.md](../SECURITY.md).

## "This endpoint needs the local control token" / 401 from the control API

The control API is authenticated, so a page you have open cannot read your sessions
out of it. Things that talk to it get the token automatically — the panel, and the
`hook` and `mcp` subcommands, which read it from the app's state directory.

Two cases need it by hand:

**The browser extension.** Panel → **Settings → Browser extension** → copy, then
paste into the extension's popup. The popup says "Not paired yet" until you do.

**`curl` or a script.**

```bash
curl -H "X-Drifterr-Token: $(drifterr-proxy token)" http://127.0.0.1:8788/status
```

If `drifterr-proxy token` says there is no token yet, the app has not run since it
was installed — start it once. The token lives at `<state dir>/token` (mode `0600`):

| Platform | Location |
| --- | --- |
| macOS | `~/Library/Application Support/com.drifterr.app/token` |
| Linux | `~/.local/share/com.drifterr.app/token` |
| Windows | `%APPDATA%\com.drifterr.app\token` |

Deleting that file rotates the token: the app mints a new one on next start, and the
extension needs re-pairing.

## A rule from my CLAUDE.md shows as "Proposed" and only warns

That is deliberate. Drifterr read the rule out of a file you wrote for your agent,
not out of something you told Drifterr — so it checks the rule and names it, but
caps at amber until you confirm. Press **Enforce** on the constraint in the panel and
it behaves like a rule you typed.

The alternative is worse: an importer reading English will eventually misread a
sentence, and when it does, the cost should be a proposal you glance at rather than a
red alert on a rule nobody wrote.
