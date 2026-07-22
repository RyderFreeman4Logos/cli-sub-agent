//! User-facing error hints for common CLI failures.
//!
//! Inspired by xurl's `user_facing_error()` pattern: match on error type/message
//! and return actionable fix suggestions with concrete commands.

use std::path::Path;

use anyhow::Error;
use csa_core::error::AppError;

const HINT_REMOVED_GEMINI: &str = "hint: gemini-cli support has been removed because the provider is discontinued; use codex or claude-code instead";
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
const HINT_SKILL_NOT_FOUND: &str = "hint: list runnable skills with 'csa skill list' or install one with 'csa skill install <repo>'";
const HINT_GEMINI_RUNTIME_HOME: &str = HINT_REMOVED_GEMINI;
const LEFTHOOK_CORE_HOOKSPATH_CONFLICT: &str = "core.hooksPath is set locally";
const HINT_LEFTHOOK_CORE_HOOKSPATH_CONFLICT: &str = "hint: lefthook blocked git commit because core.hooksPath is set locally. Staged work may be uncommitted. Run `git config --unset-all --local core.hooksPath`, then rerun the commit or continue the session.";
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

    if is_exact_session_registry_loss_diagnostic(&chain_text) {
        return None;
    }

    let has_not_installed_or_not_found =
        chain_text.contains("not installed") || chain_text.contains("not found");

    if chain_text.contains("skill '") && chain_text.contains("not found") {
        return Some(HINT_SKILL_NOT_FOUND.to_string());
    }

    if has_not_installed_or_not_found {
        if chain_text.contains("gemini") {
            return Some(HINT_REMOVED_GEMINI.to_string());
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

fn is_exact_session_registry_loss_diagnostic(chain_text: &str) -> bool {
    chain_text.contains("session registry lookup failed")
        || chain_text.contains("csa infrastructure session-registry loss")
        || chain_text.contains("csa:session_started")
}

pub(crate) fn sandbox_fs_denial_hint(
    stderr: &str,
    _stdout: &str,
    fs_sandbox_active: bool,
    session_id: &str,
) -> Option<String> {
    if !fs_sandbox_active {
        return None;
    }

    let denied_path = extract_denied_path(stderr)?;
    let suggested = Path::new(&denied_path)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| parent.display().to_string())
        .unwrap_or_else(|| denied_path.clone());
    let path_hint = format!("--extra-writable {suggested}");

    Some(format!(
        "hint: sandbox filesystem write denied for {denied_path}. To continue from this session's partial work rather than re-running from scratch, run:\n  csa run --fork-from {session_id} {path_hint} --prompt-file CONTINUATION_PROMPT.md"
    ))
}

pub(crate) fn append_sandbox_fs_denial_hint(
    stderr_output: &mut String,
    stdout: &str,
    fs_sandbox_active: bool,
    session_id: &str,
) {
    let Some(hint) = sandbox_fs_denial_hint(stderr_output, stdout, fs_sandbox_active, session_id)
    else {
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

pub(crate) fn lefthook_core_hookspath_conflict_hint(
    stderr: &str,
    stdout: &str,
) -> Option<&'static str> {
    // Only emit the hint when a commit was actually attempted and hooksPath
    // is the concrete failure explanation (#2055, #2729).
    //
    // Do NOT treat `git config --unset-all --local core.hooksPath` alone as
    // commit evidence: that string is part of lefthook's diagnostic template
    // and can appear (or be paraphrased) without any commit attempt, producing
    // false session-wait warnings on non-commit runs with empty hooksPath.
    let has_hookspath = stderr.contains(LEFTHOOK_CORE_HOOKSPATH_CONFLICT)
        || stdout.contains(LEFTHOOK_CORE_HOOKSPATH_CONFLICT);
    if !has_hookspath {
        return None;
    }
    let combined = format!("{}\n{}", stderr, stdout).to_ascii_lowercase();
    let has_commit_context = combined.contains("git commit")
        || combined.contains("lefthook run")
        || combined.contains("pre-commit")
        || combined.contains("commit-msg");
    if has_commit_context {
        return Some(HINT_LEFTHOOK_CORE_HOOKSPATH_CONFLICT);
    }
    None
}

fn extract_denied_path(text: &str) -> Option<String> {
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        if !SANDBOX_FS_DENIAL_MARKERS
            .iter()
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
        if let Some(path) = extract_unquoted_absolute_path(line) {
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

fn extract_unquoted_absolute_path(line: &str) -> Option<String> {
    let start = line.find('/')?;
    let rest = &line[start..];
    let end = rest
        .find(|ch: char| {
            ch.is_whitespace() || matches!(ch, ':' | ',' | ';' | ')' | ']' | '"' | '\'')
        })
        .unwrap_or(rest.len());
    let path = rest[..end].trim_end_matches('.');
    if path.is_empty() || path == "/" {
        None
    } else {
        Some(path.to_string())
    }
}

fn tool_install_hint(tool_name: &str) -> Option<&'static str> {
    let tool = tool_name.to_lowercase();
    if tool.contains("codex-acp") {
        return Some(HINT_INSTALL_CODEX_ACP);
    }
    if tool.contains("gemini") {
        return Some(HINT_REMOVED_GEMINI);
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
        let hint = suggest_fix(&err).expect("removed gemini should produce a hint");
        assert!(hint.contains("support has been removed"), "{hint}");
        assert!(!hint.contains("npm install"), "{hint}");
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
    fn test_skill_not_found_with_codex_name_does_not_suggest_codex_install() {
        let err = anyhow::anyhow!(
            "Skill 'mktsk-codex' not found. Searched:\n  - /project/.codex/skills/mktsk-codex"
        );
        let hint = suggest_fix(&err).unwrap();
        assert!(hint.contains("csa skill list"), "got: {hint}");
        assert!(
            !hint.contains("@openai/codex"),
            "missing skill must not suggest installing Codex CLI: {hint}"
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
    fn exact_session_registry_loss_does_not_suggest_session_list() {
        let err = anyhow::anyhow!(
            "session registry lookup failed for session '01ARZ3NDEKTSV4RRFFQ69G5FAV': no session registration was found. If this id came from CSA:SESSION_STARTED, this is CSA infrastructure session-registry loss, not a product-code failure."
        );
        assert_eq!(suggest_fix(&err), None);
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
    fn lefthook_core_hookspath_conflict_hint_detects_commit_failure_output() {
        let stderr = r#"
Error: core.hooksPath is set locally to '/x/.git/hooks'
hint: Unset it:
hint:   git config --unset-all --local core.hooksPath
git commit failed during pre-commit
"#;
        let hint = lefthook_core_hookspath_conflict_hint(stderr, "").unwrap();
        assert!(
            hint.contains("git config --unset-all --local core.hooksPath"),
            "got: {hint}"
        );
        assert!(
            hint.contains("Staged work may be uncommitted"),
            "got: {hint}"
        );
    }

    #[test]
    fn lefthook_core_hookspath_conflict_hint_ignores_unrelated_commit_failure() {
        let stderr = "error: Recipe `clippy` failed on line 12 with exit code 1";
        let hint = lefthook_core_hookspath_conflict_hint(stderr, "");
        assert!(hint.is_none(), "got: {hint:?}");
    }

    #[test]
    fn lefthook_core_hookspath_conflict_hint_suppressed_without_commit_context() {
        // Session output mentions hooksPath but no commit attempt — should NOT emit hint (#2055)
        let stderr = "core.hooksPath is set locally\noutput generated successfully";
        let hint = lefthook_core_hookspath_conflict_hint(stderr, "");
        assert!(
            hint.is_none(),
            "hint should be suppressed when no commit context exists"
        );
    }

    #[test]
    fn lefthook_core_hookspath_conflict_hint_suppressed_with_unset_template_only() {
        // Lefthook diagnostic text includes the unset-all template without any
        // actual commit attempt — must NOT emit the session-wait hint (#2729).
        let stderr = "Error: core.hooksPath is set locally to '/x/.git/hooks'\nhint: Unset it:\nhint:   git config --unset-all --local core.hooksPath\n";
        let hint = lefthook_core_hookspath_conflict_hint(stderr, "");
        assert!(
            hint.is_none(),
            "hint must not fire for hooksPath diagnostic template alone (#2729)"
        );

        // And stderr with just "Unset it" without commit context should still not fire
        let stderr2 = "core.hooksPath is set locally\nhint: Unset it\nsome other output";
        let hint2 = lefthook_core_hookspath_conflict_hint(stderr2, "");
        assert!(
            hint2.is_none(),
            "hint should be suppressed for generic 'Unset it' without commit command"
        );
    }

    #[test]
    fn lefthook_core_hookspath_conflict_hint_suppressed_on_non_commit_run_noise() {
        // Non-commit run: hooksPath diagnostic phrase with zero staged work and
        // no commit/hook invocation evidence — session wait must stay quiet (#2729).
        let stderr = "core.hooksPath is set locally\noutput generated successfully\n0 staged paths";
        let hint = lefthook_core_hookspath_conflict_hint(stderr, "");
        assert!(
            hint.is_none(),
            "non-commit noise must not force a false positive"
        );
        // Positive control: real commit attempt still fires.
        let real = lefthook_core_hookspath_conflict_hint(
            "Error: core.hooksPath is set locally to '/x/.git/hooks'\npre-commit hook failed\n",
            "",
        );
        assert!(real.is_some(), "pre-commit context should emit the hint");
    }

    #[test]
    fn sandbox_fs_denial_hint_fires_on_read_only_error() {
        let stderr = "OSError: [Errno 30] Read-only file system: '/home/obj/.claude-mem/settings.json.tmp.1234'";
        let hint = sandbox_fs_denial_hint(stderr, "", true, "01KTEST123").unwrap();
        assert!(hint.contains("--fork-from 01KTEST123"), "got: {hint}");
        assert!(
            hint.contains("denied for /home/obj/.claude-mem/settings.json.tmp.1234"),
            "got: {hint}"
        );
        assert!(
            hint.contains("--extra-writable /home/obj/.claude-mem"),
            "got: {hint}"
        );
        assert!(hint.contains("--prompt-file"), "got: {hint}");
    }

    #[test]
    fn sandbox_fs_denial_hint_fires_on_permission_denied() {
        let stderr = "PermissionError: [Errno 13] Permission denied: '/etc/foo'";
        let hint = sandbox_fs_denial_hint(stderr, "", true, "01KTEST123").unwrap();
        assert!(hint.contains("denied for /etc/foo"), "got: {hint}");
        assert!(hint.contains("--extra-writable /etc"), "got: {hint}");
    }

    #[test]
    fn sandbox_fs_denial_hint_fires_on_unquoted_bwrap_path() {
        let stderr =
            "bwrap: Can't mkdir parents for /home/obj/.cache/tool/state: Read-only file system";
        let hint = sandbox_fs_denial_hint(stderr, "", true, "01KTEST123").unwrap();
        assert!(
            hint.contains("denied for /home/obj/.cache/tool/state"),
            "got: {hint}"
        );
        assert!(
            hint.contains("--extra-writable /home/obj/.cache/tool"),
            "got: {hint}"
        );
    }

    #[test]
    fn sandbox_fs_denial_hint_suppressed_when_sandbox_inactive() {
        let stderr = "open(/etc/foo): Permission denied";
        let hint = sandbox_fs_denial_hint(stderr, "", false, "01KTEST123");
        assert!(hint.is_none(), "got: {hint:?}");
    }

    #[test]
    fn sandbox_fs_denial_hint_suppressed_without_denial_text() {
        let stderr = "Error: connection refused";
        let hint = sandbox_fs_denial_hint(stderr, "", true, "01KTEST123");
        assert!(hint.is_none());
    }

    #[test]
    fn sandbox_fs_denial_hint_suppressed_without_denied_path() {
        let stderr = "bash: cannot create file: Read-only file system";
        let hint = sandbox_fs_denial_hint(stderr, "", true, "01KTEST123");
        assert!(hint.is_none(), "got: {hint:?}");
    }

    #[test]
    fn sandbox_fs_denial_hint_ignores_source_text_in_stdout() {
        let stdout = r#"
            const SANDBOX_FS_DENIAL_MARKERS: [&str; 3] = [
                "permission denied",
                "bwrap",
                "landlock",
            ];
        "#;
        let hint = sandbox_fs_denial_hint("tool exited with status 1", stdout, true, "01KTEST123");
        assert!(hint.is_none(), "got: {hint:?}");
    }
}
