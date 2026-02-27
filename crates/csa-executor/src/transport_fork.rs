use std::path::PathBuf;
use std::time::Duration;

use super::TransportFactory;

/// Which fork strategy was used for a session fork.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForkMethod {
    /// Claude Code CLI `--fork-session` (provider-level fork).
    Native,
    /// Context summary injection for tools without native fork support.
    Soft,
}

/// Result of a transport-level session fork attempt.
#[derive(Debug, Clone)]
pub struct ForkInfo {
    /// Whether the fork succeeded.
    pub success: bool,
    /// Which fork method was used.
    pub method: ForkMethod,
    /// New provider-level session ID (Native forks only).
    pub new_session_id: Option<String>,
    /// Human-readable notes (e.g. error details or context summary path).
    pub notes: Option<String>,
}

/// Options for a transport-level fork request.
#[derive(Debug, Clone)]
pub struct ForkRequest {
    /// The tool name.
    pub tool_name: String,
    /// Pre-computed fork method. When set, `fork_session` uses this instead of
    /// re-deriving from `tool_name`. This is essential for cross-tool forks where
    /// `resolve_fork` determines `ForkMethod::Soft` but the target tool would
    /// otherwise select `Native`.
    pub fork_method: Option<ForkMethod>,
    /// Provider session ID of the parent (required for Native fork).
    pub provider_session_id: Option<String>,
    /// Whether Codex PTY native fork should auto-accept trust prompts.
    pub codex_auto_trust: bool,
    /// CSA session ID of the parent (used for Soft fork context loading).
    pub parent_csa_session_id: String,
    /// Directory of the parent session (used for Soft fork to read result/output).
    pub parent_session_dir: PathBuf,
    /// Working directory for CLI fork commands.
    pub working_dir: PathBuf,
    /// Timeout for the CLI fork subprocess.
    pub timeout: Duration,
}

impl TransportFactory {
    /// Determine which fork method applies for a given tool.
    pub fn fork_method_for_tool(tool_name: &str) -> ForkMethod {
        if tool_name == "claude-code" {
            ForkMethod::Native
        } else if tool_name == "codex" {
            #[cfg(feature = "codex-pty-fork")]
            {
                ForkMethod::Native
            }
            #[cfg(not(feature = "codex-pty-fork"))]
            {
                ForkMethod::Soft
            }
        } else {
            ForkMethod::Soft
        }
    }

    /// Fork a session via the appropriate transport-level mechanism.
    ///
    /// - `claude-code`: Native fork via `claude --fork-session` CLI.
    /// - All others: Soft fork via context summary injection from parent session.
    pub async fn fork_session(request: &ForkRequest) -> ForkInfo {
        let method = request
            .fork_method
            .unwrap_or_else(|| Self::fork_method_for_tool(&request.tool_name));
        match method {
            ForkMethod::Native => Self::fork_native(request).await,
            ForkMethod::Soft => Self::fork_soft(request),
        }
    }

    async fn fork_native(request: &ForkRequest) -> ForkInfo {
        if request.tool_name == "codex" {
            return Self::fork_codex_via_pty(request).await;
        }

        let Some(provider_session_id) = &request.provider_session_id else {
            return ForkInfo {
                success: false,
                method: ForkMethod::Native,
                new_session_id: None,
                notes: Some(
                    "Native fork requires provider_session_id, but none was provided".to_string(),
                ),
            };
        };

        match csa_acp::fork_session_via_cli(
            provider_session_id,
            &request.working_dir,
            request.timeout,
        )
        .await
        {
            Ok(result) => ForkInfo {
                success: true,
                method: ForkMethod::Native,
                new_session_id: Some(result.session_id),
                notes: None,
            },
            Err(e) => ForkInfo {
                success: false,
                method: ForkMethod::Native,
                new_session_id: None,
                notes: Some(format!("Native fork failed: {e}")),
            },
        }
    }

