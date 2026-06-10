//! Default grounding / persona for the lifed chat agent.
//!
//! ## Why this exists
//!
//! The transitional [`crate::VercelAiGatewayArcan`] /
//! [`crate::AnthropicArcan`] backends are bare single-completion bridges.
//! Without a system prompt the live agent answers *"Who is Carlos?"* /
//! *"What is broomva.tech?"* with *"I don't have enough context."* — there
//! is no harness, no tools, and no retrieval on the chat path yet.
//!
//! The proper, retrieval-backed grounding (lago-knowledge search + the
//! arcan-core context compiler) lives **behind the arcand gRPC boundary**,
//! NOT in lifed: `scripts/verify_dependencies_lifed.sh` forbids `lifed`
//! and the `*-proxy` crates from depending on `arcan-core`, `arcan-lago`,
//! `lago-knowledge`, or `arcan-praxis`. So a context-compiler dependency
//! is not an option here — that is the next arc, gated by arcand
//! publishing a tool-capable agent service.
//!
//! What *is* dependency-clean and ships value today is a source-controlled,
//! embedded default persona compiled straight into the binary: a static
//! string, zero new crate dependencies. It grounds the agent in the
//! verifiable facts the repo already publishes about itself
//! (`llms.txt`, `.well-known/agent.json`, `README.md`).
//!
//! ## Precedence
//!
//! Operators keep full control. [`resolve_system_prompt`] returns:
//! 1. the `LIFED_ARCAN_SYSTEM_PROMPT` env var if it is set and non-empty
//!    (an explicit operator override — wins wholesale), otherwise
//! 2. [`DEFAULT_CHAT_SYSTEM_PROMPT`] (the grounded baseline).
//!
//! It always returns `Some` (the default is never empty), so the backends'
//! existing `Option<String>` plumbing is unchanged — an unset env var now
//! yields a grounded agent instead of a generic one.
//!
//! **Deploy note:** a deployment that currently sets
//! `LIFED_ARCAN_SYSTEM_PROMPT` to a generic value (e.g. *"You are a helpful
//! AI assistant."*) must **clear that env var** to adopt this grounded
//! default — an explicit override still wins by design.

/// Env var operators set to override [`DEFAULT_CHAT_SYSTEM_PROMPT`].
pub const SYSTEM_PROMPT_ENV: &str = "LIFED_ARCAN_SYSTEM_PROMPT";

/// The embedded default system prompt for the chat agent.
///
/// Source-controlled at `arcan-proxy/assets/chat_agent_persona.md` and
/// compiled into the binary via `include_str!`. Grounds the agent in the
/// project's own canonical self-description so it can answer FAQs about
/// Broomva, Carlos Escobar-Valbuena, and the Life Agent OS factually
/// instead of replying *"I don't have enough context."*
pub const DEFAULT_CHAT_SYSTEM_PROMPT: &str = include_str!("../assets/chat_agent_persona.md");

/// Resolve the system prompt for a chat dispatch from the process
/// environment. See [`resolve_system_prompt_from`] for the precedence
/// rule; this is the thin env-reading wrapper the backends call from
/// `from_env`.
pub fn resolve_system_prompt() -> Option<String> {
    resolve_system_prompt_from(std::env::var(SYSTEM_PROMPT_ENV).ok())
}

/// Pure core of [`resolve_system_prompt`] — testable without touching the
/// process environment.
///
/// Returns the operator override when it is present and non-blank,
/// otherwise the grounded [`DEFAULT_CHAT_SYSTEM_PROMPT`]. Never returns
/// `None`: there is always a grounded baseline.
pub fn resolve_system_prompt_from(override_value: Option<String>) -> Option<String> {
    match override_value {
        Some(s) if !s.trim().is_empty() => Some(s),
        _ => Some(DEFAULT_CHAT_SYSTEM_PROMPT.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The embedded default must be non-empty and ground the agent in the
    /// facts that make the failing FAQ probes pass — the anchors here are
    /// exactly the entities the live agent could not answer about before
    /// this change ("Who is Carlos?" / "What is broomva.tech?").
    #[test]
    fn default_prompt_is_nonempty_and_grounded() {
        let p = DEFAULT_CHAT_SYSTEM_PROMPT;
        assert!(!p.trim().is_empty(), "default prompt must not be empty");
        for anchor in [
            "Life Agent OS",
            "broomva.tech",
            "Carlos",
            "Escobar-Valbuena",
            "Arcan",
            "Lago",
            "https://github.com/broomva/life",
        ] {
            assert!(
                p.contains(anchor),
                "default grounding prompt is missing the `{anchor}` anchor",
            );
        }
    }

    /// The default must be honest about not having tool/sandbox access on
    /// the chat surface yet — guards against a future edit that overclaims.
    #[test]
    fn default_prompt_is_honest_about_capabilities() {
        let p = DEFAULT_CHAT_SYSTEM_PROMPT;
        assert!(
            p.contains("not yet wired into this chat surface"),
            "default prompt must stay honest about current (no-tools) capabilities",
        );
    }

    #[test]
    fn resolve_prefers_non_blank_override() {
        let got = resolve_system_prompt_from(Some("custom operator prompt".to_string()));
        assert_eq!(got.as_deref(), Some("custom operator prompt"));
    }

    #[test]
    fn resolve_ignores_blank_override() {
        // Empty and whitespace-only overrides fall back to the grounded
        // default — an operator who sets `LIFED_ARCAN_SYSTEM_PROMPT=""`
        // (or to spaces) should not silently un-ground the agent.
        for blank in ["", "   ", "\n\t "] {
            let got = resolve_system_prompt_from(Some(blank.to_string()));
            assert_eq!(
                got.as_deref(),
                Some(DEFAULT_CHAT_SYSTEM_PROMPT),
                "blank override `{blank:?}` must fall back to the grounded default",
            );
        }
    }

    #[test]
    fn resolve_defaults_when_unset() {
        let got = resolve_system_prompt_from(None);
        assert_eq!(got.as_deref(), Some(DEFAULT_CHAT_SYSTEM_PROMPT));
    }
}
