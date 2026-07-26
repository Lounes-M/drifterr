//! Deterministic rule inference from natural-language constraint phrasing.
//!
//! Shared by two callers:
//!
//! * [`crate::signals::constraints`] — to derive a check for a constraint that
//!   was given without an explicit [`Rule`].
//! * [`crate::baseline::Baseline::extract`] — to mine constraints out of the
//!   user's own messages when no baseline was supplied (the proxy channel).
//!
//! Inference is deliberately small and high-precision. It only recognizes
//! phrasings we can turn into a false-positive-free deterministic check;
//! everything else falls through to "no rule". A hard signal that cries wolf is
//! worse than one that stays quiet, so we under-claim rather than guess.

use crate::baseline::Rule;
use regex::Regex;
use std::sync::OnceLock;

/// "no js", "not js", "avoid js", "don't use javascript", "pas de js",
/// "sans javascript", "évite le js" — but NOT "no json" (the `\b` after
/// `js`/`javascript` forbids a following word char).
fn no_js_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:no|not|avoid|don'?t\s+use|do\s+not\s+use|never\s+use|pas\s+de|sans|évite(?:\s+le)?)\s+`?\.?(?:js|javascript)\b",
        )
        .unwrap()
    })
}

/// A word cap: "under 200 words", "at most 200 words", "no more than 200 words",
/// "max 200 words", "within 200 words", "less than 200 words", plus FR "moins de
/// 200 mots" / "au plus 200 mots". Captures the number.
fn max_words_prefix_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:under|below|at\s+most|no\s+more\s+than|max(?:imum)?(?:\s+of)?|within|less\s+than|fewer\s+than|moins\s+de|au\s+plus|maximum\s+de)\s+(\d{1,5})\s+(?:words?|mots?)\b",
        )
        .unwrap()
    })
}

/// The suffix form: "200 words max", "200 words or fewer", "200 mots max".
fn max_words_suffix_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)\b(\d{1,5})\s+(?:words?|mots?)\s+(?:max|maximum|or\s+(?:fewer|less)|ou\s+moins)\b",
        )
        .unwrap()
    })
}

/// The tightest inferable word cap in `text`, if any.
fn infer_max_words(text: &str) -> Option<usize> {
    let mut best: Option<usize> = None;
    for re in [max_words_prefix_re(), max_words_suffix_re()] {
        for caps in re.captures_iter(text) {
            if let Ok(n) = caps[1].parse::<usize>() {
                if n > 0 {
                    best = Some(best.map_or(n, |b| b.min(n)));
                }
            }
        }
    }
    best
}

/// "no comments", "aucun commentaire", "pas de commentaire(s)".
fn no_comments_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?i)\b(no comment|aucun commentaire|pas de commentaire)").unwrap()
    })
}

/// "no TODOs", "no TODO/FIXME", "no placeholders", "pas de TODO". A common
/// Claude Code rule ("finish it, no TODOs left"). We map it to a code-scoped
/// check so the word "todo" in prose never fires it.
fn no_todo_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:no|not|avoid|don'?t\s+(?:use|leave)|do\s+not\s+(?:use|leave)|never\s+(?:use|leave|commit)|without|pas\s+de|sans|aucun)\s+`?(?:todos?|fixmes?|placeholders?)\b",
        )
        .unwrap()
    })
}

/// "no console.log(s)", "remove console logs", "no console statements", "pas de
/// console.log". Scoped to JS/TS debug logging via the literal `console` so it
/// stays precise.
fn no_console_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            // A leading `` ` `` is tolerated because rules files habitually write the
            // symbol as an inline code span (``no `console.log` ``).
            r"(?i)\b(?:no|not|avoid|don'?t\s+use|do\s+not\s+use|never\s+(?:use|commit|leave)|remove|strip|pas\s+de|sans)\s+`?console(?:\.\w+|\s+(?:logs?|statements?|calls?))?\b",
        )
        .unwrap()
    })
}

/// "no any type", "don't use any", "avoid any types", "pas de any" — the
/// TypeScript `any` prohibition. Requires the type context ("any type", "use
/// any", backticked `any`) so the everyday English word "any" never trips it.
fn no_any_type_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        // Requires the TYPE context — "any type(s)", a backticked `any`, or the
        // explicit FR "pas de any" — so the everyday word "any" never fires.
        // "never use" is included because it is the dominant phrasing in project
        // rules files ("Never use `any`"), which is where most of these now come
        // from; it is no less unambiguous than "don't use".
        Regex::new(
            r"(?i)(?:\b(?:no|avoid|don'?t\s+use|do\s+not\s+use|never\s+use)\s+(?:`any`|\bany\b)\s+types?\b|\b(?:no|avoid|don'?t\s+use|do\s+not\s+use|never\s+use)\s+`any`|\bpas\s+de\s+(?:`any`|\bany\b))",
        )
        .unwrap()
    })
}

/// "no new dependencies", "don't add packages/libraries", "no new deps",
/// "pas de nouvelle dépendance", "n'ajoute pas de dépendances". A very common
/// Claude Code guardrail. Checked by forbidding package-manager install commands
/// (that name a package) inside code — see [`INSTALL_CMD_PATTERN`].
fn no_new_deps_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:no\s+(?:new\s+|additional\s+|extra\s+)?(?:dependenc(?:y|ies)|deps?|packages?|libraries|libs?|modules?)|(?:don'?t|do\s+not|never|no\s+need\s+to)\s+(?:add|install|introduce|pull\s+in)\s+(?:any\s+|a\s+|new\s+)?(?:dependenc(?:y|ies)|deps?|packages?|libraries|libs?)|(?:n'?ajoute[rz]?\s+pas|sans|pas\s+de)\s+(?:nouvelles?\s+)?(?:d[ée]pendances?|paquets?|librairies?))\b",
        )
        .unwrap()
    })
}

/// A package-manager install command that names a package (so `npm install`
/// alone — reinstalling existing deps — never fires). Only whitelisted install
/// flags are tolerated before the package, so `pip install -r requirements.txt`
/// (installing *existing* deps from a file) does NOT match. Line-anchored and
/// checked inside code blocks only, keeping it false-positive-free.
const INSTALL_CMD_PATTERN: &str = r"(?im)^[ \t]*(?:sudo[ \t]+)?(?:npm[ \t]+(?:install|i|add)|yarn[ \t]+add|pnpm[ \t]+(?:add|install)|bun[ \t]+add|pip3?[ \t]+install|cargo[ \t]+add|go[ \t]+get|gem[ \t]+install|composer[ \t]+require|poetry[ \t]+add)[ \t]+(?:(?:--save(?:-dev|-exact)?|--global|--dev|--user|--production|--upgrade|-[DgUSEPw])[ \t]+)*[@a-zA-Z0-9][\w@./+-]*";