    #[cfg(feature = "codex-pty-fork")]
    async fn fork_codex_via_pty(request: &ForkRequest) -> ForkInfo {
        use csa_process::pty_fork::{PtyForkConfig, PtyForkResult, fork_codex_session};

        let Some(parent_provider_session_id) = request.provider_session_id.as_deref() else {
            let reason = "codex native fork requires provider_session_id";
            tracing::warn!(tool = %request.tool_name, reason, "native fork degraded to soft fork");
            return Self::fork_soft_with_reason(request, reason);
        };

        let config = PtyForkConfig {
            codex_auto_trust: request.codex_auto_trust,
            ..PtyForkConfig::default()
        };
        match fork_codex_session(parent_provider_session_id, Path::new("codex"), &config).await {
            Ok(PtyForkResult::Success { child_session_id }) => ForkInfo {
                success: true,
                method: ForkMethod::Native,
                new_session_id: Some(child_session_id),
                notes: None,
            },
            Ok(PtyForkResult::Degraded { reason }) => {
                tracing::warn!(
                    tool = %request.tool_name,
                    reason = %reason,
                    "native fork degraded to soft fork"
                );
                Self::fork_soft_with_reason(request, &reason)
            }
            Ok(PtyForkResult::Failed { error }) => {
                tracing::warn!(
                    tool = %request.tool_name,
                    error = %error,
                    "native fork failed; falling back to soft fork"
                );
                Self::fork_soft_with_reason(request, &format!("Native codex fork failed: {error}"))
            }
            Err(e) => {
                tracing::warn!(
                    tool = %request.tool_name,
                    error = %e,
                    "native fork errored; falling back to soft fork"
                );
                Self::fork_soft_with_reason(
                    request,
                    &format!("Native codex fork errored unexpectedly: {e}"),
                )
            }
        }
    }

    #[cfg(not(feature = "codex-pty-fork"))]
    async fn fork_codex_via_pty(request: &ForkRequest) -> ForkInfo {
        Self::fork_soft_with_reason(
            request,
            "Native codex fork unavailable: feature `codex-pty-fork` is disabled",
        )
    }

    fn fork_soft_with_reason(request: &ForkRequest, native_reason: &str) -> ForkInfo {
        let mut soft = Self::fork_soft(request);
        let soft_note = soft.notes.take().unwrap_or_default();
        soft.notes = Some(if soft_note.is_empty() {
            format!("Native codex fork degraded: {native_reason}")
        } else {
            format!("Native codex fork degraded: {native_reason}; {soft_note}")
        });
        soft
    }

