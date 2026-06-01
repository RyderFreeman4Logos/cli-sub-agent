//! Environment-variable reservation list for ACP child-process spawns.

/// Environment variables stripped before spawning ACP child processes.
///
/// These are set by the parent Claude Code instance (or leak in from the
/// ambient shell) and interfere with the child ACP adapter or the tool it
/// wraps. Backing list for [`super::AcpConnection::STRIPPED_ENV_VARS`].
pub(crate) const STRIPPED_ENV_VARS: &[&str] = &[
    // Claude Code sets this to detect recursive invocations.  When inherited by
    // a child claude-code-acp → claude-code chain, the child refuses to start.
    "CLAUDECODE",
    // Entrypoint tracking for the parent session — not meaningful for the ACP
    // subprocess.
    "CLAUDE_CODE_ENTRYPOINT",
    // Gemini auth/routing must be controlled by CSA retry state so each fresh
    // invocation still starts on the quota-backed path.
    "GEMINI_API_KEY",
    "GOOGLE_GEMINI_BASE_URL",
    // Lefthook hook-bypass env vars must never leak into child tool processes.
    // If the parent process has these set (e.g. from a user's shell), the child
    // tool would silently skip pre-commit hooks, violating AGENTS.md rule 029.
    "LEFTHOOK",
    "LEFTHOOK_SKIP",
    // Subtree model-pin context vars (csa_core::env::SUBTREE_PIN_ENV_KEYS).
    // These are CSA-owned: an ambient value (leaked into a shell or set by a
    // wrapper) must never be honored as a subtree pin, or it would silently pin
    // an otherwise-unpinned nested worker and drop tier routing (#1741). CSA
    // re-injects the legit pin via the effective_env (unfiltered) only when the
    // parent was explicitly --model-spec-pinned. String literals (not the
    // csa_core constants) because csa-acp does not depend on csa-core; presence
    // guarded by the strips_subtree_pin_env_vars test.
    "CSA_MODEL_SPEC",
    "CSA_FORCE_IGNORE_TIER_SETTING",
    "CSA_NO_FAILOVER",
];
