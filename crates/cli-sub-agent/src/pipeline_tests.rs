use super::*;
use crate::session_guard::{SessionCleanupGuard, write_pre_exec_error_result};
use chrono::Utc;
use csa_config::config::CURRENT_SCHEMA_VERSION;
use csa_config::{ProjectMeta, ResourcesConfig};
use std::collections::HashMap;

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
        },
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        preferences: None,
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
        },
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        preferences: None,
    };

    assert_eq!(resolve_idle_timeout_seconds(Some(&cfg), None), 222);
    assert_eq!(
        resolve_idle_timeout_seconds(None, None),
        DEFAULT_IDLE_TIMEOUT_SECONDS
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
