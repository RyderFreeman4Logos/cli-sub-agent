//! User-facing error hints for common CLI failures.
//!
//! Inspired by xurl's `user_facing_error()` pattern: match on error type/message
//! and return actionable fix suggestions with concrete commands.

use std::path::Path;

use anyhow::Error;
use csa_core::error::AppError;

const HINT_INSTALL_GEMINI: &str = "hint: install gemini-cli: npm install -g @google/gemini-cli";
const HINT_INSTALL_CODEX: &str = "hint: install codex CLI: npm install -g @openai/codex";
const HINT_INSTALL_CODEX_ACP: &str =
    "hint: install codex ACP adapter: npm install -g @zed-industries/codex-acp";
const HINT_INSTALL_CLAUDE: &str =
    "hint: install claude-code ACP adapter: npm install -g @zed-industries/claude-code-acp";
const HINT_INSTALL_OPENCODE: &str =
    "hint: install opencode: go install github.com/opencode-ai/opencode@latest";
const HINT_RATE_LIMIT: &str = "hint: wait and retry, or try a different tool: csa run --sa-mode <true|false> --tool <alternative> ...";
const HINT_SLOT_EXHAUSTED: &str = "hint: free slots with 'csa gc' or wait with '--wait' flag";
const HINT_SESSION_NOT_FOUND: &str = "hint: list available sessions with 'csa session list'";
const HINT_CONFIG_ERROR: &str =
    "hint: validate config with 'csa config validate' or reinitialize with 'csa init'";
const HINT_GEMINI_RUNTIME_HOME: &str = "hint: Gemini ACP needs a writable runtime home; current builds pin TMPDIR to a writable sandbox temp dir (private /tmp in bwrap, session tmp elsewhere) and seed under CSA session state, but older builds may still need re-run with TMPDIR=/tmp";
const SANDBOX_FS_DENIAL_MARKERS: [&str; 7] = [
    "read-only file system",
    "permission denied",
    "operation not permitted",
    "eacces",
    "eperm",
    "errno 13",
    "errno 30",
];

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
        if chain_text.contains("codex-acp") {
            return Some(HINT_INSTALL_CODEX_ACP.to_string());
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

    if chain_text.contains("failed to create gemini runtime dir")
        || (chain_text.contains("gemini runtime dir")
            && chain_text.contains("read-only file system"))
    {
        return Some(HINT_GEMINI_RUNTIME_HOME.to_string());
    }

    None
}

pub(crate) fn sandbox_fs_denial_hint(
    stderr: &str,
    stdout: &str,
    session_id: &str,
) -> Option<String> {
    let combined_lower = format!("{stderr}\n{stdout}").to_ascii_lowercase();
    if !SANDBOX_FS_DENIAL_MARKERS
        .iter()
        .any(|marker| combined_lower.contains(marker))
    {
        return None;
    }

    let path_hint = extract_denied_path(stderr)
        .or_else(|| extract_denied_path(stdout))
        .map(|path| {
            let suggested = Path::new(&path)
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map(|parent| parent.display().to_string())
                .unwrap_or(path);
            format!("--extra-writable {suggested}")
        })
        .unwrap_or_else(|| "--extra-writable /path/to/needed/dir".to_string());

    Some(format!(
        "hint: sandbox filesystem write denied. To continue from this session's partial work rather than re-running from scratch, run:\n  csa run --fork-from {session_id} {path_hint} --prompt-file <continuation prompt>"
    ))
}

pub(crate) fn append_sandbox_fs_denial_hint(
    stderr_output: &mut String,
    stdout: &str,
    session_id: &str,
) {
    let Some(hint) = sandbox_fs_denial_hint(stderr_output, stdout, session_id) else {
        return;
    };
    if stderr_output.contains(&hint) {
        return;
    }
    if !stderr_output.is_empty() && !stderr_output.ends_with('\n') {
        stderr_output.push('\n');
    }
    stderr_output.push_str(&hint);
    stderr_output.push('\n');
}

fn extract_denied_path(text: &str) -> Option<String> {
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        if !SANDBOX_FS_DENIAL_MARKERS
            .iter()
            .take(3)
            .any(|marker| lower.contains(marker))
        {
            continue;
        }

        if let Some(path) = extract_quoted_path(line, ": '", '\'') {
            return Some(path);
        }
        if let Some(path) = extract_quoted_path(line, ": \"", '"') {
            return Some(path);
        }
    }
    None
}

fn extract_quoted_path(line: &str, marker: &str, quote: char) -> Option<String> {
    let start = line.rfind(marker)?;
    let rest = &line[start + marker.len()..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

fn tool_install_hint(tool_name: &str) -> Option<&'static str> {
    let tool = tool_name.to_lowercase();
    if tool.contains("codex-acp") {
        return Some(HINT_INSTALL_CODEX_ACP);
    }
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
        let hint = suggest_fix(&err).unwrap();
        assert!(
            hint.contains("@openai/codex"),
            "should mention the default codex CLI: {hint}"
        );
    }

    #[test]
    fn test_tool_not_installed_claude() {
        let err = anyhow::anyhow!("claude-code is not installed");
        let hint = suggest_fix(&err).unwrap();
        assert!(
            hint.contains("claude-code-acp"),
            "should mention ACP adapter: {hint}"
        );
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
    fn test_gemini_runtime_home_hint() {
        let err = anyhow::anyhow!(
            "failed to create gemini runtime dir /home/obj/.claude/tmp/cli-sub-agent-gemini/01ABC/.gemini: Read-only file system (os error 30)"
        );
        assert_eq!(suggest_fix(&err).as_deref(), Some(HINT_GEMINI_RUNTIME_HOME));
    }

    #[test]
    fn test_no_hint_for_generic_error() {
        let err = anyhow::anyhow!("something failed");
        assert_eq!(suggest_fix(&err), None);
    }

    #[test]
    fn sandbox_fs_denial_hint_fires_on_read_only_error() {
        let stderr = "OSError: [Errno 30] Read-only file system: '/home/obj/.claude-mem/settings.json.tmp.1234'";
        let hint = sandbox_fs_denial_hint(stderr, "", "01KTEST123").unwrap();
        assert!(hint.contains("--fork-from 01KTEST123"), "got: {hint}");
        assert!(
            hint.contains("--extra-writable /home/obj/.claude-mem"),
            "got: {hint}"
        );
        assert!(hint.contains("--prompt-file"), "got: {hint}");
    }

    #[test]
    fn sandbox_fs_denial_hint_fires_on_permission_denied() {
        let stderr = "PermissionError: [Errno 13] Permission denied: '/etc/foo'";
        let hint = sandbox_fs_denial_hint(stderr, "", "01KTEST123").unwrap();
        assert!(hint.contains("--extra-writable /etc"), "got: {hint}");
    }

    #[test]
    fn sandbox_fs_denial_hint_is_none_for_unrelated_failure() {
        let stderr = "Error: connection refused";
        let hint = sandbox_fs_denial_hint(stderr, "", "01KTEST123");
        assert!(hint.is_none());
    }

    #[test]
    fn sandbox_fs_denial_hint_generic_path_fallback_when_no_parse() {
        let stderr = "bash: cannot create file: Read-only file system";
        let hint = sandbox_fs_denial_hint(stderr, "", "01KTEST123").unwrap();
        assert!(
            hint.contains("--extra-writable /path/to/needed/dir"),
            "got: {hint}"
        );
    }
}
