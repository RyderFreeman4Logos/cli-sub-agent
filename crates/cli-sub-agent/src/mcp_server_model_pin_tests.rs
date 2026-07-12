use super::*;
use std::collections::HashMap;
use std::path::Path;
use tempfile::tempdir;

fn trusted_startup_env_for_pinned_mcp_session(
    project_root: &Path,
    model_spec: &str,
    no_failover: bool,
) -> crate::startup_env::StartupSubtreeEnv {
    let session = create_session(
        project_root,
        Some("mcp pinned startup"),
        None,
        Some("codex"),
    )
    .expect("create mcp pinned session");
    let session_dir = get_session_dir(project_root, &session.meta_session_id).expect("session dir");
    let pin =
        crate::run_cmd_model_pin::resolve_subtree_model_pin(Some(model_spec), true, no_failover)
            .expect("trusted pin");
    crate::run_cmd_model_pin::sync_subtree_model_pin_sidecar(
        project_root,
        &session.meta_session_id,
        &session_dir,
        Some(&pin),
    )
    .expect("write trusted pin sidecar");

    crate::startup_env::StartupSubtreeEnv::from_values(HashMap::from([
        (
            csa_core::env::CSA_DEPTH_ENV_KEY,
            session.genealogy.depth.saturating_add(1).to_string(),
        ),
        (
            csa_core::env::CSA_INTERNAL_INVOCATION_ENV_KEY,
            "1".to_string(),
        ),
        (
            csa_core::env::CSA_SESSION_ID_ENV_KEY,
            session.meta_session_id,
        ),
        (
            csa_core::env::CSA_SESSION_DIR_ENV_KEY,
            session_dir.display().to_string(),
        ),
        (
            csa_core::env::CSA_PROJECT_ROOT_ENV_KEY,
            project_root.display().to_string(),
        ),
        (
            csa_core::env::CSA_MODEL_SPEC_ENV_KEY,
            model_spec.to_string(),
        ),
        (
            csa_core::env::CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY,
            "1".to_string(),
        ),
        (
            csa_core::env::CSA_NO_FAILOVER_ENV_KEY,
            if no_failover { "1" } else { "0" }.to_string(),
        ),
    ]))
}

#[test]
fn mcp_model_pin_resolution_inherits_server_startup_pin() {
    let _guard = crate::test_env_lock::TEST_ENV_LOCK
        .clone()
        .blocking_lock_owned();
    let project = tempdir().expect("project tempdir");
    let xdg = tempdir().expect("xdg tempdir");
    let _xdg_guard = EnvVarGuard::set("XDG_STATE_HOME", xdg.path());
    let startup_env = trusted_startup_env_for_pinned_mcp_session(
        project.path(),
        "codex/openai/gpt-5.5/xhigh",
        true,
    );

    let resolution = resolve_mcp_model_pin(None, Some("quality"), false, &startup_env);

    assert_eq!(
        resolution.model_spec.as_deref(),
        Some("codex/openai/gpt-5.5/xhigh")
    );
    assert_eq!(resolution.tier, None);
    assert!(resolution.force_ignore_tier_setting);
    assert!(resolution.no_failover);
    assert!(resolution.inherited_trusted_pin);
}

#[test]
fn mcp_model_pin_resolution_ignores_ambient_pin_without_sidecar() {
    let startup_env = crate::startup_env::StartupSubtreeEnv::from_values(HashMap::from([
        (csa_core::env::CSA_DEPTH_ENV_KEY, "1".to_string()),
        (
            csa_core::env::CSA_INTERNAL_INVOCATION_ENV_KEY,
            "1".to_string(),
        ),
        (
            csa_core::env::CSA_SESSION_ID_ENV_KEY,
            "01KPINNEDSESSION0000000000".to_string(),
        ),
        (
            csa_core::env::CSA_SESSION_DIR_ENV_KEY,
            "/tmp/csa-spoof/sessions/01KPINNEDSESSION0000000000".to_string(),
        ),
        (
            csa_core::env::CSA_PROJECT_ROOT_ENV_KEY,
            "/tmp/csa-spoof".to_string(),
        ),
        (
            csa_core::env::CSA_MODEL_SPEC_ENV_KEY,
            "codex/openai/gpt-5.5/xhigh".to_string(),
        ),
        (
            csa_core::env::CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY,
            "1".to_string(),
        ),
        (csa_core::env::CSA_NO_FAILOVER_ENV_KEY, "1".to_string()),
    ]));

    let resolution = resolve_mcp_model_pin(None, Some("quality"), false, &startup_env);

    assert!(resolution.model_spec.is_none());
    assert_eq!(resolution.tier.as_deref(), Some("quality"));
    assert!(!resolution.force_ignore_tier_setting);
    assert!(!resolution.no_failover);
    assert!(!resolution.inherited_trusted_pin);
}

