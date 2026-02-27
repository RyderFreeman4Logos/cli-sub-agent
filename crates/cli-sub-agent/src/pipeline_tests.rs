use super::*;
use crate::session_guard::{SessionCleanupGuard, write_pre_exec_error_result};
use chrono::Utc;
use csa_config::config::{CURRENT_SCHEMA_VERSION, TierConfig};
use csa_config::{ProjectMeta, ResourcesConfig};
use csa_hooks::{FailPolicy, HookConfig, HookEvent, HooksConfig, Waiver};
use std::collections::HashMap;
use std::fs;

#[test]
fn determine_project_root_none_returns_cwd() {
    let result = determine_project_root(None).unwrap();
    let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
    assert_eq!(result, cwd);
}

#[test]
fn determine_project_root_with_valid_path() {
    let tmp = tempfile::tempdir().unwrap();
    let result = determine_project_root(Some(tmp.path().to_str().unwrap())).unwrap();
    assert_eq!(result, tmp.path().canonicalize().unwrap());
}

#[test]
fn determine_project_root_nonexistent_path_errors() {
    let result = determine_project_root(Some("/nonexistent/path/12345"));
    assert!(result.is_err());
}

#[test]
fn load_and_validate_exceeds_depth_returns_none() {
    let tmp = tempfile::tempdir().unwrap();
    // With no config, max_depth defaults to 5
    let result = load_and_validate(tmp.path(), 100).unwrap();
    assert!(
        result.is_none(),
        "Should return None when depth exceeds max"
    );
}

#[test]
fn load_and_validate_within_depth_returns_some() {
    let tmp = tempfile::tempdir().unwrap();
    let result = load_and_validate(tmp.path(), 0).unwrap();
    assert!(
        result.is_some(),
        "Should return Some when depth is within bounds"
    );
}

#[test]
fn resolve_idle_timeout_prefers_cli_override() {
    let cfg = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig {
            min_free_memory_mb: 4096,
            idle_timeout_seconds: 111,
            initial_estimates: HashMap::new(),
            ..Default::default()
        },
        acp: Default::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
    };

    assert_eq!(resolve_idle_timeout_seconds(Some(&cfg), Some(42)), 42);
}

/// Verify that a new session without `--description` gets an auto-generated
/// description derived from `truncate_prompt(prompt, 80)`.
#[test]
fn auto_description_from_prompt_when_none_provided() {
    use crate::run_helpers::truncate_prompt;

    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path();

    let prompt = "Analyze the authentication module and fix the login bug";
    let description: Option<String> = None;

    // Replicate the pipeline logic: when description is None, derive from prompt
    let effective_description = description.or_else(|| Some(truncate_prompt(prompt, 80)));

    assert!(
        effective_description.is_some(),
        "auto-generated description must be Some"
    );
    assert_eq!(
        effective_description.as_deref().unwrap(),
        prompt,
        "short prompt should be used as-is (no truncation needed)"
    );

    // Verify the session is persisted with the auto-generated description
    let session = csa_session::create_session(
        project_root,
        effective_description.as_deref(),
        None,
        Some("codex"),
    )
    .unwrap();
    assert_eq!(
        session.description.as_deref(),
        Some(prompt),
        "session state must carry the auto-generated description"
    );

    // Reload from disk and confirm persistence
    let reloaded = csa_session::load_session(project_root, &session.meta_session_id).unwrap();
    assert_eq!(
        reloaded.description.as_deref(),
        Some(prompt),
        "description must survive save/load round-trip"
    );
}

/// Verify that a long prompt is truncated to 80 chars for auto-description.
#[test]
fn auto_description_truncates_long_prompt() {
    use crate::run_helpers::truncate_prompt;

    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path();

    let long_prompt = "Please analyze the entire authentication module including OAuth2 flows, JWT token validation, session management, RBAC permissions, and the password reset workflow to identify all security vulnerabilities";
    let description: Option<String> = None;

    let effective_description = description.or_else(|| Some(truncate_prompt(long_prompt, 80)));
    let desc = effective_description.as_deref().unwrap();

    assert!(
        desc.chars().count() <= 80,
        "auto-generated description must be at most 80 chars, got {}",
        desc.chars().count()
    );
    assert!(
        desc.ends_with("..."),
        "truncated description must end with '...'"
    );

    // Verify it persists correctly in the session
    let session =
        csa_session::create_session(project_root, Some(desc), None, Some("codex")).unwrap();
    assert_eq!(session.description.as_deref(), Some(desc));
}

