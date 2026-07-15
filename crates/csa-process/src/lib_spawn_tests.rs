use super::*;
use csa_resource::sandbox::ResourceCapability;
use std::collections::HashMap;

fn recorded_env(cmd: &Command) -> HashMap<String, Option<String>> {
    cmd.as_std()
        .get_envs()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.map(|v| v.to_string_lossy().into_owned()),
            )
        })
        .collect()
}

fn bwrap_plan() -> IsolationPlan {
    IsolationPlan {
        resource: ResourceCapability::None,
        filesystem: FilesystemCapability::Bwrap,
        writable_paths: vec![std::path::PathBuf::from("/tmp")],
        readable_paths: Vec::new(),
        env_overrides: HashMap::new(),
        degraded_reasons: Vec::new(),
        memory_max_mb: None,
        memory_swap_max_mb: None,
        pids_max: None,
        readonly_project_root: false,
        user_daemon_ipc: false,
        project_root: None,
        soft_limit_percent: None,
        memory_monitor_interval_seconds: None,
    }
}

fn no_filesystem_wrapper_plan_with_tmpdir(tmpdir: &str) -> IsolationPlan {
    IsolationPlan {
        resource: ResourceCapability::None,
        filesystem: FilesystemCapability::None,
        writable_paths: Vec::new(),
        readable_paths: Vec::new(),
        env_overrides: HashMap::from([("TMPDIR".to_string(), tmpdir.to_string())]),
        degraded_reasons: Vec::new(),
        memory_max_mb: None,
        memory_swap_max_mb: None,
        pids_max: None,
        readonly_project_root: false,
        user_daemon_ipc: false,
        project_root: None,
        soft_limit_percent: None,
        memory_monitor_interval_seconds: None,
    }
}

#[tokio::test]
async fn non_bwrap_spawn_applies_plan_env_overrides_over_explicit_env() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session_tmp = temp.path().join("session-tmp");
    std::fs::create_dir_all(&session_tmp).expect("create session tmpdir");
    let expected_tmpdir = session_tmp.to_string_lossy().into_owned();
    let mut original = Command::new("/bin/sh");
    original
        .arg("-c")
        .arg("printf probe > \"$TMPDIR/probe\" && printf '%s' \"$TMPDIR\"")
        .env("TMPDIR", "/usr/local/tmp");
    let plan = no_filesystem_wrapper_plan_with_tmpdir(&expected_tmpdir);

    let (child, _handle) = spawn_tool_sandboxed(
        original,
        None,
        SpawnOptions::default(),
        Some(&plan),
        "codex",
        "01KTEST",
    )
    .await
    .expect("spawn should succeed");
    let result = crate::wait_and_capture(child, crate::StreamMode::BufferOnly)
        .await
        .expect("wait should succeed");

    assert_eq!(result.exit_code, 0);
    assert_eq!(
        result.output, expected_tmpdir,
        "non-bwrap sandbox paths must still apply IsolationPlan env overrides"
    );
    assert_eq!(
        std::fs::read_to_string(session_tmp.join("probe")).expect("read tmpdir probe"),
        "probe",
        "normalized TMPDIR must be writable by the child process"
    );
}

#[test]
fn bwrap_wrapper_scrubs_ambient_subtree_contract_env() {
    let original = Command::new("/usr/bin/tool");
    let wrapped = wrap_command_with_bwrap(original, &bwrap_plan());
    let env = recorded_env(&wrapped);

    for key in csa_core::env::STARTUP_SUBTREE_ENV_KEYS {
        assert_eq!(
            env.get(*key),
            Some(&None),
            "bwrap wrapper must env_remove ambient subtree-contract key {key}"
        );
    }
}

#[test]
fn bwrap_wrapper_scrubs_ambient_git_push_authorization_env() {
    let original = Command::new("/usr/bin/tool");
    let wrapped = wrap_command_with_bwrap(original, &bwrap_plan());
    let env = recorded_env(&wrapped);

    for key in csa_core::env::GIT_PUSH_AUTHORIZATION_ENV_KEYS {
        assert_eq!(
            env.get(*key),
            Some(&None),
            "bwrap wrapper must env_remove ambient git-push authorization key {key}"
        );
    }
}

