//! Known upstream providers, so users can plug Drifterr straight into the major
//! AI providers — not only OpenRouter.
//!
//! All of these except Anthropic speak the OpenAI schema, so a tool pointed at
//! the proxy with `OPENAI_BASE_URL=http://localhost:8787/v1` relays straight
//! through; only the upstream host changes. Google Gemini is the one exception:
//! its OpenAI-compatible endpoint lives under `/v1beta/openai`, so we strip the
//! incoming `/v1` and join onto that prefix.

/// A selectable provider preset.
pub struct Preset {
    /// Stable id used by `DRIFTERR_PROVIDER`.
    pub id: &'static str,
    /// Human label for the UI / `/config`.
    pub label: &'static str,
    /// OpenAI-schema upstream base (empty = leave the current default).
    pub openai_base: &'static str,
    /// Strip the incoming `/v1` before joining (Gemini's `/v1beta/openai`).
    pub openai_strip_v1: bool,
    /// Anthropic-schema upstream base (empty = leave the default).
    pub anthropic_base: &'static str,
}

/// The built-in providers you can switch to with `DRIFTERR_PROVIDER=<id>`.
pub const PRESETS: &[Preset] = &[
    Preset {
        id: "openrouter",
        label: "OpenRouter",
        openai_base: "https://openrouter.ai/api",
        openai_strip_v1: false,
        anthropic_base: "",
    },
    Preset {
        id: "openai",
        label: "OpenAI",
        openai_base: "https://api.openai.com",
        openai_strip_v1: false,
        anthropic_base: "",
    },
    Preset {
        id: "anthropic",
        label: "Anthropic",
        openai_base: "",
        openai_strip_v1: false,
        anthropic_base: "https://api.anthropic.com",
    },
    Preset {
        id: "gemini",
        label: "Google Gemini",
        openai_base: "https://generativelanguage.googleapis.com/v1beta/openai",
        openai_strip_v1: true,
        anthropic_base: "",
    },
    Preset {
        id: "groq",
        label: "Groq",
        openai_base: "https://api.groq.com/openai",
        openai_strip_v1: false,
        anthropic_base: "",
    },
    Preset {
        id: "mistral",
        label: "Mistral",
        openai_base: "https://api.mistral.ai",
        openai_strip_v1: false,
        anthropic_base: "",
    },
    Preset {
        id: "deepseek",
        label: "DeepSeek",
        openai_base: "https://api.deepseek.com",
        openai_strip_v1: false,
        anthropic_base: "",
    },
    Preset {
        id: "xai",
        label: "xAI (Grok)",
        openai_base: "https://api.x.ai",
        openai_strip_v1: false,
        anthropic_base: "",
    },
    Preset {
        id: "together",
        label: "Together",
        openai_base: "https://api.together.xyz",
        openai_strip_v1: false,
        anthropic_base: "",
    },
];

/// Find a preset by id (case-insensitive).
pub fn find(id: &str) -> Option<&'static Preset> {
    let id = id.trim();
    PRESETS.iter().find(|p| p.id.eq_ignore_ascii_case(id))
}

/// A friendly label for an OpenAI-schema base URL (for `/config` / the UI).
/// Falls back to "Custom" for anything not in the registry.
pub fn label_for_base(base: &str) -> &'static str {
    by_base(base).map(|p| p.label).unwrap_or("Custom")
}

/// The preset id for an OpenAI-schema base URL ("custom" if unknown).
pub fn id_for_base(base: &str) -> &'static str {
    by_base(base).map(|p| p.id).unwrap_or("custom")
}

fn by_base(base: &str) -> Option<&'static Preset> {
    let base = base.trim_end_matches('/');
    PRESETS
        .iter()
        .find(|p| !p.openai_base.is_empty() && p.openai_base.trim_end_matches('/') == base)
}

/// Build the final upstream URL. With `strip_v1`, the incoming `/v1` prefix is
/// removed so the path joins onto a base that already carries its own prefix
/// (Gemini). Otherwise it's a straight base + path join.
pub fn join_url(base: &str, path_and_query: &str, strip_v1: bool) -> String {
    let base = base.trim_end_matches('/');
    if strip_v1 {
        if let Some(rest) = path_and_query.strip_prefix("/v1") {
            return format!("{base}{rest}");
        }
    }
    format!("{base}{path_and_query}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_is_case_insensitive() {
        assert_eq!(find("OpenAI").unwrap().id, "openai");
        assert_eq!(find(" gemini ").unwrap().id, "gemini");
        assert!(find("nope").is_none());
    }

    #[test]
    fn standard_providers_pass_the_path_through() {
        assert_eq!(
            join_url("https://api.openai.com", "/v1/chat/completions", false),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            join_url("https://api.groq.com/openai", "/v1/chat/completions", false),
            "https://api.groq.com/openai/v1/chat/completions"
        );
    }

    #[test]
    fn gemini_strips_v1_onto_its_openai_prefix() {
        let g = find("gemini").unwrap();
        assert!(g.openai_strip_v1);
        assert_eq!(
            join_url(g.openai_base, "/v1/chat/completions", g.openai_strip_v1),
            "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions"
        );
    }

    #[test]
    fn labels_resolve_known_and_custom() {
        assert_eq!(label_for_base("https://openrouter.ai/api"), "OpenRouter");
        assert_eq!(label_for_base("https://api.openai.com/"), "OpenAI");
        assert_eq!(label_for_base("https://my.local.llm"), "Custom");
    }
}
