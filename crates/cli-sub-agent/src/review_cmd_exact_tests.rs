use std::path::Path;
use std::process::Command;

use csa_config::{GlobalConfig, ProjectProfile, TierStrategy, config::TierConfig};
use csa_core::types::{ReviewDecision, ToolName};
use csa_session::state::ReviewSessionMeta;
use tempfile::TempDir;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

fn exact_test_run_git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git command should execute");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn exact_test_setup_git_repo() -> TempDir {
    let temp = TempDir::new().expect("create tempdir");
    exact_test_run_git(temp.path(), &["init"]);
    exact_test_run_git(temp.path(), &["config", "user.email", "test@example.com"]);
    exact_test_run_git(temp.path(), &["config", "user.name", "Test User"]);

    std::fs::write(temp.path().join("tracked.txt"), "baseline\n").expect("write tracked file");
    exact_test_run_git(temp.path(), &["add", "tracked.txt"]);
    exact_test_run_git(temp.path(), &["commit", "-m", "initial"]);

    temp
}

fn exact_test_project_config_with_enabled_tools(tools: &[&str]) -> csa_config::ProjectConfig {
    let mut tool_map = std::collections::HashMap::new();
    for tool in csa_config::global::all_known_tools() {
        tool_map.insert(
            tool.as_str().to_string(),
            csa_config::ToolConfig {
                enabled: false,
                restrictions: None,
                suppress_notify: true,
                ..Default::default()
            },
        );
    }
    for tool in tools {
        tool_map.insert(
            (*tool).to_string(),
            csa_config::ToolConfig {
                enabled: true,
                restrictions: None,
                suppress_notify: true,
                ..Default::default()
            },
        );
    }

    csa_config::ProjectConfig {
        schema_version: 1,
        project: csa_config::ProjectMeta::default(),
        resources: csa_config::ResourcesConfig::default(),
        acp: Default::default(),
        tools: tool_map,
        review: None,
        debate: None,
        tiers: std::collections::HashMap::new(),
        tier_mapping: std::collections::HashMap::new(),
        aliases: std::collections::HashMap::new(),
        tool_aliases: std::collections::HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        execution: Default::default(),
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    }
}

