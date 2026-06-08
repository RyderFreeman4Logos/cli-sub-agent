#[test]
fn test_stripped_env_vars_contains_git_push_authorization() {
    assert!(
        Executor::STRIPPED_ENV_VARS.contains(&csa_core::env::CSA_GIT_PUSH_ALLOWED_ENV_KEY),
        "STRIPPED_ENV_VARS must strip inherited CSA_GIT_PUSH_ALLOWED"
    );
    assert!(
        Executor::STRIPPED_ENV_VARS.contains(&csa_core::env::CSA_RUN_GIT_PUSH_AUTHORIZED_ENV_KEY),
        "STRIPPED_ENV_VARS must strip inherited CSA_RUN_GIT_PUSH_AUTHORIZED"
    );
}

#[test]
fn test_build_command_strips_git_push_authorization_without_typed_allow() {
    let exec = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: crate::codex_runtime::codex_runtime_metadata(),
    };
    let session = make_test_session();
    let extra_env = HashMap::from([
        (
            csa_core::env::CSA_GIT_PUSH_ALLOWED_ENV_KEY.to_string(),
            "true".to_string(),
        ),
        (
            csa_core::env::CSA_RUN_GIT_PUSH_AUTHORIZED_ENV_KEY.to_string(),
            "true".to_string(),
        ),
    ]);

    let (cmd, _stdin) = exec.build_command("test", None, &session, Some(&extra_env), None);
    let env_map: HashMap<&std::ffi::OsStr, Option<&std::ffi::OsStr>> =
        cmd.as_std().get_envs().collect();

    assert_eq!(
        env_map.get(std::ffi::OsStr::new(
            csa_core::env::CSA_GIT_PUSH_ALLOWED_ENV_KEY,
        )),
        Some(&None),
        "persistent command must env_remove inherited/spoofed CSA_GIT_PUSH_ALLOWED"
    );
    assert_eq!(
        env_map.get(std::ffi::OsStr::new(
            csa_core::env::CSA_RUN_GIT_PUSH_AUTHORIZED_ENV_KEY,
        )),
        Some(&None),
        "persistent command must env_remove internal git-push marker"
    );
}

#[test]
fn test_build_command_applies_typed_git_push_authorization() {
    let exec = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: crate::codex_runtime::codex_runtime_metadata(),
    };
    let session = make_test_session();

    let (cmd, _stdin) =
        exec.build_command_with_git_push_allowed("test", None, &session, None, None, true);
    let env_map: HashMap<&std::ffi::OsStr, Option<&std::ffi::OsStr>> =
        cmd.as_std().get_envs().collect();

    assert_eq!(
        env_map.get(std::ffi::OsStr::new(
            csa_core::env::CSA_GIT_PUSH_ALLOWED_ENV_KEY,
        )),
        Some(&Some(std::ffi::OsStr::new("true"))),
        "typed allow must write CSA_GIT_PUSH_ALLOWED=true"
    );
    assert_eq!(
        env_map.get(std::ffi::OsStr::new(
            csa_core::env::CSA_RUN_GIT_PUSH_AUTHORIZED_ENV_KEY,
        )),
        Some(&None),
        "internal authorization marker must not reach the leaf command"
    );
}

#[test]
fn test_build_execute_in_command_strips_git_push_authorization_without_typed_allow() {
    let exec = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: crate::codex_runtime::codex_runtime_metadata(),
    };
    let work_dir = std::path::Path::new("/tmp/test-project");
    let extra_env = HashMap::from([(
        csa_core::env::CSA_GIT_PUSH_ALLOWED_ENV_KEY.to_string(),
        "true".to_string(),
    )]);

    let (cmd, _stdin) = exec.build_execute_in_command("test", work_dir, Some(&extra_env), None);
    let env_map: HashMap<&std::ffi::OsStr, Option<&std::ffi::OsStr>> =
        cmd.as_std().get_envs().collect();

    assert_eq!(
        env_map.get(std::ffi::OsStr::new(
            csa_core::env::CSA_GIT_PUSH_ALLOWED_ENV_KEY,
        )),
        Some(&None),
        "execute_in command must env_remove inherited/spoofed CSA_GIT_PUSH_ALLOWED"
    );
}
