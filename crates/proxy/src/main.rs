//! Drifterr proxy binary.
//!
//! Point your tool at the proxy port and read state from the control port:
//!
//! ```text
//! OPENAI_BASE_URL=http://localhost:8787/v1   # OpenAI-style tools
//! ANTHROPIC_BASE_URL=http://localhost:8787   # Anthropic-style tools
//! curl http://localhost:8788/status          # live drift status (JSON)
//! ```
//!
//! Configuration via environment (a `.env` file in the working directory is
//! auto-loaded; see `.env.example`):
//! * `DRIFTERR_PROXY_ADDR`     (default `127.0.0.1:8787`)
//! * `DRIFTERR_CONTROL_ADDR`   (default `127.0.0.1:8788`)
//! * `DRIFTERR_DB`             SQLite path (default: in-memory, not persisted)
//! * `DRIFTERR_PROVIDER`       preset (default openrouter): openai, anthropic, gemini, groq, mistral, deepseek, xai, together
//! * `OPENAI_UPSTREAM`         explicit OpenAI-schema URL (overrides the preset)
//! * `ANTHROPIC_UPSTREAM`      (default `https://api.anthropic.com`)

use drifterr_proxy::{serve, AppState, ProxyConfig};
use drifterr_store::Store;
use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    // Load a local .env if present (no-op otherwise), so config persists.
    dotenvy::dotenv().ok();

    // `drifterr-proxy init` — one-command setup: detect the tool + provider,
    // print tailored config, and check whether the proxy is reachable.
    if std::env::args().nth(1).as_deref() == Some("init") {
        run_init().await;
        return;
    }

    // `drifterr-proxy hook` — the Claude Code UserPromptSubmit hook. Runs in the path
    // of a keypress, so it prints at most a small JSON object and always exits 0.
    if std::env::args().nth(1).as_deref() == Some("hook") {
        run_hook().await;
        return;
    }

    // `drifterr-proxy check` — CI mode. Reads agent output on stdin (or a file) and exits
    // non-zero on a constraint violation.
    if std::env::args().nth(1).as_deref() == Some("check") {
        std::process::exit(run_check());
    }

    let proxy_addr: SocketAddr = env_or("DRIFTERR_PROXY_ADDR", "127.0.0.1:8787")
        .parse()
        .expect("invalid DRIFTERR_PROXY_ADDR");
    let control_addr: SocketAddr = env_or("DRIFTERR_CONTROL_ADDR", "127.0.0.1:8788")
        .parse()
        .expect("invalid DRIFTERR_CONTROL_ADDR");

    // Select the upstream provider from the environment (DRIFTERR_PROVIDER
    // preset, or explicit OPENAI_UPSTREAM / ANTHROPIC_UPSTREAM URLs).
    let cfg = ProxyConfig::from_env();

    let store = match std::env::var("DRIFTERR_DB") {
        Ok(path) if !path.is_empty() => match Store::open(&path) {
            Ok(s) => {
                eprintln!("drifterr: persisting to {path}");
                Some(s)
            }
            Err(e) => {
                eprintln!("drifterr: could not open {path}: {e} — running without persistence");
                None
            }
        },
        _ => None,
    };

    eprintln!("drifterr proxy    → {proxy_addr}  (point your tool here)");
    eprintln!("drifterr control  → http://{control_addr}/  (dashboard + /status)");
    eprintln!(
        "drifterr upstream → {} ({})",
        cfg.openai_upstream,
        cfg.provider_label()
    );

    let state = AppState::new(cfg, store);
    if state.auto_reanchor_on() {
        eprintln!("drifterr re-anchor → ON (injects the preamble into drifting requests)");
    }

    // File channel: watch a directory of Claude Code sessions and feed the SAME
    // engine via the normalized format. Zero-config by default (~/.claude/projects);
    // DRIFTERR_WATCH_DIR overrides the location. Held in `_watcher` so it lives for
    // the program's lifetime.
    let watch_dir = std::env::var("DRIFTERR_WATCH_DIR")
        .ok()
        .filter(|d| !d.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(drifterr_proxy::default_claude_projects_dir);
    let _watcher = watch_dir.filter(|p| p.is_dir()).and_then(|dir| {
        let w = drifterr_proxy::watch_claude_sessions(&dir, state.clone());
        if w.is_some() {
            state.set_watching_files(true);
            eprintln!(
                "drifterr files    → watching {}  (Claude Code sessions)",
                dir.display()
            );
        }
        w
    });

    if let Err(e) = serve(proxy_addr, control_addr, state).await {
        eprintln!("drifterr: server error: {e}");
        std::process::exit(1);
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// One-command setup. Detects the AI tool and provider from the environment,
/// prints the exact config to use, and checks whether the proxy is reachable.
/// Read-only and side-effect-free — it never writes files or contacts a provider.
/// `drifterr-proxy check` — run the constraint rules over agent output in CI.
///
/// Exits 0 when clean, 1 on a violation, 2 on a usage error. The distinction matters: a
/// misconfigured check must not look like a passing one, which is why "nothing was
/// checked" is also an error rather than a cheerful zero.
fn run_check() -> i32 {
    use std::io::Read;

    let args: Vec<String> = std::env::args().skip(2).collect();
    let mut rules_path: Option<String> = None;
    let mut pack_id: Option<String> = None;
    let mut pack_path: Option<String> = None;
    let mut input_path: Option<String> = None;
    let mut warn_only = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--rules" => {
                rules_path = args.get(i + 1).cloned();
                i += 2;
            }
            "--pack" => {
                pack_id = args.get(i + 1).cloned();
                i += 2;
            }
            "--pack-file" => {
                pack_path = args.get(i + 1).cloned();
                i += 2;
            }
            "--input" => {
                input_path = args.get(i + 1).cloned();
                i += 2;
            }
            // For adopting the check on an existing repo without blocking everyone on
            // day one: report violations, exit 0. A team that cannot start gradually
            // usually does not start.
            "--warn-only" => {
                warn_only = true;
                i += 1;
            }
            "--help" | "-h" => {
                println!(
                    "Check agent output against the rules you stated.\n\n\
                     USAGE:\n  \
                       git diff origin/main... | drifterr-proxy check --rules CLAUDE.md\n  \
                       drifterr-proxy check --pack tight-scope --input patch.diff\n\n\
                     OPTIONS:\n  \
                       --rules <file>      a rules file (CLAUDE.md, AGENTS.md, .cursor/rules)\n  \
                       --pack <id>         a built-in rule pack\n  \
                       --pack-file <file>  a rule pack JSON file\n  \
                       --input <file>      read from a file instead of stdin\n  \
                       --warn-only         report violations but exit 0\n\n\
                     EXIT CODES:\n  \
                       0 clean · 1 violation · 2 usage error (including nothing to check)\n"
                );
                return 0;
            }
            other => {
                eprintln!("drifterr check: unknown argument {other}");
                return 2;
            }
        }
    }

    if rules_path.is_none() && pack_id.is_none() && pack_path.is_none() {
        eprintln!(
            "drifterr check: nothing to check against. Pass --rules, --pack or --pack-file.\n\
             Refusing to exit 0, because a check with no rules is not a passing check."
        );
        return 2;
    }

    let read = |p: &str| -> Result<String, String> {
        std::fs::read_to_string(p).map_err(|e| format!("cannot read {p}: {e}"))
    };
    let rules_text = match rules_path.as_deref().map(read) {
        Some(Ok(t)) => Some(t),
        Some(Err(e)) => {
            eprintln!("drifterr check: {e}");
            return 2;
        }
        None => None,
    };
    let pack_json = match pack_path.as_deref().map(read) {
        Some(Ok(t)) => Some(t),
        Some(Err(e)) => {
            eprintln!("drifterr check: {e}");
            return 2;
        }
        None => None,
    };

    let (constraints, unchecked) = match drifterr_proxy::check::load(
        rules_text.as_deref(),
        pack_id.as_deref(),
        pack_json.as_deref(),
    ) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("drifterr check: {e}");
            return 2;
        }
    };

    // Content to inspect: a file, or stdin (the `git diff | …` case).
    let content = match input_path.as_deref().map(read) {
        Some(Ok(t)) => t,
        Some(Err(e)) => {
            eprintln!("drifterr check: {e}");
            return 2;
        }
        None => {
            let mut buf = String::new();
            if std::io::stdin().read_to_string(&mut buf).is_err() {
                eprintln!("drifterr check: could not read stdin");
                return 2;
            }
            buf
        }
    };

    let report = drifterr_proxy::check::check_with(&content, constraints, unchecked);
    let gha = std::env::var_os("GITHUB_ACTIONS").is_some();
    print!("{}", report.render(gha));

    if report.checked == 0 {
        // Already explained by `render`. Exit 2, never 0 — a check that verified nothing
        // must not be mistaken for a clean run.
        return 2;
    }
    if report.ok() || warn_only {
        0
    } else {
        1
    }
}