    fn fork_soft(request: &ForkRequest) -> ForkInfo {
        match csa_session::soft_fork_session(
            &request.parent_session_dir,
            &request.parent_csa_session_id,
        ) {
            Ok(ctx) => ForkInfo {
                success: true,
                method: ForkMethod::Soft,
                new_session_id: None,
                notes: Some(format!(
                    "Soft fork context ({} chars) ready for injection",
                    ctx.context_summary.len()
                )),
            },
            Err(e) => ForkInfo {
                success: false,
                method: ForkMethod::Soft,
                new_session_id: None,
                notes: Some(format!("Soft fork failed: {e}")),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fork_method_for_tool_claude_code_is_native() {
        assert_eq!(
            TransportFactory::fork_method_for_tool("claude-code"),
            ForkMethod::Native
        );
    }

    #[test]
    #[cfg(feature = "codex-pty-fork")]
    fn test_fork_method_for_tool_codex_is_native_when_feature_enabled() {
        assert_eq!(
            TransportFactory::fork_method_for_tool("codex"),
            ForkMethod::Native
        );
    }

    #[test]
    #[cfg(not(feature = "codex-pty-fork"))]
    fn test_fork_method_for_tool_codex_is_soft_when_feature_disabled() {
        assert_eq!(
            TransportFactory::fork_method_for_tool("codex"),
            ForkMethod::Soft
        );
    }

    #[test]
    fn test_fork_method_for_tool_gemini_cli_is_soft() {
        assert_eq!(
            TransportFactory::fork_method_for_tool("gemini-cli"),
            ForkMethod::Soft
        );
    }

    #[test]
    fn test_fork_method_for_tool_opencode_is_soft() {
        assert_eq!(
            TransportFactory::fork_method_for_tool("opencode"),
            ForkMethod::Soft
        );
    }

    #[test]
    fn test_fork_info_construction_success() {
        let info = ForkInfo {
            success: true,
            method: ForkMethod::Native,
            new_session_id: Some("new-sess-123".to_string()),
            notes: None,
        };
        assert!(info.success);
        assert_eq!(info.method, ForkMethod::Native);
        assert_eq!(info.new_session_id.as_deref(), Some("new-sess-123"));
        assert!(info.notes.is_none());
    }

    #[test]
    fn test_fork_info_construction_failure() {
        let info = ForkInfo {
            success: false,
            method: ForkMethod::Soft,
            new_session_id: None,
            notes: Some("something went wrong".to_string()),
        };
        assert!(!info.success);
        assert_eq!(info.method, ForkMethod::Soft);
        assert!(info.new_session_id.is_none());
        assert!(info.notes.as_deref().unwrap().contains("wrong"));
    }

    #[tokio::test]
    async fn test_fork_session_soft_with_empty_parent_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let request = ForkRequest {
            tool_name: "codex".to_string(),
            fork_method: None,
            provider_session_id: None,
            codex_auto_trust: false,
            parent_csa_session_id: "01TEST_PARENT".to_string(),
            parent_session_dir: tmp.path().to_path_buf(),
            working_dir: tmp.path().to_path_buf(),
            timeout: Duration::from_secs(10),
        };

        let info = TransportFactory::fork_session(&request).await;

        assert!(
            info.success,
            "Soft fork should succeed even with empty parent: {:?}",
            info.notes
        );
        assert_eq!(info.method, ForkMethod::Soft);
        assert!(
            info.new_session_id.is_none(),
            "Soft fork does not create provider session"
        );
        assert!(
            info.notes
                .as_deref()
                .unwrap()
                .contains("ready for injection")
        );
    }

    #[tokio::test]
    async fn test_fork_session_codex_native_falls_back_to_soft_when_provider_id_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let request = ForkRequest {
            tool_name: "codex".to_string(),
            fork_method: Some(ForkMethod::Native),
            provider_session_id: None,
            codex_auto_trust: false,
            parent_csa_session_id: "01TEST_PARENT".to_string(),
            parent_session_dir: tmp.path().to_path_buf(),
            working_dir: tmp.path().to_path_buf(),
            timeout: Duration::from_secs(10),
        };

        let info = TransportFactory::fork_session(&request).await;

        assert!(info.success);
        assert_eq!(info.method, ForkMethod::Soft);
        assert!(info.new_session_id.is_none());
        assert!(
            info.notes
                .as_deref()
                .unwrap_or_default()
                .contains("Native codex fork degraded")
        );
    }

    #[tokio::test]
    async fn test_fork_session_native_without_provider_id_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let request = ForkRequest {
            tool_name: "claude-code".to_string(),
            fork_method: None,
            provider_session_id: None,
            codex_auto_trust: false,
            parent_csa_session_id: "01TEST_PARENT".to_string(),
            parent_session_dir: tmp.path().to_path_buf(),
            working_dir: tmp.path().to_path_buf(),
            timeout: Duration::from_secs(10),
        };

        let info = TransportFactory::fork_session(&request).await;

        assert!(!info.success);
        assert_eq!(info.method, ForkMethod::Native);
        assert!(
            info.notes
                .as_deref()
                .unwrap()
                .contains("provider_session_id"),
            "Should explain missing provider ID: {:?}",
            info.notes
        );
    }

    #[tokio::test]
    async fn test_fork_session_soft_with_result_toml() {
        use chrono::Utc;
        use csa_session::result::{RESULT_FILE_NAME, SessionResult};

        let tmp = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let result = SessionResult {
            status: "success".to_string(),
            exit_code: 0,
            summary: "Tests passed".to_string(),
            tool: "codex".to_string(),
            started_at: now,
            completed_at: now,
            events_count: 0,
            artifacts: vec![],
        };
        std::fs::write(
            tmp.path().join(RESULT_FILE_NAME),
            toml::to_string_pretty(&result).unwrap(),
        )
        .unwrap();

        let request = ForkRequest {
            tool_name: "gemini-cli".to_string(),
            fork_method: None,
            provider_session_id: None,
            codex_auto_trust: false,
            parent_csa_session_id: "01RICH_PARENT".to_string(),
            parent_session_dir: tmp.path().to_path_buf(),
            working_dir: tmp.path().to_path_buf(),
            timeout: Duration::from_secs(10),
        };

        let info = TransportFactory::fork_session(&request).await;

        assert!(info.success);
        assert_eq!(info.method, ForkMethod::Soft);
        assert!(
            info.notes
                .as_deref()
                .unwrap()
                .contains("ready for injection")
        );
    }
}