/// Verify that resumed sessions preserve their existing description.
#[test]
fn resumed_session_keeps_existing_description() {
    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path();

    // Create a session with an explicit description
    let original_desc = "original task description";
    let session =
        csa_session::create_session(project_root, Some(original_desc), None, Some("codex"))
            .unwrap();

    // Simulate resuming: load the existing session (as the pipeline does for --session)
    let loaded = csa_session::load_session(project_root, &session.meta_session_id).unwrap();

    assert_eq!(
        loaded.description.as_deref(),
        Some(original_desc),
        "resumed session must keep its original description"
    );
}

#[test]
fn resolve_idle_timeout_uses_config_then_default() {
    let cfg = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig {
            min_free_memory_mb: 4096,
            idle_timeout_seconds: 222,
            initial_estimates: HashMap::new(),
            ..Default::default()
        },
        acp: Default::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
    };

    assert_eq!(resolve_idle_timeout_seconds(Some(&cfg), None), 222);
    assert_eq!(
        resolve_idle_timeout_seconds(None, None),
        DEFAULT_IDLE_TIMEOUT_SECONDS
    );
}

#[test]
fn resolve_liveness_dead_seconds_uses_config_then_default() {
    let cfg = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig {
            liveness_dead_seconds: Some(42),
            ..Default::default()
        },
        acp: Default::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
    };

    assert_eq!(resolve_liveness_dead_seconds(Some(&cfg)), 42);
    assert_eq!(
        resolve_liveness_dead_seconds(None),
        DEFAULT_LIVENESS_DEAD_SECONDS
    );
}

fn make_hooks_config(
    event: HookEvent,
    command: &str,
    fail_policy: FailPolicy,
    waivers: Vec<Waiver>,
) -> HooksConfig {
    let mut hooks = HashMap::new();
    hooks.insert(
        event.as_config_key().to_string(),
        HookConfig {
            enabled: true,
            command: Some(command.to_string()),
            timeout_secs: 2,
            fail_policy,
            waivers,
        },
    );
    HooksConfig {
        builtin_guards: None,
        prompt_guard: Vec::new(),
        hooks,
    }
}

#[test]
fn pipeline_hook_open_policy_failure_continues() {
    let config = make_hooks_config(HookEvent::PreRun, "exit 1", FailPolicy::Open, Vec::new());
    let vars = HashMap::new();
    let result = run_pipeline_hook(HookEvent::PreRun, &config, &vars);
    assert!(
        result.is_ok(),
        "open policy should continue even when hook command fails"
    );
}

#[test]
fn pipeline_hook_closed_policy_failure_without_waiver_returns_err() {
    let config = make_hooks_config(HookEvent::PreRun, "exit 1", FailPolicy::Closed, Vec::new());
    let vars = HashMap::new();
    let result = run_pipeline_hook(HookEvent::PreRun, &config, &vars);
    assert!(
        result.is_err(),
        "closed policy without waiver must return an error"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("PreRun"));
    assert!(err_msg.contains("fail_policy=closed"));
}

#[test]
fn pipeline_hook_closed_policy_failure_with_valid_waiver_continues() {
    let waiver = Waiver {
        scope: "pre_run".to_string(),
        justification: "temporary exception".to_string(),
        ticket: Some("CSA-123".to_string()),
        approver: Some("reviewer".to_string()),
        expires_at: None,
    };
    let config = make_hooks_config(
        HookEvent::PreRun,
        "exit 1",
        FailPolicy::Closed,
        vec![waiver],
    );
    let vars = HashMap::new();
    let result = run_pipeline_hook(HookEvent::PreRun, &config, &vars);
    assert!(
        result.is_ok(),
        "closed policy with valid waiver should continue"
    );
}

#[test]
fn pipeline_hook_closed_policy_success_continues() {
    let config = make_hooks_config(HookEvent::PreRun, "exit 0", FailPolicy::Closed, Vec::new());
    let vars = HashMap::new();
    let result = run_pipeline_hook(HookEvent::PreRun, &config, &vars);
    assert!(
        result.is_ok(),
        "closed policy with successful hook should pass"
    );
}