/// "no eval", "don't use eval", "avoid eval", "pas de eval" — forbid the
/// dynamic-code `eval(` call in code (a classic security/quality constraint).
fn no_eval_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:no|don'?t\s+use|do\s+not\s+use|avoid|never\s+use|pas\s+de|sans)\s+eval\b",
        )
        .unwrap()
    })
}

/// "no hardcoded secrets/keys/passwords", "don't hardcode credentials", "no
/// secrets in code", "pas de secrets en dur". Checked by scanning code for
/// unambiguous secret *shapes* (see [`SECRET_PATTERN`]), never a loose
/// `password = "..."` (which would false-positive on placeholders).
fn no_secrets_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r#"(?i)(?:\bno\s+hard[\s-]?coded?\s+(?:secrets?|keys?|api[\s-]?keys?|passwords?|credentials?|tokens?)|(?:don'?t|do\s+not|never)\s+hard[\s-]?code|\bno\s+(?:secrets?|api[\s-]?keys?|credentials?|tokens?)\s+(?:in\s+(?:the\s+)?code|committed|hard[\s-]?coded)|\bpas\s+de\s+(?:secrets?|cl[ée]s?|mots?\s+de\s+passe|identifiants?)\s+en\s+dur)"#,
        )
        .unwrap()
    })
}

/// Unambiguous secret shapes: AWS access-key id, a PEM private-key header, and
/// common provider token prefixes (GitHub, Slack, OpenAI, Google). Each is
/// specific enough to be false-positive-free; deliberately *not* a generic
/// `KEY = "..."` assignment, which trips on example/placeholder values.
const SECRET_PATTERN: &str = r#"(?:AKIA[0-9A-Z]{16}|-----BEGIN (?:RSA |EC |DSA |OPENSSH |PGP )?PRIVATE KEY-----|ghp_[0-9A-Za-z]{36}|github_pat_[0-9A-Za-z_]{20,}|xox[baprs]-[0-9A-Za-z-]{10,}|sk-[A-Za-z0-9]{20,}|AIza[0-9A-Za-z_-]{35})"#;

/// "don't touch/modify/edit/change X", "leave X alone", "ne touche pas à X" —
/// where X names a file or path. Captures the file so a per-file protected rule
/// can be built. Deliberately high-precision: the captured token must look like a
/// file (has an extension or a path separator), checked by the caller.
fn protected_file_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:(?:don'?t|do\s+not|never|please\s+don'?t)\s+(?:ever\s+)?(?:touch|modify|change|edit|alter|rewrite|refactor)\s+(?:the\s+|module\s+|file\s+)*(\.?[A-Za-z0-9_][\w./-]*)|\bleave\s+(?:the\s+)?(\.?[A-Za-z0-9_][\w./-]*)\s+(?:alone|untouched|as[\s-]is)|\bne\s+(?:touche[rz]?|modifie[rz]?|change[rz]?|[ée]dite[rz]?)\s+pas\s+(?:à\s+la\s+|à\s+|au\s+|le\s+|la\s+|l')*(\.?[A-Za-z0-9_][\w./-]*))",
        )
        .unwrap()
    })
}

// --- layer / scope families ------------------------------------------------
//
// Everything above this line is code *hygiene* — real constraints, but a narrow
// slice of what people actually state. The rules users care most about are about
// *where* work happens: "keep it server-side", "don't touch the migrations",
// "work in the existing files". Those were unrepresentable, which is why the
// marketing site ended up illustrating a "server-side only" detection the engine
// could not perform.
//
// They are made checkable the same way as everything else here: by proving a
// marker appeared inside a code block, or a path appeared in a diff header. We
// never claim to judge whether a *design* is server-side — only that code which
// can only run on the client showed up in a reply that was pinned to the server.

/// "server-side only", "keep it on the server", "backend only", "côté serveur".
fn server_only_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)(?:\bserver[\s-]?side\b|\bback[\s-]?end\b|\bon\s+the\s+server\b|\bc[ôo]t[ée]\s+serveur\b)",
        )
        .unwrap()
    })
}

/// "client-side only", "in the browser", "front-end only", "côté client".
fn client_only_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)(?:\bclient[\s-]?side\b|\bfront[\s-]?end\b|\bin\s+the\s+browser\b|\bc[ôo]t[ée]\s+client\b)",
        )
        .unwrap()
    })
}

/// A cue that the phrase *pins* work to one side rather than merely mentioning it.
/// "keep it server-side" is a constraint; "the server-side cache is warm" is not.
/// Without this gate the layer rules would fire on ordinary architecture talk.
fn pinning_cue_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:only|strictly|purely|keep|stay|stick|must|should|all|entirely|no\s+\w+\s+side|uniquement|seulement|garde|reste|toujours)\b",
        )
        .unwrap()
    })
}

/// Markers that can only appear in **client** code — React hooks and event
/// handlers, DOM and browser globals, `localStorage`. Presence inside a fenced
/// block is proof the reply crossed a server-only boundary.
///
/// Kept to unambiguous browser-only APIs. `fetch(` is excluded on purpose: it
/// exists server-side in modern runtimes, so it would false-positive.
const CLIENT_MARKERS: &str = r#"(?:\buse(?:State|Effect|Ref|Memo|Callback|Context)\s*\(|\bdocument\.(?:getElementById|querySelector|createElement|addEventListener)\b|\bwindow\.(?:location|localStorage|sessionStorage|alert|addEventListener)\b|\blocalStorage\.\w+|\bsessionStorage\.\w+|\bonClick\s*=|\bonChange\s*=|"use client"|'use client')"#;

/// Markers that can only appear in **server** code — filesystem and process
/// access, DB drivers, server-framework route handlers, secret env reads.
///
/// Again unambiguous only: `process.env` is server-side in any bundler-free
/// context, and `fs.`/`createServer` cannot run in a browser.
const SERVER_MARKERS: &str = r#"(?:\bfs\.(?:readFile|writeFile|readFileSync|writeFileSync|createReadStream)\b|\brequire\s*\(\s*['"](?:fs|net|child_process|crypto)['"]|\bfrom\s+['"](?:node:)?(?:fs|net|child_process)['"]|\bprocess\.env\.\w+|\bcreateServer\s*\(|\bapp\.(?:get|post|put|delete)\s*\(|\"use server\"|'use server')"#;

/// "don't touch the migrations", "stay out of tests/", "hands off src/legacy" —
/// a *directory* the assistant must not modify. Captures the directory token.
fn protected_dir_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:(?:don'?t|do\s+not|never)\s+(?:touch|modify|change|edit|go\s+into)\s+(?:the\s+|anything\s+in\s+|inside\s+)*([A-Za-z0-9_][\w./-]*)\s*(?:dir(?:ectory)?|folder|tree)?|(?:stay\s+out\s+of|hands\s+off)\s+(?:the\s+)?([A-Za-z0-9_][\w./-]*)|\bne\s+touche[rz]?\s+pas\s+(?:à\s+|au\s+|aux\s+)?(?:dossier\s+)?([A-Za-z0-9_][\w./-]*))",
        )
        .unwrap()
    })
}