fn exact_test_config_with_review_tier(
    enabled_tools: &[&str],
    models: &[&str],
) -> csa_config::ProjectConfig {
    let mut config = exact_test_project_config_with_enabled_tools(enabled_tools);
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

fn exact_test_write_executable(bin_dir: &Path, name: &str, body: &str) {
    let path = bin_dir.join(name);
    std::fs::write(&path, body).expect("write fake binary");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn execute_review_marks_unavailable_when_all_tier_models_fail() {
    let project_dir = exact_test_setup_git_repo();
    let _sandbox = test_session_sandbox::ScopedSessionSandbox::new(&project_dir).await;
    let bin_dir = project_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    exact_test_write_executable(
        &bin_dir,
        "gemini",
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf 'gemini-cli 1.0.0\\n'\n  exit 0\nfi\nprintf \"reason: 'QUOTA_EXHAUSTED'\\n\" >&2\nexit 1\n",
    );
    exact_test_write_executable(
        &bin_dir,
        "codex",
        "#!/bin/sh\nprintf 'HTTP 401 Invalid API key\\n' >&2\nexit 1\n",
    );
    exact_test_write_executable(
        &bin_dir,
        "claude-code-acp",
        "#!/bin/sh\nprintf 'HTTP 403 Forbidden\\n' >&2\nexit 1\n",
    );

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let patched_path = format!("{}:{inherited_path}", bin_dir.display());
    let _path_guard = test_env_lock::ScopedEnvVarRestore::set("PATH", &patched_path);

    let config = exact_test_config_with_review_tier(
        &["gemini-cli", "codex", "claude-code"],
        &[
            "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
            "codex/openai/gpt-5.4/high",
            "claude-code/anthropic/claude-sonnet/high",
        ],
    );
    let global = GlobalConfig::default();

    let result = review_cmd::execute_review_for_tests(
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
        review_routing::ReviewRoutingMetadata {
            project_profile: ProjectProfile::Unknown,
            detection_method: "auto",
        },
        csa_process::StreamMode::BufferOnly,
        pipeline::DEFAULT_IDLE_TIMEOUT_SECONDS,
        None,
        false,
        false,
        false,
        false,
        false,
        &[],
        &[],
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
async fn execute_review_falls_back_to_next_tier_model_and_persists_routing_metadata() {
    let project_dir = exact_test_setup_git_repo();
    let _sandbox = test_session_sandbox::ScopedSessionSandbox::new(&project_dir).await;
    let bin_dir = project_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    exact_test_write_executable(
        &bin_dir,
        "gemini",
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf 'gemini-cli 1.0.0\\n'\n  exit 0\nfi\nprintf \"reason: 'QUOTA_EXHAUSTED'\\n\" >&2\nexit 1\n",
    );
    exact_test_write_executable(
        &bin_dir,
        "codex",
        "#!/bin/sh\nprintf '%s\\n' '<!-- CSA:SECTION:summary -->' 'PASS' '<!-- CSA:SECTION:summary:END -->' '<!-- CSA:SECTION:details -->' 'No blocking issues found.' '<!-- CSA:SECTION:details:END -->'\n",
    );

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let patched_path = format!("{}:{inherited_path}", bin_dir.display());
    let _path_guard = test_env_lock::ScopedEnvVarRestore::set("PATH", &patched_path);

    let config = exact_test_config_with_review_tier(
        &["gemini-cli", "codex"],
        &[
            "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
            "codex/openai/gpt-5.4/high",
        ],
    );
    let global = GlobalConfig::default();

    let result = review_cmd::execute_review_for_tests(
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
        review_routing::ReviewRoutingMetadata {
            project_profile: ProjectProfile::Unknown,
            detection_method: "auto",
        },
        csa_process::StreamMode::BufferOnly,
        pipeline::DEFAULT_IDLE_TIMEOUT_SECONDS,
        None,
        false,
        false,
        false,
        false,
        false,
        &[],
        &[],
    )
    .await
    .expect("tier fallback should succeed");

    assert_eq!(result.executed_tool, ToolName::Codex);
    assert_eq!(
        result.routed_to.as_deref(),
        Some("codex/openai/gpt-5.4/high")
    );
    assert_eq!(result.primary_failure.as_deref(), Some("QUOTA_EXHAUSTED"));

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
    };
    let session_dir =
        csa_session::get_session_dir(project_dir.path(), &result.execution.meta_session_id)
            .unwrap();
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    if verdict_path.exists() {
        std::fs::remove_file(&verdict_path).unwrap();
    }
    review_cmd::persist_review_sidecars_if_session_exists(
        project_dir.path(),
        &meta,
        result.persistable_session_id.as_deref(),
    );
    let artifact: csa_session::ReviewVerdictArtifact =
        serde_json::from_str(&std::fs::read_to_string(&verdict_path).unwrap()).unwrap();
    assert_eq!(artifact.routed_to, result.routed_to);
    assert_eq!(artifact.primary_failure, result.primary_failure);
}

#[test]
fn execute_review_fix_loop_skipped_on_unavailable() {
    assert!(review_cmd::should_run_fix_loop(true, ReviewDecision::Fail));
    assert!(!review_cmd::should_run_fix_loop(
        true,
        ReviewDecision::Unavailable
    ));
    assert!(!review_cmd::should_run_fix_loop(true, ReviewDecision::Pass));
    assert!(!review_cmd::should_run_fix_loop(true, ReviewDecision::Skip));
    assert!(!review_cmd::should_run_fix_loop(
        true,
        ReviewDecision::Uncertain
    ));
    assert!(!review_cmd::should_run_fix_loop(
        false,
        ReviewDecision::Fail
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn execute_review_unavailable_does_not_persist_session_artifacts() {
    let project_dir = exact_test_setup_git_repo();
    let _sandbox = test_session_sandbox::ScopedSessionSandbox::new(&project_dir).await;
    let meta = ReviewSessionMeta {
        session_id: "unknown".to_string(),
        head_sha: String::new(),
        decision: ReviewDecision::Unavailable.as_str().to_string(),
        verdict: "UNAVAILABLE".to_string(),
        status_reason: Some("tier_models_unavailable".to_string()),
        routed_to: None,
        primary_failure: Some("QUOTA_EXHAUSTED".to_string()),
        failure_reason: Some("quality exhausted".to_string()),
        tool: ToolName::GeminiCli.as_str().to_string(),
        scope: "uncommitted".to_string(),
        exit_code: 1,
        fix_attempted: false,
        fix_rounds: 0,
        review_iterations: 0,
        timestamp: chrono::Utc::now(),
        diff_fingerprint: None,
    };
    review_cmd::persist_review_sidecars_if_session_exists(
        project_dir.path(),
        &meta,
        None,
    );

    let unknown_output = csa_session::get_session_root(project_dir.path())
        .unwrap()
        .join("sessions")
        .join("unknown")
        .join("output");
    assert!(
        !unknown_output.exists(),
        "unexpected session sidecars leaked into {}",
        unknown_output.display()
    );
}