#[test]
fn bwrap_wrapper_preserves_explicit_typed_git_push_authorization() {
    let mut original = Command::new("/usr/bin/tool");
    original.env(csa_core::env::CSA_GIT_PUSH_ALLOWED_ENV_KEY, "true");

    let wrapped = wrap_command_with_bwrap(original, &bwrap_plan());
    let env = recorded_env(&wrapped);

    assert_eq!(
        env.get(csa_core::env::CSA_GIT_PUSH_ALLOWED_ENV_KEY),
        Some(&Some("true".to_string())),
        "explicit typed git-push authorization must survive bwrap wrapping"
    );
    assert_eq!(
        env.get(csa_core::env::CSA_RUN_GIT_PUSH_AUTHORIZED_ENV_KEY),
        Some(&None),
        "internal git-push marker must remain stripped"
    );
}

#[test]
fn cgroup_wrapper_scrubs_ambient_then_preserves_explicit_fresh_env() {
    let mut original = Command::new("/usr/bin/tool");
    original
        .env(csa_core::env::CSA_DEPTH_ENV_KEY, "3")
        .env(csa_core::env::CSA_INTERNAL_INVOCATION_ENV_KEY, "1");
    let config = csa_resource::cgroup::SandboxConfig {
        memory_max_mb: 1024,
        memory_swap_max_mb: None,
        pids_max: Some(64),
    };

    let wrapped = build_cgroup_scope_command(&original, "codex", "01KTEST", &config);
    let env = recorded_env(&wrapped);

    assert_eq!(
        env.get(csa_core::env::CSA_DEPTH_ENV_KEY),
        Some(&Some("3".to_string())),
        "fresh explicit CSA_DEPTH must be preserved after wrapper scrub"
    );
    assert_eq!(
        env.get(csa_core::env::CSA_INTERNAL_INVOCATION_ENV_KEY),
        Some(&Some("1".to_string())),
        "fresh explicit CSA_INTERNAL_INVOCATION must be preserved"
    );
    for key in csa_core::env::STARTUP_SUBTREE_ENV_KEYS
        .iter()
        .filter(|key| {
            **key != csa_core::env::CSA_DEPTH_ENV_KEY
                && **key != csa_core::env::CSA_INTERNAL_INVOCATION_ENV_KEY
        })
    {
        assert_eq!(
            env.get(*key),
            Some(&None),
            "cgroup wrapper must env_remove ambient subtree-contract key {key}"
        );
    }
    for key in csa_core::env::GIT_PUSH_AUTHORIZATION_ENV_KEYS {
        assert_eq!(
            env.get(*key),
            Some(&None),
            "cgroup wrapper must env_remove ambient git-push authorization key {key}"
        );
    }
}

#[test]
fn cgroup_wrapper_preserves_explicit_typed_git_push_authorization() {
    let mut original = Command::new("/usr/bin/tool");
    original.env(csa_core::env::CSA_GIT_PUSH_ALLOWED_ENV_KEY, "true");
    let config = csa_resource::cgroup::SandboxConfig {
        memory_max_mb: 1024,
        memory_swap_max_mb: None,
        pids_max: Some(64),
    };

    let wrapped = build_cgroup_scope_command(&original, "codex", "01KTEST", &config);
    let env = recorded_env(&wrapped);

    assert_eq!(
        env.get(csa_core::env::CSA_GIT_PUSH_ALLOWED_ENV_KEY),
        Some(&Some("true".to_string())),
        "explicit typed git-push authorization must survive cgroup wrapping"
    );
    assert_eq!(
        env.get(csa_core::env::CSA_RUN_GIT_PUSH_AUTHORIZED_ENV_KEY),
        Some(&None),
        "internal git-push marker must remain stripped"
    );
}