/// Directory names common enough to be worth a scoped rule. An arbitrary word
/// captured by [`protected_dir_re`] is only trusted when it looks like a path
/// (contains `/` or `.`) or appears here — otherwise "don't touch anything" would
/// build a rule matching the literal path `anything`.
const KNOWN_DIRS: &[&str] = &[
    "migrations",
    "migration",
    "tests",
    "test",
    "__tests__",
    "spec",
    "vendor",
    "node_modules",
    "dist",
    "build",
    "target",
    "generated",
    "legacy",
    "schema",
    "schemas",
    "proto",
    "fixtures",
    "snapshots",
    "docs",
    "infra",
    "terraform",
    ".github",
];

/// "no new files", "work in the existing files", "don't create new files",
/// "pas de nouveaux fichiers".
fn no_new_files_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)(?:\bno\s+new\s+files?\b|\b(?:don'?t|do\s+not|never)\s+(?:create|add)\s+(?:any\s+)?new\s+files?\b|\bwork\s+(?:only\s+)?(?:with)?in\s+the\s+existing\s+files?\b|\bexisting\s+files\s+only\b|\bpas\s+de\s+nouveaux?\s+fichiers?\b)",
        )
        .unwrap()
    })
}

/// The tightest inferable *line* cap in `text` (mirrors [`infer_max_words`] for
/// "under 50 lines" / "50 lines max" / "moins de 50 lignes"), if any.
fn infer_max_lines(text: &str) -> Option<usize> {
    static PREFIX: OnceLock<Regex> = OnceLock::new();
    static SUFFIX: OnceLock<Regex> = OnceLock::new();
    let prefix = PREFIX.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:under|below|at\s+most|no\s+more\s+than|max(?:imum)?(?:\s+of)?|within|less\s+than|fewer\s+than|moins\s+de|au\s+plus|maximum\s+de)\s+(\d{1,6})\s+(?:lines?|lignes?)(?:\s+of\s+code)?\b",
        )
        .unwrap()
    });
    let suffix = SUFFIX.get_or_init(|| {
        Regex::new(
            r"(?i)\b(\d{1,6})\s+(?:lines?|lignes?)(?:\s+of\s+code)?\s+(?:max|maximum|or\s+(?:fewer|less)|ou\s+moins)\b",
        )
        .unwrap()
    });
    let mut best: Option<usize> = None;
    for re in [prefix, suffix] {
        for caps in re.captures_iter(text) {
            if let Ok(n) = caps[1].parse::<usize>() {
                if n > 0 {
                    best = Some(best.map_or(n, |b| b.min(n)));
                }
            }
        }
    }
    best
}

/// Strip *trailing* sentence punctuation/quotes from a captured file token, so
/// "config.py." or "package.json," yields the bare path. Trailing-only, so a
/// leading dot (`.env`, `.gitignore`) is preserved.
fn clean_file_token(tok: &str) -> &str {
    tok.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '"', '\''])
}

/// Every deterministic rule we can confidently infer from `text`.
///
/// Returns all matches (a single message may state several constraints), so the
/// baseline extractor can capture more than one. Case-insensitive; recognizes
/// both English and French phrasings, matching the product's audience. Matching
/// is word-boundary-anchored so near-misses ("no json") don't fire — a hard
/// signal must not be built on a sloppy substring.
pub fn infer_rules(text: &str) -> Vec<Rule> {
    let mut rules = Vec::new();

    // "no JS" / "no .js" — forbid JavaScript file extensions.
    if no_js_re().is_match(text) {
        rules.push(Rule::ForbidPattern {
            pattern: r"\.js\b".to_string(),
        });
    }

    // "No comments in code" — forbid comment syntax inside code blocks.
    if no_comments_re().is_match(text) {
        rules.push(Rule::ForbidInCode {
            pattern: r"(//|/\*|^\s*#|<!--)".to_string(),
        });
    }

    // "No TODOs / FIXMEs" — forbid leftover markers in code.
    if no_todo_re().is_match(text) {
        rules.push(Rule::ForbidInCode {
            pattern: r"\b(?:TODO|FIXME)\b".to_string(),
        });
    }

    // "No console.log" — forbid JS/TS debug logging left in code.
    if no_console_re().is_match(text) {
        rules.push(Rule::ForbidInCode {
            pattern: r"console\.(?:log|debug|info|warn|error)\b".to_string(),
        });
    }

    // "No `any` type" — forbid TypeScript `any` annotations in code.
    if no_any_type_re().is_match(text) {
        rules.push(Rule::ForbidInCode {
            pattern: r":\s*any\b".to_string(),
        });
    }

    // "under 200 words" / "200 words max" — a hard length cap.
    if let Some(max) = infer_max_words(text) {
        rules.push(Rule::MaxWords { max });
    }

    // "under 50 lines" / "50 lines max" — a per-code-block length cap.
    if let Some(max) = infer_max_lines(text) {
        rules.push(Rule::MaxLines { max });
    }

    // "No new dependencies" — forbid install commands that name a package.
    if no_new_deps_re().is_match(text) {
        rules.push(Rule::ForbidInCode {
            pattern: INSTALL_CMD_PATTERN.to_string(),
        });
    }

    // "No eval" — forbid the dynamic-code eval() call in code.
    if no_eval_re().is_match(text) {
        rules.push(Rule::ForbidInCode {
            pattern: r"\beval\s*\(".to_string(),
        });
    }

    // "No hardcoded secrets" — scan code for unambiguous secret shapes.
    if no_secrets_re().is_match(text) {
        rules.push(Rule::ForbidInCode {
            pattern: SECRET_PATTERN.to_string(),
        });
    }

    // "Don't touch <file>" — forbid a unified-diff header naming that file.
    // Parameterized per captured path; a prose mention won't match a diff line,
    // and tool-only edits (no diff text) are under-claimed by design.
    for caps in protected_file_re().captures_iter(text) {
        let Some(tok) = caps.get(1).or_else(|| caps.get(2)).or_else(|| caps.get(3)) else {
            continue;
        };
        let file = clean_file_token(tok.as_str());
        // Must look like a file: an extension or a path separator. This keeps
        // "don't touch it/that" from ever producing a rule.
        if !(file.contains('/') || file.contains('.')) || file.len() < 3 {
            continue;
        }
        let esc = regex::escape(file);
        let pattern = format!(r"(?m)^(?:diff --git |\+\+\+ |--- )[ab]/(?:\S*/)?{esc}\b");
        let rule = Rule::ForbidPattern { pattern };
        if !rules.contains(&rule) {
            rules.push(rule);
        }
    }

    // --- layer / scope families --------------------------------------------

    // "Keep it server-side" / "client-side only" — forbid the *other* side's
    // unambiguous markers inside code blocks. Requires a pinning cue so ordinary
    // architecture talk ("the server-side cache is warm") can't build a rule, and
    // skips text that pins both sides at once (a description, not a constraint).
    if pinning_cue_re().is_match(text) {
        let server = server_only_re().is_match(text);
        let client = client_only_re().is_match(text);
        if server && !client {
            rules.push(Rule::ForbidLayerMarkers {
                label: "server-side only".to_string(),
                pattern: CLIENT_MARKERS.to_string(),
            });
        } else if client && !server {
            rules.push(Rule::ForbidLayerMarkers {
                label: "client-side only".to_string(),
                pattern: SERVER_MARKERS.to_string(),
            });
        }
    }

    // "Don't touch the migrations" — forbid diff headers under that directory.
    for caps in protected_dir_re().captures_iter(text) {
        let Some(tok) = caps.get(1).or_else(|| caps.get(2)).or_else(|| caps.get(3)) else {
            continue;
        };
        let dir = clean_file_token(tok.as_str()).trim_end_matches('/');
        // A token whose last segment has an extension is a *file*, and
        // `protected_file_re` above already owns those. Skipping them here keeps one
        // violation from producing two events for the same edit.
        let last_segment = dir.rsplit('/').next().unwrap_or(dir);
        if last_segment.contains('.') {
            continue;
        }
        // Trust a captured word only if it is path-shaped or is a directory name
        // worth scoping. Otherwise "don't touch anything else" yields a rule
        // matching the literal path `anything`.
        let path_like = dir.contains('/');
        if dir.len() < 3 || !(path_like || KNOWN_DIRS.contains(&dir.to_ascii_lowercase().as_str()))
        {
            continue;
        }
        let esc = regex::escape(dir);
        // Match the directory anywhere in the touched path, as a full segment.
        let rule = Rule::ForbidPathTouch {
            pattern: format!(r"(?i)(?:^|/){esc}(?:/|$)"),
        };
        if !rules.contains(&rule) {
            rules.push(rule);
        }
    }

    // "No new files" — forbid diffs that create files.
    if no_new_files_re().is_match(text) {
        rules.push(Rule::ForbidNewFiles);
    }

    rules
}