/// `drifterr-proxy hook` — Claude Code `UserPromptSubmit` integration.
///
/// This is what makes automatic re-anchoring real on the zero-config path. The file
/// watcher has no request to rewrite, so before this the flagship channel could only
/// offer a manual copy-paste while the site promised one click. A hook is also
/// strictly better than injection would have been: it runs *before* the model sees
/// the turn, so the reminder lands before the reply that would have broken the rule.
///
/// Exits 0 unconditionally. It sits directly in the path of a keypress, and a
/// monitoring tool that can stop you typing is worse than no monitoring tool at all.
async fn run_hook() {
    use std::io::Read;

    let args: Vec<String> = std::env::args().skip(2).collect();

    if args.iter().any(|a| a == "--install") {
        let exe = std::env::current_exe()
            .ok()
            .and_then(|p| p.to_str().map(str::to_string))
            .unwrap_or_else(|| "drifterr-proxy".to_string());
        println!("Add this to ~/.claude/settings.json (merging with what's there):\n");
        println!("{}", drifterr_proxy::hook::install_snippet(&exe));
        println!(
            "\nThen Drifterr restates your goal and live constraints *before* a drifting\n\
             turn is answered, instead of telling you about it afterwards.\n\n\
             Automatic injection is a Pro capability (every install starts with 14 days).\n\
             Without it the hook stays silent and you can still copy the snapshot from\n\
             the panel.\n\n\
             Check it end to end with:  drifterr-proxy hook --explain < /dev/null"
        );
        return;
    }

    let explain = args.iter().any(|a| a == "--explain");

    // Read the payload. A read failure is just an empty payload — never a hard exit.
    let mut buf = String::new();
    let _ = std::io::stdin().read_to_string(&mut buf);
    let input = drifterr_proxy::hook::parse_input(&buf);

    let control = env_or("DRIFTERR_CONTROL_ADDR", "127.0.0.1:8788");
    let api_base = format!("http://{control}");
    let out = drifterr_proxy::hook::run(&api_base, &input, explain).await;
    if !out.is_empty() {
        println!("{out}");
    }
}

