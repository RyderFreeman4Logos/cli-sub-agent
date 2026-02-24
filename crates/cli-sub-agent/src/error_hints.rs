//! User-facing error hints for common CLI failures.
//!
//! Inspired by xurl's `user_facing_error()` pattern: match on error type/message
//! and return actionable fix suggestions with concrete commands.

use anyhow::Error;
use csa_core::error::AppError;

const HINT_INSTALL_GEMINI: &str = "hint: install gemini-cli: npm install -g @google/gemini-cli";
const HINT_INSTALL_CODEX: &str = "hint: install codex: npm install -g @openai/codex";
const HINT_INSTALL_CLAUDE: &str =
    "hint: install claude-code: npm install -g @anthropic-ai/claude-code";
const HINT_INSTALL_OPENCODE: &str =
    "hint: install opencode: go install github.com/opencode-ai/opencode@latest";
const HINT_RATE_LIMIT: &str =
    "hint: wait and retry, or try a different tool: csa run --tool <alternative> ...";
const HINT_SLOT_EXHAUSTED: &str = "hint: free slots with 'csa gc' or wait with '--wait' flag";
const HINT_SESSION_NOT_FOUND: &str = "hint: list available sessions with 'csa session list'";
const HINT_CONFIG_ERROR: &str =
    "hint: validate config with 'csa config validate' or reinitialize with 'csa init'";

pub fn suggest_fix(err: &Error) -> Option<String> {
    for cause in err.chain() {
        if let Some(app_err) = cause.downcast_ref::<AppError>() {
            match app_err {
                AppError::ToolNotInstalled(tool) => {
                    if let Some(hint) = tool_install_hint(tool) {
                        return Some(hint.to_string());
                    }
                }
                AppError::RateLimited { .. } => return Some(HINT_RATE_LIMIT.to_string()),
                AppError::SlotExhausted { .. } => return Some(HINT_SLOT_EXHAUSTED.to_string()),
                AppError::SessionNotFound(_) => return Some(HINT_SESSION_NOT_FOUND.to_string()),
                _ => {}
            }
        }
    }

    let chain_text = err
        .chain()
        .map(|cause| cause.to_string().to_lowercase())
        .collect::<Vec<_>>()
        .join(" | ");

    let has_not_installed_or_not_found =
        chain_text.contains("not installed") || chain_text.contains("not found");

    if has_not_installed_or_not_found {
        if chain_text.contains("gemini") {
            return Some(HINT_INSTALL_GEMINI.to_string());
        }
        if chain_text.contains("codex") {
            return Some(HINT_INSTALL_CODEX.to_string());
        }
        if chain_text.contains("claude") {
            return Some(HINT_INSTALL_CLAUDE.to_string());
        }
        if chain_text.contains("opencode") {
            return Some(HINT_INSTALL_OPENCODE.to_string());
        }
    }

    if chain_text.contains("rate limit")
        || chain_text.contains("429")
        || chain_text.contains("quota")
    {
        return Some(HINT_RATE_LIMIT.to_string());
    }

    if chain_text.contains("slot")
        && (chain_text.contains("exhaust") || chain_text.contains("occupied"))
    {
        return Some(HINT_SLOT_EXHAUSTED.to_string());
    }

    if chain_text.contains("session") && chain_text.contains("not found") {
        return Some(HINT_SESSION_NOT_FOUND.to_string());
    }

    if chain_text.contains("config")
        && (chain_text.contains("invalid")
            || chain_text.contains("missing")
            || chain_text.contains("parse"))
    {
        return Some(HINT_CONFIG_ERROR.to_string());
    }

    None
}

fn tool_install_hint(tool_name: &str) -> Option<&'static str> {
    let tool = tool_name.to_lowercase();
    if tool.contains("gemini") {
        return Some(HINT_INSTALL_GEMINI);
    }
    if tool.contains("codex") {
        return Some(HINT_INSTALL_CODEX);
    }
    if tool.contains("claude") {
        return Some(HINT_INSTALL_CLAUDE);
    }
    if tool.contains("opencode") {
        return Some(HINT_INSTALL_OPENCODE);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_not_installed_gemini() {
        let err = Error::new(AppError::ToolNotInstalled("gemini-cli".into()));
        assert_eq!(suggest_fix(&err).as_deref(), Some(HINT_INSTALL_GEMINI));
    }

    #[test]
    fn test_tool_not_installed_codex() {
        let err = anyhow::anyhow!("tool codex not found");
        assert_eq!(suggest_fix(&err).as_deref(), Some(HINT_INSTALL_CODEX));
    }

    #[test]
    fn test_tool_not_installed_claude() {
        let err = anyhow::anyhow!("claude-code is not installed");
        assert_eq!(suggest_fix(&err).as_deref(), Some(HINT_INSTALL_CLAUDE));
    }

    #[test]
    fn test_tool_not_installed_opencode() {
        let err = Error::new(AppError::ToolNotInstalled("opencode".into()));
        assert_eq!(suggest_fix(&err).as_deref(), Some(HINT_INSTALL_OPENCODE));
    }

    #[test]
    fn test_rate_limit_hint() {
        let err = Error::new(AppError::RateLimited {
            tool: "codex".into(),
            message: "429 Too Many Requests".into(),
        });
        assert_eq!(suggest_fix(&err).as_deref(), Some(HINT_RATE_LIMIT));
    }

    #[test]
    fn test_slot_exhausted_hint() {
        let err = Error::new(AppError::SlotExhausted {
            tool: "codex".into(),
            max: 2,
            alternatives: vec![],
        });
        assert_eq!(suggest_fix(&err).as_deref(), Some(HINT_SLOT_EXHAUSTED));
    }

    #[test]
    fn test_session_not_found_hint() {
        let err = Error::new(AppError::SessionNotFound("01ARZ".into()));
        assert_eq!(suggest_fix(&err).as_deref(), Some(HINT_SESSION_NOT_FOUND));
    }

    #[test]
    fn test_config_error_hint() {
        let err = anyhow::anyhow!("config parse error: invalid value");
        assert_eq!(suggest_fix(&err).as_deref(), Some(HINT_CONFIG_ERROR));
    }

    #[test]
    fn test_no_hint_for_generic_error() {
        let err = anyhow::anyhow!("something failed");
        assert_eq!(suggest_fix(&err), None);
    }
}