#[test]
fn pipeline_hook_open_policy_success_continues() {
    let config = make_hooks_config(HookEvent::PreRun, "exit 0", FailPolicy::Open, Vec::new());
    let vars = HashMap::new();
    let result = run_pipeline_hook(HookEvent::PreRun, &config, &vars);
    assert!(result.is_ok(), "open policy baseline success should pass");
}

#[test]
fn pipeline_post_run_closed_policy_failure_without_waiver_returns_err() {
    let config = make_hooks_config(HookEvent::PostRun, "exit 1", FailPolicy::Closed, Vec::new());
    let vars = HashMap::new();
    let result = run_pipeline_hook(HookEvent::PostRun, &config, &vars);
    assert!(
        result.is_err(),
        "post-run closed policy without waiver must return an error"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("PostRun"));
    assert!(err_msg.contains("fail_policy=closed"));
}

fn test_config_with_node_heap_limit(node_heap_limit_mb: Option<u64>) -> ProjectConfig {
    ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig {
            node_heap_limit_mb,
            ..Default::default()
        },
        acp: Default::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
    }
}

#[test]
fn build_merged_env_injects_node_options_when_heap_limit_configured() {
    let cfg = test_config_with_node_heap_limit(Some(2048));
    let merged = crate::pipeline_env::build_merged_env(None, Some(&cfg), "claude-code");

    assert_eq!(
        merged.get("NODE_OPTIONS"),
        Some(&"--max-old-space-size=2048".to_string())
    );
    assert_eq!(
        merged.get("CSA_SUPPRESS_NOTIFY"),
        Some(&"1".to_string()),
        "suppress notify should remain enabled by default"
    );
}

#[test]
fn build_merged_env_does_not_inject_node_options_without_heap_limit() {
    let cfg = test_config_with_node_heap_limit(None);
    // Use a lightweight tool (codex) whose profile does not default node_heap_limit_mb.
    // Heavyweight tools (claude-code) now default to Some(2048) even without explicit config.
    let merged = crate::pipeline_env::build_merged_env(None, Some(&cfg), "codex");

    assert!(
        !merged.contains_key("NODE_OPTIONS"),
        "NODE_OPTIONS should be absent when no node heap limit is configured"
    );
}

#[test]
fn build_merged_env_appends_node_options_when_existing_value_present() {
    let cfg = test_config_with_node_heap_limit(Some(2048));
    let mut extra_env = HashMap::new();
    extra_env.insert("NODE_OPTIONS".to_string(), "--trace-warnings".to_string());

    let merged = crate::pipeline_env::build_merged_env(Some(&extra_env), Some(&cfg), "claude-code");

    assert_eq!(
        merged.get("NODE_OPTIONS"),
        Some(&"--trace-warnings --max-old-space-size=2048".to_string())
    );
}

#[test]
fn context_load_options_with_skips_empty_returns_none() {
    let skip_files: Vec<String> = Vec::new();
    let options = context_load_options_with_skips(&skip_files);
    assert!(options.is_none());
}

#[test]
fn context_load_options_with_skips_propagates_files() {
    let skip_files = vec!["AGENTS.md".to_string(), "rules/private.md".to_string()];
    let options = context_load_options_with_skips(&skip_files).expect("must return options");
    assert_eq!(options.skip_files, skip_files);
    assert_eq!(options.max_bytes, None);
}

/// Verify that `SessionCleanupGuard` removes the directory on drop when not defused.
#[test]
fn cleanup_guard_removes_orphan_dir_on_drop() {
    let tmp = tempfile::tempdir().unwrap();
    let orphan_dir = tmp.path().join("orphan-session");
    fs::create_dir_all(&orphan_dir).unwrap();
    assert!(orphan_dir.exists());

    {
        let _guard = SessionCleanupGuard::new(orphan_dir.clone());
        // guard drops here without defuse
    }

    assert!(
        !orphan_dir.exists(),
        "cleanup guard must remove orphan session directory on drop"
    );
}

/// Verify that `SessionCleanupGuard` preserves the directory when defused.
#[test]
fn cleanup_guard_preserves_dir_when_defused() {
    let tmp = tempfile::tempdir().unwrap();
    let session_dir = tmp.path().join("good-session");
    fs::create_dir_all(&session_dir).unwrap();
    assert!(session_dir.exists());

    {
        let mut guard = SessionCleanupGuard::new(session_dir.clone());
        guard.defuse();
        // guard drops here after defuse
    }

    assert!(
        session_dir.exists(),
        "cleanup guard must preserve session directory when defused"
    );
}

