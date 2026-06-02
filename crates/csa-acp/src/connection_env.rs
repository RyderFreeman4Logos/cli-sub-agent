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
    // The startup subtree contract is scrubbed through csa_core::env so the
    // contract key list has one source of truth (#1750).
];