async fn run_init() {
    let control = env_or("DRIFTERR_CONTROL_ADDR", "127.0.0.1:8788");
    let proxy = env_or("DRIFTERR_PROXY_ADDR", "127.0.0.1:8787");

    println!("\n  Drifterr — setup\n  {}", "─".repeat(48));

    // 1) Claude Code — zero-config file channel.
    let claude = drifterr_proxy::default_claude_projects_dir().filter(|p| p.is_dir());
    if let Some(dir) = &claude {
        println!("  ✓ Claude Code detected ({})", dir.display());
        println!("    Nothing to configure — the app watches your sessions.");
        println!("    Just declare your intent in the panel and keep coding.");
    } else {
        println!("  • Claude Code not detected (no ~/.claude/projects).");
    }

    // 2) Provider — recommend a preset from whatever key is already exported.
    // Order matters: the first match wins, mirroring how you'd likely relay.
    let providers: [(&str, &str); 8] = [
        ("OPENROUTER_API_KEY", "openrouter"),
        ("OPENAI_API_KEY", "openai"),
        ("ANTHROPIC_API_KEY", "anthropic"),
        ("GROQ_API_KEY", "groq"),
        ("MISTRAL_API_KEY", "mistral"),
        ("DEEPSEEK_API_KEY", "deepseek"),
        ("XAI_API_KEY", "xai"),
        ("TOGETHER_API_KEY", "together"),
    ];
    let detected: Vec<&str> = providers
        .iter()
        .filter(|(env, _)| std::env::var_os(env).is_some())
        .map(|(_, preset)| *preset)
        .collect();

    println!("\n  For other tools, point them at the proxy:");
    println!("    export OPENAI_BASE_URL=http://{proxy}/v1     # OpenAI-style");
    println!("    export ANTHROPIC_BASE_URL=http://{proxy}      # Anthropic-style");
    match detected.as_slice() {
        [] => {
            println!("\n  • No provider key found in your environment.");
            println!("    Export your own key and (optionally) pick a preset:");
            println!("      export DRIFTERR_PROVIDER=openai   # or anthropic | gemini | groq | …");
        }
        [one] => {
            println!("\n  ✓ Provider key detected → use:");
            println!("      export DRIFTERR_PROVIDER={one}");
        }
        many => {
            println!(
                "\n  ✓ Multiple provider keys detected ({}).",
                many.join(", ")
            );
            println!("    Pick one:  export DRIFTERR_PROVIDER={}", many[0]);
        }
    }

    // 3) Is the proxy already up? Best-effort; a short timeout, localhost only.
    println!("\n  Checking the proxy…");
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_millis(800))
        .build();
    let reachable = match client {
        Ok(c) => c
            .get(format!("http://{control}/health"))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false),
        Err(_) => false,
    };
    if reachable {
        println!("  ✓ Proxy is running — dashboard at http://{control}/");
    } else {
        println!("  • Proxy not reachable on http://{control}/");
        println!("    Start it with `drifterr-proxy`, or open the Drifterr app.");
    }
    println!("  {}\n", "─".repeat(48));
}