/// The first inferable rule, if any — the per-constraint fallback used by the
/// constraint checker.
pub fn infer_rule(text: &str) -> Option<Rule> {
    infer_rules(text).into_iter().next()
}

/// Explicit-rejection phrasings the user might use to discard an approach/tech:
/// "don't use X", "stop using X", "avoid X", "get rid of X", "instead of X",
/// "rather than X", plus FR "n'utilise pas X", "au lieu de X", "plutôt que X",
/// "abandonne X", "pas de X". A single leading verb-alternation, an optional
/// filler group ("using"/"the"/"any"/"de"/"l'"…), then one capture for the
/// object. Verbs prone to non-rejection idioms ("remove", "drop", "skip",
/// "sans", "no more") are deliberately excluded to keep precision.
fn rejected_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:don'?t\s+(?:use|want)|do\s+not\s+use|stop\s+using|no\s+longer\s+use|avoid|get\s+rid\s+of|ditch|steer\s+clear\s+of|stay\s+away\s+from|instead\s+of|rather\s+than|n'?utilise[rz]?\s+(?:pas|plus)|on\s+n'?utilise\s+pas|arr[êe]te[rz]?\s+d'utiliser|au\s+lieu\s+de?|plut[ôo]t\s+que|abandonne[rz]?|laisse[rz]?\s+tomber|pas\s+de|[ée]vite[rz]?)[\s']+(?:using\s+|use\s+of\s+|to\s+use\s+|utiliser\s+|the\s+|a\s+|an\s+|any\s+|de\s+|d'|l'|le\s+|la\s+|les\s+)*([a-z0-9][a-z0-9._+\-]{1,38})",
        )
        .unwrap()
    })
}

/// Objects that are pronouns/articles/idiom fragments, not a rejected
/// technology — filtered so "avoid the trap"/"sans doute"-style captures don't
/// pollute the decision set.
fn is_nonspecific_object(lc: &str) -> bool {
    matches!(
        lc,
        "it" | "this"
            | "that"
            | "these"
            | "those"
            | "them"
            | "the"
            | "a"
            | "an"
            | "any"
            | "us"
            | "me"
            | "you"
            | "him"
            | "her"
            | "doing"
            | "using"
            | "everything"
            | "anything"
            | "something"
            | "nothing"
            | "than"
            | "doute"
            | "cesse"
    )
}

/// Extract decisions the user explicitly rejected, as short normalized phrases
/// (e.g. "use bcrypt"). High-precision by design — it only matches clear
/// rejection statements, filters pronouns/idiom fragments, and skips phrasings
/// the deterministic JS/comments rules already cover.
pub fn infer_rejected_decisions(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for caps in rejected_re().captures_iter(text) {
        if let Some(obj) = caps.get(1) {
            let object = obj
                .as_str()
                .trim()
                .trim_end_matches(['.', ',', ';', ':', '!', '?'])
                .trim();
            let lc = object.to_ascii_lowercase();
            // Drop phrasings the deterministic rules already own, and pronoun /
            // idiom captures that aren't a real rejected approach.
            if lc == "js" || lc == "javascript" || lc.starts_with("comment") {
                continue;
            }
            if is_nonspecific_object(&lc) {
                continue;
            }
            let phrase = format!("use {object}");
            if !out.contains(&phrase) {
                out.push(phrase);
            }
        }
    }
    out
}

/// A broad EN/FR cue that a user message *states a constraint* on the
/// assistant's work — imperative "must/only/always/never", prohibitions
/// ("don't", "no ", "avoid"), or explicit constraint words. Used as a cheap
/// local gate so we only spend an LLM extraction call on turns that plausibly
/// carry a rule, never on every message.
///
/// This is deliberately *recall-oriented* (unlike [`infer_rules`], which is
/// precision-oriented): a false positive here only costs one judge call that
/// returns `[]`, whereas a miss would silently drop a real fuzzy constraint. The
/// judge is the precision stage.
pub fn has_constraint_cue(text: &str) -> bool {
    constraint_cue_re().is_match(text)
}

fn constraint_cue_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:must(?:\s+not)?|only|always|never|do\s+not|don'?t|no\s+\w|without|avoid|ensure|make\s+sure|keep\s+it|stick\s+to|limit|at\s+most|at\s+least|constraint|require[ds]?|forbid|prohibit|toujours|jamais|uniquement|seulement|sans|évite|assure|limite|au\s+plus|au\s+moins|pas\s+de|ne\s+pas)\b",
        )
        .unwrap()
    })
}