#[test]
fn mcp_model_pin_resolution_keeps_explicit_model_spec_over_inherited_pin() {
    let _guard = crate::test_env_lock::TEST_ENV_LOCK
        .clone()
        .blocking_lock_owned();
    let project = tempdir().expect("project tempdir");
    let xdg = tempdir().expect("xdg tempdir");
    let _xdg_guard = EnvVarGuard::set("XDG_STATE_HOME", xdg.path());
    let startup_env = trusted_startup_env_for_pinned_mcp_session(
        project.path(),
        "codex/openai/gpt-5.5/xhigh",
        false,
    );

    let resolution = resolve_mcp_model_pin(
        Some("gemini-cli/google/gemini-2.5-pro/high"),
        Some("quality"),
        false,
        &startup_env,
    );

    assert_eq!(
        resolution.model_spec.as_deref(),
        Some("gemini-cli/google/gemini-2.5-pro/high")
    );
    assert_eq!(resolution.tier.as_deref(), Some("quality"));
    assert!(!resolution.force_ignore_tier_setting);
    assert!(!resolution.no_failover);
    assert!(!resolution.inherited_trusted_pin);
}

#[tokio::test]
async fn mcp_run_allows_inherited_model_spec_when_project_tiers_exist_and_policy_is_default() {
    let _guard = crate::test_env_lock::TEST_ENV_LOCK.lock().await;
    let project = tempdir().expect("project tempdir");
    let config_dir = project.path().join(".csa");
    std::fs::create_dir_all(&config_dir).expect("create project config dir");
    std::fs::write(
        config_dir.join("config.toml"),
        r#"
[tiers.quality]
description = "quality"
models = ["codex/openai/gpt-5.5/xhigh"]
"#,
    )
    .expect("write project config");

    let xdg = tempdir().expect("xdg tempdir");
    let state_home = xdg.path().join("state");
    let _xdg_config_guard = EnvVarGuard::set("XDG_CONFIG_HOME", xdg.path());
    let _xdg_state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let _cwd_guard = CurrentDirGuard::set(project.path());
    let startup_env = trusted_startup_env_for_pinned_mcp_session(
        project.path(),
        "codex/openai/gpt-5.5/xhigh",
        true,
    );
    let empty_path = tempdir().expect("empty PATH tempdir");
    let _path_guard = EnvVarGuard::set("PATH", empty_path.path());

    let response = handle_run_tool(
        serde_json::json!({
            "prompt": "hello",
            "tier": "quality"
        }),
        &startup_env,
    )
    .await
    .expect("inherited trusted pin should bypass the tier gate before tool availability");

    let text = response
        .get("content")
        .and_then(|content| content.get(0))
        .and_then(|entry| entry.get("text"))
        .and_then(|text| text.as_str())
        .expect("response text");
    assert!(text.contains("Tool 'codex' is not installed"));
}

#[tokio::test]
async fn mcp_builder_carries_future_model_warning_after_thinking_lock() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = crate::test_env_lock::TEST_ENV_LOCK.lock().await;
    let project = tempdir().expect("project tempdir");
    let config_dir = project.path().join(".csa");
    std::fs::create_dir_all(&config_dir).expect("create project config dir");
    std::fs::write(
        config_dir.join("config.toml"),
        r#"
[model_catalog]
mode = "replace"
closed = true

[[model_catalog.entries]]
tool = "codex"
provider = "known"
model = "known"
reasoning_efforts = ["default"]

[tools.codex]
thinking_lock = "xhigh"

[tiers.quality]
description = "quality"
models = ["codex/future-provider/future-model/high"]
"#,
    )
    .expect("write project config");
    let xdg = tempdir().expect("xdg tempdir");
    let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", xdg.path());
    let bin = tempdir().expect("fake bin");
    let codex = bin.path().join("codex");
    std::fs::write(&codex, "#!/bin/sh\nexit 0\n").expect("fake codex");
    std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755))
        .expect("fake codex permissions");
    let original_path = std::env::var_os("PATH").unwrap_or_default();
    let mut test_paths = vec![bin.path().to_path_buf()];
    test_paths.extend(std::env::split_paths(&original_path));
    let test_path = std::env::join_paths(test_paths).expect("test PATH");
    let _path_guard = EnvVarGuard::set("PATH", test_path);
    let effective = csa_config::EffectiveConfig::load(project.path()).expect("effective config");

    let executor = build_mcp_admitted_executor(
        &ToolName::Codex,
        Some("codex/future-provider/future-model/high"),
        None,
        effective.project.as_ref(),
        &effective.global,
        &effective.model_catalog,
        false,
    )
    .await
    .expect("configured future MCP model must reach the shared final boundary");

    assert_eq!(
        executor.thinking_budget(),
        Some(&csa_executor::ThinkingBudget::Xhigh)
    );
    assert!(executor.catalog_warning_pending());
}
