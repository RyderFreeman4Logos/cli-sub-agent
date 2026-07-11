use super::tier_tests::config_with_review_tier;
use super::*;
use crate::review_cmd::tests::{ScopedEnvVarRestore, setup_git_repo};
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_config::{GlobalConfig, ProjectProfile};
use csa_core::types::{ReviewDecision, ToolName};
use csa_session::{ReviewVerdictArtifact, state::ReviewSessionMeta};

#[cfg(unix)]
#[tokio::test]
async fn execute_review_falls_back_to_next_tier_model_and_persists_routing_metadata() {
    use crate::review_cmd::output::persist_review_verdict;
    use std::os::unix::fs::PermissionsExt;

    if which::which("bwrap").is_err() {
        eprintln!("skipping: bwrap not installed (CI gap, see #987)");
        return;
    }

    let project_dir = setup_git_repo();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    let bin_dir = project_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    std::fs::write(
        bin_dir.join("gemini"),
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf 'gemini-cli 1.0.0\\n'\n  exit 0\nfi\nprintf \"reason: 'QUOTA_EXHAUSTED'\\n\" >&2\nexit 1\n",
    )
    .unwrap();
    std::fs::write(
        bin_dir.join("codex"),
        "#!/bin/sh\nprintf '%s\\n' '<!-- CSA:SECTION:summary -->' 'PASS' '<!-- CSA:SECTION:summary:END -->' '<!-- CSA:SECTION:details -->' 'No blocking issues found.' '<!-- CSA:SECTION:details:END -->'\n",
    )
    .unwrap();
    for binary in ["gemini", "codex"] {
        let path = bin_dir.join(binary);
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let patched_path = format!("{}:{inherited_path}", bin_dir.display());
    let _path_guard = ScopedEnvVarRestore::set("PATH", &patched_path);

    let config = config_with_review_tier(
        &["gemini-cli", "codex"],
        &[
            "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
            "codex/openai/gpt-5.4/high",
        ],
    );
    let global = GlobalConfig::default();
    let requested_session = csa_session::create_session(
        project_dir.path(),
        Some("review explicit-session tier fallback"),
        None,
        Some("gemini-cli"),
    )
    .unwrap();

    let result = execute_review(
        ToolName::GeminiCli,
        "scope=uncommitted mode=review-only security=auto".to_string(),
        Some(requested_session.meta_session_id.clone()),
        None,
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh".to_string()),
        Some("quality".to_string()),
        true,
        None,
        "review: tier-fallback-success".to_string(),
        project_dir.path(),
        Some(&config),
        &global,
        None,
        ReviewRoutingMetadata {
            project_profile: ProjectProfile::Unknown,
            detection_method: "auto",
        },
        csa_process::StreamMode::BufferOnly,
        crate::pipeline::DEFAULT_IDLE_TIMEOUT_SECONDS,
        None,
        false,
        false,
        false,
        false,
        false,
        &[],
        &[],
        Some(false), // error_marker_scan_override: force scan OFF for marker-bearing fixtures (#1745)
    )
    .await
    .expect("tier fallback should succeed");

    assert_eq!(result.executed_tool, ToolName::Codex);
    assert_ne!(
        result.execution.meta_session_id, requested_session.meta_session_id,
        "cross-model fallback must not resume the requested session in place"
    );
    let child = csa_session::load_session(project_dir.path(), &result.execution.meta_session_id)
        .expect("load fallback child session");
    assert_eq!(
        child.genealogy.parent_session_id.as_deref(),
        Some(requested_session.meta_session_id.as_str())
    );
    assert_eq!(
        result.routed_to.as_deref(),
        Some("codex/openai/gpt-5.4/high")
    );
    // #1852: a SUCCESSFUL failover leaves `primary_failure` unset — the review
    // succeeded, so the failed-over-from quota error is not a terminal failure.
    // Its provenance is preserved in the persisted fallback chain below.
    assert!(
        result.primary_failure.is_none(),
        "successful fallback must not record the failed-over-from error as primary_failure"
    );

    let meta = ReviewSessionMeta {
        session_id: result.execution.meta_session_id.clone(),
        head_sha: String::new(),
        decision: ReviewDecision::Pass.as_str().to_string(),
        verdict: "CLEAN".to_string(),
        status_reason: None,
        routed_to: result.routed_to.clone(),
        primary_failure: result.primary_failure.clone(),
        failure_reason: result.failure_reason.clone(),
        tool: result.executed_tool.as_str().to_string(),
        scope: "uncommitted".to_string(),
        exit_code: 0,
        fix_attempted: false,
        fix_rounds: 0,
        review_iterations: 1,
        timestamp: chrono::Utc::now(),
        diff_fingerprint: None,
        review_mode: None,
        fix_convergence: None,
    };
    let session_dir =
        csa_session::get_session_dir(project_dir.path(), &result.execution.meta_session_id)
            .unwrap();
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    if verdict_path.exists() {
        std::fs::remove_file(&verdict_path).unwrap();
    }
    persist_review_verdict(project_dir.path(), &meta, &[], Vec::new());
    let artifact: ReviewVerdictArtifact =
        serde_json::from_str(&std::fs::read_to_string(&verdict_path).unwrap()).unwrap();
    assert_eq!(artifact.routed_to, result.routed_to);
    assert_eq!(artifact.primary_failure, result.primary_failure);

    let persisted = csa_session::load_result(project_dir.path(), &result.execution.meta_session_id)
        .unwrap()
        .expect("result.toml should exist");
    assert_eq!(persisted.original_tool.as_deref(), Some("gemini-cli"));
    assert_eq!(persisted.fallback_tool.as_deref(), Some("codex"));
    assert_eq!(
        persisted.fallback_reason.as_deref(),
        Some("429_quota_exhausted")
    );
}