/// Recover the literal path a parameterized rule was built around, by undoing
/// [`regex::escape`] on the captured segment between `before` and `after`.
///
/// The parameterized rules (protected file, protected directory) bake the user's own
/// path into their pattern, and that path is the only useful part of the label: "a
/// protected file must not be modified" tells nobody *which*. Recovering it here keeps
/// the label honest without having to widen `Rule` itself.
fn literal_between(pattern: &str, before: &str, after: &str) -> Option<String> {
    let start = pattern.find(before)? + before.len();
    let rest = &pattern[start..];
    let end = rest.find(after)?;
    let escaped = &rest[..end];
    if escaped.is_empty() {
        return None;
    }
    // `regex::escape` only ever inserts a backslash before an ASCII punctuation
    // character, so dropping those backslashes is an exact inverse.
    let mut out = String::with_capacity(escaped.len());
    let mut chars = escaped.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            // A trailing lone backslash means this isn't output of `regex::escape`, so
            // there is no path to recover — bail rather than invent one.
            out.push(chars.next()?);
        } else {
            out.push(c);
        }
    }
    Some(out)
}

/// A human label and category for an inferred rule, used when the extractor
/// synthesizes a [`crate::baseline::Constraint`] from a bare rule.
///
/// Labels name the *specific* rule wherever the rule carries a parameter, because this
/// string is what the panel shows, what the re-anchor preamble restates, and what the
/// MCP server hands an agent asking which rules apply. "A protected file must not be
/// modified" is useless in all three places; "package.json must not be modified" is
/// actionable in all three.
pub fn describe(rule: &Rule) -> (String, crate::baseline::ConstraintType) {
    use crate::baseline::ConstraintType;
    let (text, kind): (String, ConstraintType) = match rule {
        // ForbidPattern covers the no-JS rule and the parameterized protected-file
        // rule; the diff-header pattern distinguishes the latter.
        Rule::ForbidPattern { pattern } if pattern.contains("diff --git") => (
            match literal_between(pattern, r"[ab]/(?:\S*/)?", r"\b") {
                Some(file) => format!("{file} must not be modified"),
                None => "A protected file must not be modified".to_string(),
            },
            ConstraintType::Tech,
        ),
        Rule::ForbidPattern { .. } => (
            "TypeScript only, no JS files".to_string(),
            ConstraintType::Tech,
        ),
        // Several distinct code rules share the ForbidInCode mechanism; name each
        // by its pattern so the panel can state the actual cause, not a generic
        // "no comments". Keep these substrings in sync with `infer_rules`.
        Rule::ForbidInCode { pattern } if pattern.contains("TODO") => (
            "No TODOs or FIXMEs in code".to_string(),
            ConstraintType::Format,
        ),
        Rule::ForbidInCode { pattern } if pattern.contains("console") => (
            "No console logging in code".to_string(),
            ConstraintType::Format,
        ),
        Rule::ForbidInCode { pattern } if pattern.contains(":\\s*any") => {
            ("No `any` type in code".to_string(), ConstraintType::Tech)
        }
        Rule::ForbidInCode { pattern } if pattern.contains("npm") => {
            ("No new dependencies".to_string(), ConstraintType::Tech)
        }
        Rule::ForbidInCode { pattern } if pattern.contains("eval") => {
            ("No eval() calls in code".to_string(), ConstraintType::Tech)
        }
        Rule::ForbidInCode { pattern } if pattern.contains("AKIA") => (
            "No hardcoded secrets in code".to_string(),
            ConstraintType::Tech,
        ),
        Rule::ForbidInCode { .. } => ("No comments in code".to_string(), ConstraintType::Format),
        Rule::RequirePattern { .. } => (
            "Required pattern must be present".to_string(),
            ConstraintType::Tech,
        ),
        Rule::MaxWords { .. } => (
            "Stay within the word limit".to_string(),
            ConstraintType::Format,
        ),
        Rule::MaxLines { .. } => (
            "Stay within the code line limit".to_string(),
            ConstraintType::Format,
        ),
        Rule::ForbidPathTouch { pattern } => (
            match literal_between(pattern, r"(?:^|/)", r"(?:/|$)") {
                Some(dir) => format!("Nothing under {dir}/ may be modified"),
                None => "A protected directory must not be modified".to_string(),
            },
            ConstraintType::Tech,
        ),
        // The rule already carries the boundary the user pinned, so name it: an agent
        // told "stay on the pinned layer" has no idea which layer that is.
        Rule::ForbidLayerMarkers { label, .. } => {
            (format!("Work must stay {label}"), ConstraintType::Tech)
        }
        Rule::ForbidNewFiles => ("No new files".to_string(), ConstraintType::Tech),
    };
    (text, kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infers_both_from_one_message() {
        let rules = infer_rules("Use TypeScript only, no JS, and no comments please");
        assert_eq!(rules.len(), 2);
    }

    #[test]
    fn nothing_inferred_from_plain_text() {
        assert!(infer_rules("please make it fast and elegant").is_empty());
    }

    #[test]
    fn rejected_decisions_high_precision() {
        assert_eq!(
            infer_rejected_decisions("Please don't use bcrypt for hashing"),
            vec!["use bcrypt".to_string()]
        );
        assert_eq!(
            infer_rejected_decisions("avoid redux and stop using moment"),
            vec!["use redux".to_string(), "use moment".to_string()]
        );
        // Plain prose with no rejection phrasing → nothing.
        assert!(infer_rejected_decisions("we should ship this feature soon").is_empty());
    }

    #[test]
    fn rejected_decisions_broadened_phrasings() {
        // Broader EN rejection verbs, incl. comparative "instead of / rather than".
        for (text, want) in [
            ("let's get rid of jest here", "use jest"),
            ("use vitest instead of jest", "use jest"),
            ("rather than using moment, pick date-fns", "use moment"),
            ("ditch webpack for this", "use webpack"),
            ("steer clear of lodash", "use lodash"),
            ("don't want mongodb in this project", "use mongodb"),
        ] {
            assert!(
                infer_rejected_decisions(text).contains(&want.to_string()),
                "want {want:?} from {text:?}, got {:?}",
                infer_rejected_decisions(text)
            );
        }
        // FR rejection phrasings.
        for (text, want) in [
            ("au lieu de redux, on prend zustand", "use redux"),
            ("plutôt que moment on utilise luxon", "use moment"),
            ("abandonne webpack", "use webpack"),
            ("n'utilise pas axios", "use axios"),
            ("au lieu d'utiliser jquery", "use jquery"),
        ] {
            assert!(
                infer_rejected_decisions(text).contains(&want.to_string()),
                "want {want:?} from {text:?}, got {:?}",
                infer_rejected_decisions(text)
            );
        }
        // Pronoun / idiom captures are filtered (no nonsense decisions).
        assert!(
            infer_rejected_decisions("avoid the trap of premature optimization")
                .iter()
                .all(|d| d != "use the")
        );
        assert!(infer_rejected_decisions("don't use it if you can help it").is_empty());
        // A length rule ("no more than 200 words") must not read as a rejection.
        assert!(infer_rejected_decisions("keep it to no more than 200 words").is_empty());
    }

    #[test]
    fn constraint_cue_recall() {
        // Imperatives / prohibitions / explicit constraint words fire (EN + FR).
        assert!(has_constraint_cue("Keep it under 200 words please"));
        assert!(has_constraint_cue("You must always cite your sources"));
        assert!(has_constraint_cue("don't use any external libraries"));
        assert!(has_constraint_cue("réponds uniquement en français"));
        assert!(has_constraint_cue("sans jamais utiliser de jargon"));
        // Plain task requests with no rule phrasing stay quiet.
        assert!(!has_constraint_cue("can you help me write a poem"));
        assert!(!has_constraint_cue("what is the capital of France"));
    }

    #[test]
    fn ts_abbreviation_and_no_json_guard() {
        // "TS, no JS" should infer the rule even without the word "typescript".
        assert_eq!(infer_rules("refactor in TS, no JS").len(), 1);
        // "no json" must NOT trigger the JS rule.
        assert!(infer_rules("return no json, just text").is_empty());
    }

    #[test]
    fn no_js_broadened_phrasings() {
        for s in [
            "avoid javascript entirely",
            "don't use js here",
            "do not use JavaScript",
            "évite le js",
            "sans javascript",
        ] {
            assert!(
                infer_rules(s)
                    .iter()
                    .any(|r| matches!(r, Rule::ForbidPattern { .. })),
                "should infer no-JS from: {s}"
            );
        }
        // "jsonify" / "no json" still must not fire.
        assert!(infer_rules("please jsonify the output").is_empty());
    }

    #[test]
    fn word_limit_inference() {
        let cases = [
            ("keep it under 200 words", 200),
            ("at most 50 words please", 50),
            ("no more than 1000 words", 1000),
            ("answer in 30 words max", 30),
            ("réponds en moins de 100 mots", 100),
            ("120 mots max", 120),
        ];
        for (text, expect) in cases {
            let max = infer_rules(text).into_iter().find_map(|r| match r {
                Rule::MaxWords { max } => Some(max),
                _ => None,
            });
            assert_eq!(max, Some(expect), "for: {text}");
        }
        // The tightest cap wins when several are stated.
        let max = infer_rules("under 500 words, ideally 200 words max")
            .into_iter()
            .find_map(|r| match r {
                Rule::MaxWords { max } => Some(max),
                _ => None,
            });
        assert_eq!(max, Some(200));
        // Numbers unrelated to a word cap don't fire.
        assert!(infer_rules("use port 8080 and 3 retries").is_empty());
    }

    /// Helper: does `text` infer a code rule whose forbid pattern contains `frag`?
    fn infers_code_pattern(text: &str, frag: &str) -> bool {
        infer_rules(text)
            .iter()
            .any(|r| matches!(r, Rule::ForbidInCode { pattern } if pattern.contains(frag)))
    }

    #[test]
    fn no_todo_inference() {
        for s in [
            "no TODOs left please",
            "finish it, no todo",
            "don't leave FIXMEs",
            "no placeholders",
            "pas de todo",
            "sans placeholder",
        ] {
            assert!(
                infers_code_pattern(s, "TODO"),
                "should infer no-TODO from: {s}"
            );
        }
        // "today" / prose must not fire (word-boundary + specific words).
        assert!(!infers_code_pattern("ship it today", "TODO"));
        assert!(infer_rules("what should I do today?").is_empty());
    }

    #[test]
    fn no_console_inference() {
        for s in [
            "no console.log",
            "remove console logs",
            "don't use console.error",
            "strip console statements",
            "pas de console.log",
        ] {
            assert!(
                infers_code_pattern(s, "console"),
                "should infer no-console from: {s}"
            );
        }
        // A game console mention without a prohibition doesn't fire.
        assert!(!infers_code_pattern(
            "the console shows the score",
            "console"
        ));
    }

    #[test]
    fn no_any_type_inference() {
        for s in [
            "no any type",
            "don't use any type",
            "do not use any types",
            "avoid any types",
            "no `any`",
            "pas de any",
        ] {
            assert!(
                infers_code_pattern(s, "any"),
                "should infer no-any from: {s}"
            );
        }
        // Bare "any" without the type context stays quiet (precision).
        assert!(!infers_code_pattern("use any approach you prefer", "any"));
        // The everyday word "any" in prose must never trip it.
        assert!(!infers_code_pattern(
            "let me know if you have any questions",
            "any"
        ));
        assert!(infer_rules("pick any font you like").is_empty());
    }

    /// "Never …" is the dominant phrasing in project rules files (`CLAUDE.md`,
    /// `.cursor/rules`), which are now a primary source of constraints. It was
    /// missing from four of the families, so a rule the user had genuinely written
    /// down imported as nothing.
    #[test]
    fn never_phrasing_infers_like_dont() {
        assert!(infers_code_pattern("Never use `any`", "any"));
        assert!(infers_code_pattern("Never use any types", "any"));
        assert!(infers_code_pattern("Never use console.log", "console"));
        assert!(infers_code_pattern(
            "Never commit console statements",
            "console"
        ));
        assert!(infers_code_pattern("Never leave TODOs behind", "TODO"));
        assert!(matches!(
            infer_rules("Never use JavaScript here").first(),
            Some(Rule::ForbidPattern { .. })
        ));

        // Precision must survive: "never" alone is not a prohibition on these
        // things, and the everyday adverb must not manufacture a rule.
        assert!(infer_rules("I never got around to it").is_empty());
        assert!(infer_rules("this never happens in practice").is_empty());
        assert!(infer_rules("never mind the formatting").is_empty());
        // "never use" still needs a real object to attach to.
        assert!(infer_rules("never use it that way").is_empty());
    }

    // --- layer / scope families -------------------------------------------

    #[test]
    fn server_only_forbids_client_markers() {
        // The constraint the marketing site used to illustrate — now real.
        for s in [
            "Keep it server-side",
            "server-side only please",
            "This must stay on the server",
            "backend only",
            "garde ça côté serveur uniquement",
        ] {
            let rules = infer_rules(s);
            let layer = rules.iter().find_map(|r| match r {
                Rule::ForbidLayerMarkers { label, pattern } => Some((label, pattern)),
                _ => None,
            });
            let (label, pattern) = layer.unwrap_or_else(|| panic!("no layer rule from {s:?}"));
            assert_eq!(label, "server-side only");
            assert!(
                pattern.contains("localStorage"),
                "should forbid client markers, got {pattern}"
            );
        }
    }

    #[test]
    fn client_only_forbids_server_markers() {
        let rules = infer_rules("client-side only — no server calls");
        let layer = rules.iter().find_map(|r| match r {
            Rule::ForbidLayerMarkers { label, pattern } => Some((label, pattern)),
            _ => None,
        });
        let (label, pattern) = layer.expect("no layer rule");
        assert_eq!(label, "client-side only");
        assert!(
            pattern.contains("child_process"),
            "should forbid server markers, got {pattern}"
        );
    }

    #[test]
    fn layer_rules_need_a_pinning_cue_and_one_side() {
        let has_layer = |s: &str| {
            infer_rules(s)
                .iter()
                .any(|r| matches!(r, Rule::ForbidLayerMarkers { .. }))
        };
        // Mentioning a layer is not pinning to it — this is the false-positive
        // risk that makes the whole family worth guarding.
        assert!(!has_layer("the server-side cache is warm"));
        assert!(!has_layer("our backend is written in Go"));
        assert!(!has_layer(
            "compare the client-side and server-side timings"
        ));
        // Naming BOTH sides describes an architecture; it does not pin one.
        assert!(!has_layer(
            "keep the server-side logic and client-side UI separate"
        ));
        // Pinning cue plus exactly one side ⇒ a rule.
        assert!(has_layer("keep all of this server-side"));
    }

    #[test]
    fn server_only_violation_is_detected_in_code_only() {
        use crate::baseline::{Checkable, Constraint, ConstraintType};
        let rule = infer_rule("keep it server-side only").unwrap();
        let c = Constraint {
            id: "c1".into(),
            text: "keep it server-side only".into(),
            kind: ConstraintType::Tech,
            checkable: Checkable::Deterministic,
            active: true,
            rule: Some(rule),
        };
        let check = |content: &str| crate::signals::constraints::check_for_test(&c, content);

        // Client-only code inside a fence ⇒ violation, and the span names the
        // boundary so the panel can state a cause.
        let violated = check("```tsx\nconst [x, setX] = useState(0);\n```").unwrap();
        assert!(violated.unwrap().contains("server-side only"));

        // Server code is fine.
        assert!(check("```ts\nconst p = process.env.PORT;\n```").is_none());
        // Prose that merely says "useState" is not code — no fence, no violation.
        assert!(check("You could use useState here, but we'll keep it on the server.").is_none());
    }

    #[test]
    fn protected_directory_infers_from_known_dirs_and_paths() {
        let dir_pattern = |s: &str| {
            infer_rules(s).into_iter().find_map(|r| match r {
                Rule::ForbidPathTouch { pattern } => Some(pattern),
                _ => None,
            })
        };
        assert!(dir_pattern("don't touch the migrations").is_some());
        assert!(dir_pattern("stay out of tests").is_some());
        assert!(dir_pattern("don't modify src/legacy/").is_some());
        assert!(dir_pattern("ne touche pas au dossier migrations").is_some());

        // An arbitrary word is NOT a directory: "don't touch anything" must not
        // produce a rule matching the literal path `anything`.
        assert!(dir_pattern("don't touch anything else").is_none());
        assert!(dir_pattern("don't change it").is_none());
        // A *file* is owned by the protected-file rule; the directory rule must not
        // also fire, or one edit would report two violations.
        assert!(dir_pattern("don't touch package.json").is_none());
        assert!(
            infer_rules("don't touch package.json")
                .iter()
                .any(|r| matches!(r, Rule::ForbidPattern { .. })),
            "the file rule still covers it"
        );
    }

    #[test]
    fn protected_directory_fires_on_diff_headers_only() {
        use crate::baseline::{Checkable, Constraint, ConstraintType};
        let rule = infer_rule("don't touch the migrations").unwrap();
        let c = Constraint {
            id: "c1".into(),
            text: "don't touch the migrations".into(),
            kind: ConstraintType::Tech,
            checkable: Checkable::Deterministic,
            active: true,
            rule: Some(rule),
        };
        let check = |content: &str| crate::signals::constraints::check_for_test(&c, content);

        // A diff under the directory ⇒ violation, span naming the path.
        let v = check(
            "```diff\n--- a/db/migrations/002_add_x.sql\n+++ b/db/migrations/002_add_x.sql\n```",
        )
        .unwrap();
        assert_eq!(v.as_deref(), Some("db/migrations/002_add_x.sql"));
        // A different tree is untouched.
        assert!(check("```diff\n+++ b/src/app.ts\n```").is_none());
        // Prose mentioning it is discussion, not modification.
        assert!(check("I read db/migrations/002_add_x.sql but changed nothing").is_none());
    }

    #[test]
    fn no_new_files_fires_on_git_creation_markers_only() {
        use crate::baseline::{Checkable, Constraint, ConstraintType};
        let c = Constraint {
            id: "c1".into(),
            text: "no new files".into(),
            kind: ConstraintType::Tech,
            checkable: Checkable::Deterministic,
            active: true,
            rule: Some(Rule::ForbidNewFiles),
        };
        let check = |content: &str| crate::signals::constraints::check_for_test(&c, content);

        // `--- /dev/null` and `new file mode` are the two unambiguous markers.
        let v = check("```diff\n--- /dev/null\n+++ b/src/helper.ts\n```").unwrap();
        assert_eq!(v.as_deref(), Some("src/helper.ts"));
        let v2 = check("```diff\ndiff --git a/x.ts b/x.ts\nnew file mode 100644\n+++ b/x.ts\n```")
            .unwrap();
        assert_eq!(v2.as_deref(), Some("x.ts"));
        // Editing an existing file is fine.
        assert!(check("```diff\n--- a/src/app.ts\n+++ b/src/app.ts\n```").is_none());
        // Saying the words is not creating a file.
        assert!(check("we could add a new file for that later").is_none());
    }

    #[test]
    fn no_new_files_infers() {
        for s in [
            "no new files",
            "don't create new files",
            "work within the existing files",
            "pas de nouveaux fichiers",
        ] {
            assert!(
                infer_rules(s).contains(&Rule::ForbidNewFiles),
                "should infer no-new-files from {s:?}"
            );
        }
        assert!(!infer_rules("add the new file to the index").contains(&Rule::ForbidNewFiles));
    }

    /// Rules files write symbols as inline code spans, so the object of a
    /// prohibition is routinely backticked. Without tolerating that, a rule the
    /// user did write imports as nothing.
    #[test]
    fn backticked_objects_still_infer() {
        assert!(infers_code_pattern("Never use `console.log`", "console"));
        assert!(infers_code_pattern(
            "No `console.log` in committed code",
            "console"
        ));
        assert!(infers_code_pattern("Don't leave `TODO` markers", "TODO"));
        assert!(matches!(
            infer_rules("No `.js` files — TypeScript only").first(),
            Some(Rule::ForbidPattern { .. })
        ));
        // A backtick alone is not a rule statement.
        assert!(infer_rules("run `npm test` before pushing").is_empty());
    }

    #[test]
    fn no_new_deps_inference() {
        for s in [
            "no new dependencies",
            "don't add any dependencies",
            "no new packages please",
            "never install new libraries",
            "pas de nouvelles dépendances",
            "n'ajoute pas de dépendances",
        ] {
            assert!(
                infer_rules(s).iter().any(
                    |r| matches!(r, Rule::ForbidInCode { pattern } if pattern.contains("npm"))
                ),
                "should infer no-new-deps from: {s}"
            );
        }
        // A plain feature ask must not fire.
        assert!(infer_rules("add a login page").is_empty());
        assert!(infer_rules("write the dependency injection container").is_empty());
    }

    #[test]
    fn no_eval_inference() {
        for s in ["no eval", "don't use eval", "avoid eval", "pas de eval"] {
            assert!(
                infers_code_pattern(s, "eval"),
                "should infer no-eval from: {s}"
            );
        }
        // "evaluate" / "retrieval" must not fire (word boundary in the cue).
        assert!(infer_rules("please evaluate the tradeoffs").is_empty());
    }

    #[test]
    fn no_secrets_inference() {
        for s in [
            "no hardcoded secrets",
            "don't hardcode API keys",
            "no credentials in code",
            "pas de secrets en dur",
        ] {
            assert!(
                infers_code_pattern(s, "AKIA"),
                "should infer no-secrets from: {s}"
            );
        }
        assert!(infer_rules("keep the design secret for now").is_empty());
    }

    #[test]
    fn protected_file_inference() {
        // A named file/path yields a diff-header ForbidPattern for that file.
        let rules = infer_rules("please don't touch package.json");
        assert!(rules
            .iter()
            .any(|r| matches!(r, Rule::ForbidPattern { pattern }
                if pattern.contains(r"package\.json") && pattern.contains("diff --git"))));
        // A leading dot (dotfile) is preserved, not stripped.
        assert!(infer_rules("don't modify .env")
            .iter()
            .any(|r| matches!(r, Rule::ForbidPattern { pattern } if pattern.contains(r"\.env"))));
        // Path with separators works too.
        assert!(infer_rules("do not edit src/db/schema.sql").iter().any(
            |r| matches!(r, Rule::ForbidPattern { pattern } if pattern.contains(r"schema\.sql"))
        ));
        // FR + "leave X alone".
        assert!(!infer_rules("ne touche pas à config.toml").is_empty());
        assert!(!infer_rules("leave Cargo.lock alone").is_empty());
        // A non-file object ("it", a bare word) produces no rule (precision).
        assert!(infer_rules("don't touch it").is_empty());
        assert!(infer_rules("don't change the plan").is_empty());
    }

    #[test]
    fn line_limit_inference() {
        for (text, expect) in [
            ("keep functions under 40 lines", 40usize),
            ("no more than 100 lines", 100),
            ("50 lines max", 50),
            ("moins de 30 lignes", 30),
            ("under 200 lines of code", 200),
        ] {
            let max = infer_rules(text).into_iter().find_map(|r| match r {
                Rule::MaxLines { max } => Some(max),
                _ => None,
            });
            assert_eq!(max, Some(expect), "for: {text}");
        }
        // The tightest cap wins; word caps stay a separate rule.
        assert!(infer_rules("under 200 words")
            .iter()
            .all(|r| !matches!(r, Rule::MaxLines { .. })));
    }

    #[test]
    fn new_code_rules_are_named_distinctly() {
        use crate::baseline::Rule;
        let todo = describe(&Rule::ForbidInCode {
            pattern: r"\b(?:TODO|FIXME)\b".into(),
        })
        .0;
        let console = describe(&Rule::ForbidInCode {
            pattern: r"console\.(?:log|debug|info|warn|error)\b".into(),
        })
        .0;
        let any = describe(&Rule::ForbidInCode {
            pattern: r":\s*any\b".into(),
        })
        .0;
        let comments = describe(&Rule::ForbidInCode {
            pattern: r"(//|/\*)".into(),
        })
        .0;
        // Each ForbidInCode rule names its own cause — no generic collision.
        assert!(todo.contains("TODO"));
        assert!(console.to_lowercase().contains("console"));
        assert!(any.contains("any"));
        assert!(comments.to_lowercase().contains("comment"));
        assert_ne!(todo, comments);
        assert_ne!(console, comments);
        assert_ne!(any, comments);
    }

    #[test]
    fn parameterized_rules_name_the_path_they_protect() {
        // The label is what the panel shows, what a re-anchor restates, and what the MCP
        // server hands an agent asking which rules apply. "A protected file must not be
        // modified" is useless in all three: it does not say *which* file.
        let file = infer_rule("Don't touch package.json").expect("a rule");
        assert_eq!(describe(&file).0, "package.json must not be modified");

        let dir = infer_rule("Don't touch the migrations").expect("a rule");
        assert_eq!(
            describe(&dir).0,
            "Nothing under migrations/ may be modified"
        );

        // Layer rules already carry the boundary the user pinned; name it rather than
        // saying "the pinned layer", which tells an agent nothing.
        let layer = infer_rule("Keep it server-side only").expect("a rule");
        assert_eq!(describe(&layer).0, "Work must stay server-side only");
    }

    #[test]
    fn a_label_falls_back_rather_than_lying_when_the_path_cannot_be_recovered() {
        use crate::baseline::Rule;
        // A hand-written or future pattern that doesn't match the shape `infer_rules`
        // produces must degrade to the generic label, never to a wrong filename.
        let odd = Rule::ForbidPattern {
            pattern: "diff --git something else entirely".into(),
        };
        assert_eq!(describe(&odd).0, "A protected file must not be modified");
    }

    #[test]
    fn literal_between_is_an_exact_inverse_of_regex_escape() {
        // Round-trip the paths that actually occur, including the dotted and hyphenated
        // ones where escaping matters.
        for path in [
            "package.json",
            "package-lock.json",
            "src/legacy/index.ts",
            "a+b.rs",
        ] {
            let pattern = format!(
                r"(?m)^(?:diff --git |\+\+\+ |--- )[ab]/(?:\S*/)?{}\b",
                regex::escape(path)
            );
            assert_eq!(
                literal_between(&pattern, r"[ab]/(?:\S*/)?", r"\b").as_deref(),
                Some(path),
                "failed to recover {path}"
            );
        }
    }
}
