use super::*;
use crate::test_session_sandbox::ScopedSessionSandbox;
use std::collections::HashMap;
use std::fs;

#[cfg(unix)]
#[tokio::test]
async fn execute_with_session_and_meta_injects_global_pre_session_hook_output() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&temp).await;
    let _csa_session_id_guard = ScopedEnvVarRestore::unset("CSA_SESSION_ID");
    let xdg_config_home = temp.path().join("xdg-config");
    let home_dir = temp.path().join("home");
    fs::create_dir_all(&xdg_config_home).unwrap();
    fs::create_dir_all(&home_dir).unwrap();
    let _xdg_config_guard =
        ScopedEnvVarRestore::set("XDG_CONFIG_HOME", xdg_config_home.to_str().unwrap());
    let _home_guard = ScopedEnvVarRestore::set("HOME", home_dir.to_str().unwrap());

    let project_root = temp.path();
    let config_path = csa_config::GlobalConfig::config_path().unwrap();
    assert!(
        config_path.starts_with(&xdg_config_home),
        "test config path must stay under temp XDG_CONFIG_HOME, got {}",
        config_path.display()
    );
    fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    fs::write(
        &config_path,
        r#"
[hooks.pre_session]
enabled = true
command = "printf 'fixed context from hook\n'"
transports = ["opencode"]
timeout_seconds = 2
"#,
    )
    .unwrap();
    let global_config = csa_config::GlobalConfig::load().unwrap();
    let pre_session_hook = csa_hooks::load_global_pre_session_hook_invocation();

    let bin_dir = project_root.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let fake_opencode = bin_dir.join("opencode");
    fs::write(
        &fake_opencode,
        r#"#!/bin/sh
last=""
for arg in "$@"; do
  last="$arg"
done
printf '%s' "$last" > "$CSA_CAPTURE_PROMPT"
printf 'ok\n'
"#,
    )
    .unwrap();
    let mut perms = fs::metadata(&fake_opencode).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&fake_opencode, perms).unwrap();

    let captured_prompt = project_root.join("captured-prompt.txt");
    let mut extra_env = HashMap::new();
    let inherited_path = std::env::var("PATH").unwrap_or_default();
    extra_env.insert(
        "PATH".to_string(),
        format!("{}:{inherited_path}", bin_dir.display()),
    );
    extra_env.insert(
        "CSA_CAPTURE_PROMPT".to_string(),
        captured_prompt.display().to_string(),
    );

    let executor = Executor::Opencode {
        model_override: None,
        agent: None,
        thinking_budget: None,
    };

    let execution = execute_with_session_and_meta(
        &executor,
        &ToolName::Opencode,
        "integration prompt",
        csa_core::types::OutputFormat::Json,
        None,
        false,
        Some("pre-session-hook".to_string()),
        None,
        project_root,
        None,
        Some(&extra_env),
        None,
        None,
        None,
        csa_process::StreamMode::BufferOnly,
        DEFAULT_IDLE_TIMEOUT_SECONDS,
        None,
        None,
        None,
        Some(&global_config),
        pre_session_hook,
        false, // no_fs_sandbox
        false, // readonly_project_root
        &[],
        &[],
    )
    .await
    .unwrap();

    assert_eq!(execution.execution.exit_code, 0);
    let prompt = fs::read_to_string(&captured_prompt).unwrap();
    assert!(
        prompt.starts_with(
            "<system-reminder>\nfixed context from hook\n</system-reminder>\n\nintegration prompt"
        ),
        "first transport prompt must start with pre_session injection, got: {prompt}"
    );
}
