use super::*;
use crate::review_cmd::tests::{
    ScopedEnvVarRestore, project_config_with_enabled_tools, setup_git_repo,
};
use crate::test_env_lock::ScopedTestEnvVar;
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_config::{GlobalConfig, ProjectProfile, TierStrategy, config::TierConfig};
use csa_core::types::{ReviewDecision, ToolName};
use csa_session::{ReviewVerdictArtifact, state::ReviewSessionMeta};

fn config_with_review_tier(enabled_tools: &[&str], models: &[&str]) -> csa_config::ProjectConfig {
    let mut config = project_config_with_enabled_tools(enabled_tools);
    if enabled_tools.contains(&"codex") {
        config.tools.get_mut("codex").unwrap().transport = Some(csa_config::TransportKind::Cli);
    }
    config.tiers.insert(
        "quality".to_string(),
        TierConfig {
            description: "quality".to_string(),
            models: models.iter().map(|model| (*model).to_string()).collect(),
            strategy: TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );
    config
}

#[test]
fn build_failover_chain_records_build_time_exclusions_without_runtime_failures() {
    let _available_guard =
        ScopedTestEnvVar::set(crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1");
    let config = config_with_review_tier(
        &["claude-code"],
        &[
            "codex/openai/gpt-5.4/high",
            "claude-code/anthropic/claude-sonnet/high",
        ],
    );

    // Empty runtime failures represents the later claude-code reviewer
    // succeeding; the skipped codex build-time exclusion (BEFORE the winner)
    // still needs a trace.
    let chain = crate::tier_model_fallback::build_fallback_chain_for_result(
        Some(&config),
        Some("quality"),
        &[],
        Some("claude-code/anthropic/claude-sonnet/high"),
        &[],
    );

    assert_eq!(chain.len(), 1);
    assert_eq!(chain[0].tool, "codex");
    assert_eq!(
        chain[0].model_spec.as_deref(),
        Some("codex/openai/gpt-5.4/high")
    );
    assert_eq!(chain[0].skip_reason, "disabled");
    assert!(!chain[0].quota_exhausted);
    assert!(
        chain.iter().all(|attempt| attempt.tool != "claude-code"),
        "the successful reviewer is not a fallback-chain skip"
    );
}

#[test]
fn build_failover_chain_uses_preference_order_before_winner() {
    let _available_guard =
        ScopedTestEnvVar::set(crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1");
    let config = config_with_review_tier(
        &["claude-code"],
        &[
            "codex/openai/gpt-5.4/high",
            "claude-code/anthropic/claude-sonnet/high",
        ],
    );
    let preference_order = vec!["claude-code".to_string()];

    let chain = crate::tier_model_fallback::build_fallback_chain_for_result(
        Some(&config),
        Some("quality"),
        &[],
        Some("claude-code/anthropic/claude-sonnet/high"),
        &preference_order,
    );

    assert!(
        chain.is_empty(),
        "raw-tier predecessors must not be recorded before a preferred winner"
    );
}

#[test]
fn issue_1718_no_failover_limits_review_candidates_to_primary() {
    let _available_guard =
        ScopedTestEnvVar::set(crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1");
    let config = config_with_review_tier(
        &["codex", "gemini-cli"],
        &[
            "codex/openai/gpt-5.4/medium",
            "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
        ],
    );
    let global = GlobalConfig::default();
    let primary_spec = "codex/openai/gpt-5.4/medium";

    let candidates = review_ordered_tier_candidates(ReviewTierCandidateRequest {
        initial_tool: ToolName::Codex,
        initial_model_spec: Some(primary_spec),
        tier_name: Some("quality"),
        project_config: Some(&config),
        global_config: Some(&global),
        tier_fallback_enabled: true,
        no_failover: true,
        tier_preference_order: &[],
    });

    assert_eq!(
        candidates,
        vec![(ToolName::Codex, Some(primary_spec.to_string()))]
    );
}

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
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf 'gemini-cli 1.0.0\\n'\n  exit 0\nfi\nprintf \"reason: 'QUOTA_EXHAUSTED'; monthly spending cap reached\\n\" >&2\nexit 1\n",
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

    let result = execute_review(
        ToolName::GeminiCli,
        "scope=uncommitted mode=review-only security=auto".to_string(),
        None,
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

#[cfg(unix)]
#[tokio::test]
async fn execute_review_advances_tier_fallback_when_explicit_tool_and_tier() {
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

    let codex_invocation_log = project_dir.path().join("codex-invocations.log");
    let gemini_invocation_log = project_dir.path().join("gemini-invocations.log");
    let codex_invocation_log_str = codex_invocation_log.display().to_string();
    let gemini_invocation_log_str = gemini_invocation_log.display().to_string();

    std::fs::write(
        bin_dir.join("gemini"),
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf 'gemini-cli 1.0.0\\n'\n  exit 0\nfi\nprintf 'gemini should not be invoked\\n' >> \"{gemini_invocation_log_str}\"\nprintf \"reason: 'QUOTA_EXHAUSTED'; monthly spending cap reached\\n\" >&2\nexit 1\n"
        ),
    )
    .unwrap();
    let codex_stub = format!(
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf 'codex-cli 1.0.0\\n'\n  exit 0\nfi\ncount=0\nif [ -f \"{codex_invocation_log_str}\" ]; then\n  count=$(wc -l < \"{codex_invocation_log_str}\")\nfi\nnext=$((count + 1))\nprintf 'attempt-%s\\n' \"$next\" >> \"{codex_invocation_log_str}\"\nif [ \"$next\" -eq 1 ]; then\n  printf 'codex_429_retry_exhausted: temporary codex 429 rate limit persisted after 3 retries\\n' >&2\n  exit 1\nfi\nprintf '%s\\n' '<!-- CSA:SECTION:summary -->' 'PASS via fallback codex variant' '<!-- CSA:SECTION:summary:END -->' '<!-- CSA:SECTION:details -->' 'No blocking issues found after falling back to the second codex tier candidate.' '<!-- CSA:SECTION:details:END -->'\n"
    );
    for binary in ["codex", "codex-acp"] {
        std::fs::write(bin_dir.join(binary), &codex_stub).unwrap();
    }
    for binary in ["gemini", "codex", "codex-acp"] {
        let path = bin_dir.join(binary);
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let patched_path = format!("{}:{inherited_path}", bin_dir.display());
    let _path_guard = ScopedEnvVarRestore::set("PATH", &patched_path);

    let config = config_with_review_tier(
        &["codex", "gemini-cli"],
        &[
            "codex/openai/gpt-5.4/medium",
            "codex/openai/gpt-5/high",
            "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
        ],
    );
    let global = GlobalConfig::default();

    let result = execute_review(
        ToolName::Codex,
        "scope=uncommitted mode=review-only security=auto".to_string(),
        None,
        None,
        Some("codex/openai/gpt-5.4/medium".to_string()),
        Some("quality".to_string()),
        true,
        None,
        "review: explicit-tool-tier-fallback".to_string(),
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
    .expect("explicit codex tier fallback should succeed");

    assert_ne!(result.forced_decision, Some(ReviewDecision::Unavailable));
    assert_eq!(result.executed_tool, ToolName::Codex);
    assert_eq!(result.routed_to.as_deref(), Some("codex/openai/gpt-5/high"));
    // #1852: the second codex variant SUCCEEDED, so the exhausted first variant
    // is failover provenance (kept in result.toml below), not a terminal
    // `primary_failure`.
    assert!(
        result.primary_failure.is_none(),
        "successful fallback must not record the failed-over-from error as primary_failure"
    );

    let codex_invocations = std::fs::read_to_string(&codex_invocation_log).unwrap();
    assert_eq!(codex_invocations.lines().count(), 2);
    assert!(
        !gemini_invocation_log.exists(),
        "gemini should not be invoked because the preferred codex fallback succeeds first"
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
    assert_eq!(
        artifact.routed_to.as_deref(),
        Some("codex/openai/gpt-5/high")
    );
    // #1852: persisted verdict mirrors the success-via-fallback provenance —
    // routing metadata is kept, primary_failure is cleared.
    assert!(
        artifact.primary_failure.is_none(),
        "persisted verdict of a successful fallback must not carry primary_failure"
    );

    let result_toml = std::fs::read_to_string(session_dir.join("result.toml")).unwrap();
    assert!(result_toml.contains("tool = \"codex\""));
    assert!(result_toml.contains("original_tool = \"codex\""));
    assert!(result_toml.contains("fallback_tool = \"codex\""));
    assert!(result_toml.contains("fallback_reason = \"429_quota_exhausted\""));
    assert!(result_toml.contains("PASS via fallback codex variant"));
}

#[cfg(unix)]
#[tokio::test]
async fn execute_review_marks_unavailable_when_all_tier_models_fail() {
    use std::os::unix::fs::PermissionsExt;

    if which::which("bwrap").is_err() {
        eprintln!("skipping: bwrap not installed (CI gap, see #987)");
        return;
    }

    let project_dir = setup_git_repo();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    std::fs::write(project_dir.path().join(".claude.json"), "{}\n").unwrap();
    let bin_dir = project_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    std::fs::write(
        bin_dir.join("gemini"),
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf 'gemini-cli 1.0.0\\n'\n  exit 0\nfi\nprintf \"reason: 'QUOTA_EXHAUSTED'; monthly spending cap reached\\n\" >&2\nexit 1\n",
    )
    .unwrap();
    for binary in ["codex", "codex-acp"] {
        std::fs::write(
            bin_dir.join(binary),
            "#!/bin/sh\nprintf 'HTTP 401 Invalid API key\\n' >&2\nexit 1\n",
        )
        .unwrap();
    }
    // claude-code now defaults to CLI transport (#1115/#1117 workaround);
    // stub `claude` (not `claude-code-acp`) to simulate the claude-code failure.
    std::fs::write(
        bin_dir.join("claude"),
        "#!/bin/sh\nprintf 'HTTP 403 Forbidden\\n' >&2\nexit 1\n",
    )
    .unwrap();
    for binary in ["gemini", "codex", "codex-acp", "claude"] {
        let path = bin_dir.join(binary);
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let patched_path = format!("{}:{inherited_path}", bin_dir.display());
    let _path_guard = ScopedEnvVarRestore::set("PATH", &patched_path);

    let config = config_with_review_tier(
        &["gemini-cli", "codex", "claude-code"],
        &[
            "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
            "codex/openai/gpt-5.4/high",
            "claude-code/anthropic/claude-sonnet/high",
        ],
    );
    let global = GlobalConfig::default();

    let result = execute_review(
        ToolName::GeminiCli,
        "scope=uncommitted mode=review-only security=auto".to_string(),
        None,
        None,
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh".to_string()),
        Some("quality".to_string()),
        true,
        None,
        "review: tier-all-failed".to_string(),
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
    .expect("all-failed fallback should still return an outcome");

    assert_eq!(result.forced_decision, Some(ReviewDecision::Unavailable));
    let failure_reason = result.failure_reason.expect("failure_reason");
    assert!(
        failure_reason.contains("gemini-cli/google/gemini-3.1-pro-preview/xhigh=QUOTA_EXHAUSTED")
    );
    assert!(failure_reason.contains("codex/openai/gpt-5.4/high=HTTP 401"));
    assert!(failure_reason.contains("claude-code/anthropic/claude-sonnet/high=HTTP 403"));
}

#[cfg(unix)]
#[tokio::test]
async fn execute_review_primary_success_keeps_routing_metadata_empty() {
    use std::os::unix::fs::PermissionsExt;

    let project_dir = setup_git_repo();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    let bin_dir = project_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    std::fs::write(
        bin_dir.join("gemini"),
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf 'gemini-cli 1.0.0\\n'\n  exit 0\nfi\nprintf '%s\\n' '<!-- CSA:SECTION:summary -->' 'PASS' '<!-- CSA:SECTION:summary:END -->'\n",
    )
    .unwrap();
    let mut perms = std::fs::metadata(bin_dir.join("gemini"))
        .unwrap()
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(bin_dir.join("gemini"), perms).unwrap();

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let patched_path = format!("{}:{inherited_path}", bin_dir.display());
    let _path_guard = ScopedEnvVarRestore::set("PATH", &patched_path);

    let config = config_with_review_tier(
        &["gemini-cli"],
        &["gemini-cli/google/gemini-3.1-pro-preview/xhigh"],
    );
    let global = GlobalConfig::default();

    let result = execute_review(
        ToolName::GeminiCli,
        "scope=uncommitted mode=review-only security=auto".to_string(),
        None,
        None,
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh".to_string()),
        Some("quality".to_string()),
        true,
        None,
        "review: primary-success".to_string(),
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
    .expect("primary model should succeed");

    assert_eq!(result.executed_tool, ToolName::GeminiCli);
    assert!(result.routed_to.is_none());
    assert!(result.primary_failure.is_none());
    assert!(result.failure_reason.is_none());
}