/// Verify that pre-execution failures preserve the session directory (defuse + result.toml).
///
/// This tests the pattern used in `execute_with_session_and_meta`: when a
/// pre-execution step fails, we write an error `result.toml` and defuse the
/// guard so the session directory survives with a failure record instead of
/// being deleted as an orphan.
#[test]
fn pre_exec_failure_preserves_session_with_error_result() {
    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path();

    let session = csa_session::create_session(project_root, Some("test"), None, Some("codex"))
        .expect("session creation must succeed");

    let session_dir = csa_session::get_session_dir(project_root, &session.meta_session_id).unwrap();
    assert!(session_dir.exists());

    // Simulate the cleanup-guard + write_pre_exec_error_result pattern
    {
        let mut guard = SessionCleanupGuard::new(session_dir.clone());
        let error = anyhow::anyhow!("spawn failed: command not found");
        write_pre_exec_error_result(project_root, &session.meta_session_id, "codex", &error);
        guard.defuse();
    }

    // Session directory must survive
    assert!(
        session_dir.exists(),
        "session directory must be preserved after pre-exec failure"
    );

    // Error result.toml must exist and be loadable
    let loaded = csa_session::load_result(project_root, &session.meta_session_id)
        .expect("load_result must not error")
        .expect("result.toml must exist after pre-exec failure");

    assert_eq!(loaded.status, "failure");
    assert!(loaded.summary.starts_with("pre-exec:"));
    assert!(loaded.summary.contains("spawn failed"));
}

/// Verify that `write_pre_exec_error_result` produces a result.toml with
/// status = "failure" and a summary prefixed with "pre-exec:".
#[test]
fn pre_exec_error_writes_failure_result() {
    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path();

    // Create a real session so the directory structure exists
    let session = csa_session::create_session(project_root, Some("test"), None, Some("codex"))
        .expect("session creation must succeed");

    // Simulate a pre-execution failure (e.g., resource exhaustion)
    let error = anyhow::anyhow!("tool binary not found in PATH");
    write_pre_exec_error_result(project_root, &session.meta_session_id, "codex", &error);

    // Load and verify
    let loaded = csa_session::load_result(project_root, &session.meta_session_id)
        .expect("load_result must not error")
        .expect("result.toml must exist");

    assert_eq!(loaded.status, "failure", "status must be failure");
    assert_eq!(loaded.exit_code, 1, "exit_code must be 1");
    assert!(
        loaded.summary.starts_with("pre-exec:"),
        "summary must start with 'pre-exec:', got: {}",
        loaded.summary
    );
    assert!(
        loaded.summary.contains("tool binary not found"),
        "summary must contain the error message, got: {}",
        loaded.summary
    );
    assert_eq!(loaded.tool, "codex");
    assert!(loaded.artifacts.is_empty());
}

// --- enforce_tier regression tests ---

fn config_with_tier_for_tool(_tool_prefix: &str, model_spec: &str) -> ProjectConfig {
    let mut tiers = HashMap::new();
    tiers.insert(
        "tier-2-standard".to_string(),
        TierConfig {
            description: "test tier".to_string(),
            models: vec![model_spec.to_string()],
            token_budget: None,
            max_turns: None,
        },
    );
    ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers,
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
    }
}

/// When `enforce_tier=true`, passing a model spec not in the tier whitelist
/// must produce a tier-related error.
#[tokio::test]
async fn build_and_validate_executor_enforce_tier_true_rejects_non_whitelisted_spec() {
    let cfg = config_with_tier_for_tool("codex", "codex/openai/gpt-5.3-codex/high");

    let result = build_and_validate_executor(
        &ToolName::Codex,
        Some("codex/openai/gpt-4o/low"), // not whitelisted
        None,
        None,
        ConfigRefs {
            project: Some(&cfg),
            global: None,
        },
        true,
        false,
    )
    .await;

    assert!(
        result.is_err(),
        "enforce_tier=true must reject non-whitelisted spec"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("tier") || err_msg.contains("whitelist"),
        "error must mention tier/whitelist, got: {err_msg}"
    );
}

/// When `enforce_tier=false`, the same non-whitelisted model spec must NOT
/// produce a tier-related error. It may fail for other reasons (e.g., tool
/// not installed), but the tier check is skipped.
#[tokio::test]
async fn build_and_validate_executor_enforce_tier_false_skips_whitelist_check() {
    let cfg = config_with_tier_for_tool("codex", "codex/openai/gpt-5.3-codex/high");

    let result = build_and_validate_executor(
        &ToolName::Codex,
        Some("codex/openai/gpt-4o/low"), // not whitelisted, but won't be checked
        None,
        None,
        ConfigRefs {
            project: Some(&cfg),
            global: None,
        },
        false,
        false,
    )
    .await;

    // May succeed or fail (e.g., tool not installed), but must NOT fail
    // with a tier whitelist error.
    // Actual error fragments from enforce_tier_whitelist (config.rs):
    //   "not configured in any tier"
    //   "belongs to tool"
    if let Err(e) = &result {
        let msg = e.to_string();
        assert!(
            !msg.contains("not configured in any tier") && !msg.contains("belongs to tool"),
            "enforce_tier=false must skip tier whitelist check, but got: {msg}"
        );
    }
}

/// When `enforce_tier=true`, a model name not matching any tier entry for the
/// tool must be rejected.
#[tokio::test]
async fn build_and_validate_executor_enforce_tier_true_rejects_non_whitelisted_model_name() {
    let cfg = config_with_tier_for_tool("codex", "codex/openai/gpt-5.3-codex/high");

    let result = build_and_validate_executor(
        &ToolName::Codex,
        None,
        Some("unknown-model-xyz"), // model name not in tier
        None,
        ConfigRefs {
            project: Some(&cfg),
            global: None,
        },
        true,
        false,
    )
    .await;

    assert!(
        result.is_err(),
        "enforce_tier=true must reject non-whitelisted model name"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("tier") || err_msg.contains("model"),
        "error must mention tier/model, got: {err_msg}"
    );
}

/// When `enforce_tier=false`, a non-whitelisted model name must NOT produce
/// a tier-related error.
#[tokio::test]
async fn build_and_validate_executor_enforce_tier_false_skips_model_name_check() {
    let cfg = config_with_tier_for_tool("codex", "codex/openai/gpt-5.3-codex/high");

    let result = build_and_validate_executor(
        &ToolName::Codex,
        None,
        Some("unknown-model-xyz"), // not in tier, but won't be checked
        None,
        ConfigRefs {
            project: Some(&cfg),
            global: None,
        },
        false,
        false,
    )
    .await;

    // Actual error fragment from enforce_tier_model_name (config.rs):
    //   "is not configured in any tier"
    if let Err(e) = &result {
        let msg = e.to_string();
        assert!(
            !msg.contains("not configured in any tier"),
            "enforce_tier=false must skip model name check, but got: {msg}"
        );
    }
}

/// When config has no tiers, both enforce_tier values must behave identically:
/// no tier-related errors regardless of the flag.
#[tokio::test]
async fn build_and_validate_executor_no_tiers_both_flags_equivalent() {
    let cfg = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(), // no tiers
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
    };

    let result_true = build_and_validate_executor(
        &ToolName::Codex,
        Some("codex/openai/gpt-4o/low"),
        None,
        None,
        ConfigRefs {
            project: Some(&cfg),
            global: None,
        },
        true,
        false,
    )
    .await;

    let result_false = build_and_validate_executor(
        &ToolName::Codex,
        Some("codex/openai/gpt-4o/low"),
        None,
        None,
        ConfigRefs {
            project: Some(&cfg),
            global: None,
        },
        false,
        false,
    )
    .await;

    // Neither should fail with tier errors (tiers are empty).
    // Both should produce the same outcome (success or same non-tier error).
    for (label, result) in [("true", &result_true), ("false", &result_false)] {
        if let Err(e) = result {
            let msg = e.to_string();
            assert!(
                !msg.contains("not configured in any tier") && !msg.contains("belongs to tool"),
                "enforce_tier={label} with no tiers must not produce tier error, got: {msg}"
            );
        }
    }
    // Both must have the same Ok/Err status (empty tiers = no behavioral difference)
    assert_eq!(
        result_true.is_ok(),
        result_false.is_ok(),
        "enforce_tier=true and false must behave identically with empty tiers"
    );
}

