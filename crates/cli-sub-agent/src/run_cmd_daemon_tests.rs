use super::*;

const PINNED_SPEC: &str = "codex/openai/gpt-5.5/xhigh";

fn trusted_startup_env_for_daemon_parent(
    project_root: &Path,
    spec: &str,
    no_failover: bool,
) -> StartupSubtreeEnv {
    let session = csa_session::create_session(
        project_root,
        Some("pinned daemon parent"),
        None,
        Some("codex"),
    )
    .expect("create pinned daemon parent session");
    let session_dir =
        csa_session::get_session_dir(project_root, &session.meta_session_id).expect("session dir");
    let typed_pin =
        crate::run_cmd_model_pin::resolve_subtree_model_pin(Some(spec), true, no_failover)
            .expect("typed pin");
    crate::run_cmd_model_pin::sync_subtree_model_pin_sidecar(
        project_root,
        &session.meta_session_id,
        &session_dir,
        Some(&typed_pin),
    )
    .expect("write trusted pin sidecar");

    StartupSubtreeEnv::from_values(std::collections::HashMap::from([
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
        (csa_core::env::CSA_MODEL_SPEC_ENV_KEY, spec.to_string()),
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

fn assert_daemon_wrapper_pin_resolves_for_command(command: &str, startup_env: &StartupSubtreeEnv) {
    let inherited = crate::run_cmd_model_pin::inherited_model_pin_from_startup(startup_env)
        .unwrap_or_else(|| panic!("{command} daemon wrapper must preserve inherited pin trust"));

    match command {
        "run" => {
            let mut skill_res = crate::run_cmd_tool_selection::SkillResolution {
                prompt_text: "prompt".to_string(),
                frontmatter_difficulty: None,
                resolved_skill: None,
                tool: None,
                model: None,
                thinking: None,
            };
            let mut user_explicit_tool = false;
            let resolved = crate::run_cmd_model_pin::resolve_handle_run_model_pin(
                crate::run_cmd_model_pin::RunModelPinInput {
                    model_spec: None,
                    tier: Some("tier-4-critical".to_string()),
                    auto_route: Some("complex".to_string()),
                    force_ignore_tier_setting: false,
                    no_failover: false,
                },
                Some(inherited),
                false,
                &mut skill_res,
                &mut user_explicit_tool,
            );
            assert_eq!(resolved.model_spec.as_deref(), Some(PINNED_SPEC));
            assert!(resolved.tier.is_none());
            assert!(resolved.auto_route.is_none());
            assert!(resolved.inherited_trusted_pin);
            assert!(resolved.subtree_model_pin_active);
        }
        "review" | "debate" => {
            let resolved = crate::run_cmd_model_pin::apply_inherited_pin_for_review_debate(
                None,
                Some("tier-4-critical".to_string()),
                false,
                false,
                Some(inherited),
            );
            assert_eq!(resolved.model_spec.as_deref(), Some(PINNED_SPEC));
            assert!(resolved.tier.is_none());
            assert!(resolved.force_ignore_tier_setting);
            assert!(resolved.no_failover);
            assert!(resolved.inherited);
        }
        other => panic!("unexpected command variant {other}"),
    }
}

#[test]
fn run_daemon_options_detect_omitted_stdin_prompt_without_skill() {
    let options = DaemonSpawnOptions::for_run(None, None, None, None, false, &[], false);
    assert_eq!(options.run_stdin_prompt, RunStdinPrompt::Omitted);
}

#[test]
fn run_daemon_options_do_not_capture_stdin_for_skill_only_run() {
    let options = DaemonSpawnOptions::for_run(Some("demo"), None, None, None, false, &[], false);
    assert_eq!(options.run_stdin_prompt, RunStdinPrompt::None);
}

#[test]
fn run_daemon_options_detect_positional_stdin_sentinel() {
    let options = DaemonSpawnOptions::for_run(None, Some("-"), None, None, false, &[], false);
    assert_eq!(options.run_stdin_prompt, RunStdinPrompt::PositionalSentinel);
}

#[test]
fn run_daemon_options_detect_prompt_file_stdin_sentinel() {
    let options =
        DaemonSpawnOptions::for_run(None, None, None, Some(Path::new("-")), false, &[], false);
    assert_eq!(options.run_stdin_prompt, RunStdinPrompt::PromptFileSentinel);
    assert!(options.prompt_file_to_capture.is_none());
}

#[test]
fn run_daemon_options_capture_regular_prompt_file_before_spawn() {
    let path = Path::new("RUN_PROMPT.md");
    let options = DaemonSpawnOptions::for_run(None, None, None, Some(path), false, &[], false);

    assert_eq!(options.run_stdin_prompt, RunStdinPrompt::None);
    assert_eq!(options.prompt_file_to_capture.as_deref(), Some(path));
}

#[test]
fn run_daemon_missing_prompt_file_fails_before_spawn() {
    let path = Path::new("MISSING_RUN_PROMPT.md");
    let options = DaemonSpawnOptions::for_run(None, None, None, Some(path), false, &[], false);
    let mut stdin = std::io::Cursor::new("");

    let err = read_daemon_prompt_input_if_needed_from_reader(&options, true, &mut stdin)
        .expect_err("missing run --prompt-file must fail before daemon spawn");

    let message = format!("{err:#}");
    assert!(
        message.contains("--prompt-file: failed to read"),
        "{message}"
    );
    assert!(message.contains("MISSING_RUN_PROMPT.md"), "{message}");
}

#[test]
fn prompt_file_daemon_options_detect_dev_stdin() {
    let options = DaemonSpawnOptions::for_debate(None, None, Some(Path::new("/dev/stdin")));
    assert_eq!(options.run_stdin_prompt, RunStdinPrompt::PromptFileSentinel);
}

#[test]
fn debate_daemon_options_detect_omitted_question_stdin() {
    let options = DaemonSpawnOptions::for_debate(None, None, None);
    assert_eq!(options.run_stdin_prompt, RunStdinPrompt::DebateOmitted);
    assert_eq!(
        options.prompt_file_forward_arg,
        PromptFileForwardArg::QuestionFile
    );
}

#[test]
fn debate_daemon_options_detect_question_file_capture() {
    let path = Path::new("motion.md");
    let options = DaemonSpawnOptions::for_debate(None, None, Some(path));

    assert_eq!(options.run_stdin_prompt, RunStdinPrompt::None);
    assert_eq!(options.prompt_file_to_capture.as_deref(), Some(path));
    assert_eq!(
        options.prompt_file_forward_arg,
        PromptFileForwardArg::QuestionFile
    );
}

#[test]
fn forwarded_args_append_prompt_file_for_omitted_stdin_prompt() {
    let all_args = vec![
        "csa".to_string(),
        "run".to_string(),
        "--sa-mode".to_string(),
        "true".to_string(),
    ];
    let prompt_file = Path::new("/state/session/input/stdin-prompt.txt");

    let forwarded = build_forwarded_args(
        &all_args,
        "run",
        &DaemonSpawnOptions {
            run_stdin_prompt: RunStdinPrompt::Omitted,
            ..Default::default()
        },
        Some(prompt_file),
    );

    assert_eq!(
        forwarded,
        vec![
            "--sa-mode",
            "true",
            "--prompt-file",
            "/state/session/input/stdin-prompt.txt"
        ]
    );
}

#[test]
fn forwarded_args_replace_positional_stdin_sentinel_with_prompt_file() {
    let all_args = vec![
        "csa".to_string(),
        "run".to_string(),
        "--sa-mode".to_string(),
        "true".to_string(),
        "-".to_string(),
    ];
    let prompt_file = Path::new("/state/session/input/stdin-prompt.txt");

    let forwarded = build_forwarded_args(
        &all_args,
        "run",
        &DaemonSpawnOptions {
            run_stdin_prompt: RunStdinPrompt::PositionalSentinel,
            ..Default::default()
        },
        Some(prompt_file),
    );

    assert_eq!(
        forwarded,
        vec![
            "--sa-mode",
            "true",
            "--prompt-file",
            "/state/session/input/stdin-prompt.txt"
        ]
    );
}

#[test]
fn forwarded_args_replace_prompt_file_stdin_sentinel_with_prompt_file() {
    let all_args = vec![
        "csa".to_string(),
        "debate".to_string(),
        "--sa-mode".to_string(),
        "true".to_string(),
        "--prompt-file".to_string(),
        "/dev/stdin".to_string(),
    ];
    let prompt_file = Path::new("/state/session/input/stdin-prompt.txt");

    let forwarded = build_forwarded_args(
        &all_args,
        "debate",
        &DaemonSpawnOptions {
            run_stdin_prompt: RunStdinPrompt::PromptFileSentinel,
            ..Default::default()
        },
        Some(prompt_file),
    );

    assert_eq!(
        forwarded,
        vec![
            "--sa-mode",
            "true",
            "--prompt-file",
            "/state/session/input/stdin-prompt.txt"
        ]
    );
}

#[test]
fn forwarded_args_replace_prompt_file_equals_stdin_sentinel_with_prompt_file() {
    let all_args = vec![
        "csa".to_string(),
        "run".to_string(),
        "--sa-mode".to_string(),
        "true".to_string(),
        "--prompt-file=-".to_string(),
    ];
    let prompt_file = Path::new("/state/session/input/stdin-prompt.txt");

    let forwarded = build_forwarded_args(
        &all_args,
        "run",
        &DaemonSpawnOptions {
            run_stdin_prompt: RunStdinPrompt::PromptFileSentinel,
            ..Default::default()
        },
        Some(prompt_file),
    );

    assert_eq!(
        forwarded,
        vec![
            "--sa-mode",
            "true",
            "--prompt-file",
            "/state/session/input/stdin-prompt.txt"
        ]
    );
}

#[test]
fn forwarded_args_append_question_file_for_omitted_debate_stdin() {
    let all_args = vec![
        "csa".to_string(),
        "debate".to_string(),
        "--sa-mode".to_string(),
        "true".to_string(),
    ];
    let prompt_file = Path::new("/state/session/input/stdin-prompt.txt");

    let forwarded = build_forwarded_args(
        &all_args,
        "debate",
        &DaemonSpawnOptions {
            run_stdin_prompt: RunStdinPrompt::DebateOmitted,
            prompt_file_forward_arg: PromptFileForwardArg::QuestionFile,
            ..Default::default()
        },
        Some(prompt_file),
    );

    assert_eq!(
        forwarded,
        vec![
            "--sa-mode",
            "true",
            "--question-file",
            "/state/session/input/stdin-prompt.txt"
        ]
    );
}

#[test]
fn forwarded_args_replace_question_file_with_captured_copy() {
    let all_args = vec![
        "csa".to_string(),
        "debate".to_string(),
        "--question-file".to_string(),
        "motion.md".to_string(),
    ];
    let captured = Path::new("/state/session/input/stdin-prompt.txt");

    let forwarded = build_forwarded_args(
        &all_args,
        "debate",
        &DaemonSpawnOptions {
            prompt_file_to_capture: Some(Path::new("motion.md").to_path_buf()),
            prompt_file_forward_arg: PromptFileForwardArg::QuestionFile,
            ..Default::default()
        },
        Some(captured),
    );

    assert_eq!(
        forwarded,
        vec!["--question-file", "/state/session/input/stdin-prompt.txt"]
    );
}

#[test]
fn forwarded_args_replace_prompt_file_alias_with_question_file() {
    let all_args = vec![
        "csa".to_string(),
        "debate".to_string(),
        "--prompt-file".to_string(),
        "/dev/stdin".to_string(),
    ];
    let captured = Path::new("/state/session/input/stdin-prompt.txt");

    let forwarded = build_forwarded_args(
        &all_args,
        "debate",
        &DaemonSpawnOptions {
            run_stdin_prompt: RunStdinPrompt::PromptFileSentinel,
            prompt_file_forward_arg: PromptFileForwardArg::QuestionFile,
            ..Default::default()
        },
        Some(captured),
    );

    assert_eq!(
        forwarded,
        vec!["--question-file", "/state/session/input/stdin-prompt.txt"]
    );
}

#[test]
fn debate_omitted_question_tty_fails_before_spawn() {
    let options = DaemonSpawnOptions::for_debate(None, None, None);
    let mut stdin = std::io::Cursor::new("");
    let err = read_daemon_prompt_input_if_needed_from_reader(&options, true, &mut stdin)
        .expect_err("TTY without question must fail before daemon spawn");

    let message = err.to_string();
    assert!(message.contains("debate question is empty"), "{message}");
    assert!(message.contains("--question-file QUESTION.md"), "{message}");
}

#[test]
fn debate_omitted_question_empty_stdin_fails_before_spawn() {
    let options = DaemonSpawnOptions::for_debate(None, None, None);
    let mut stdin = std::io::Cursor::new("   ");
    let err = read_daemon_prompt_input_if_needed_from_reader(&options, false, &mut stdin)
        .expect_err("empty piped stdin must fail before daemon spawn");

    let message = err.to_string();
    assert!(message.contains("debate question is empty"), "{message}");
}

#[test]
fn forwarded_args_remove_trailing_double_dash_with_stdin_sentinel() {
    let all_args = vec![
        "csa".to_string(),
        "run".to_string(),
        "--sa-mode".to_string(),
        "true".to_string(),
        "--".to_string(),
        "-".to_string(),
    ];
    let prompt_file = Path::new("/state/session/input/stdin-prompt.txt");

    let forwarded = build_forwarded_args(
        &all_args,
        "run",
        &DaemonSpawnOptions {
            run_stdin_prompt: RunStdinPrompt::PositionalSentinel,
            ..Default::default()
        },
        Some(prompt_file),
    );

    assert_eq!(
        forwarded,
        vec![
            "--sa-mode",
            "true",
            "--prompt-file",
            "/state/session/input/stdin-prompt.txt"
        ]
    );
}

#[test]
fn bounded_stdin_prompt_accepts_prompt_at_limit() {
    let prompt = "x".repeat(16);
    let read = read_bounded_stdin_prompt(std::io::Cursor::new(prompt.as_bytes()), 16)
        .expect("prompt at limit should be accepted");

    assert_eq!(read, prompt);
}

#[test]
fn bounded_stdin_prompt_rejects_prompt_over_limit() {
    let prompt = "x".repeat(17);
    let err = read_bounded_stdin_prompt(std::io::Cursor::new(prompt.as_bytes()), 16)
        .expect_err("prompt over limit should fail");

    assert!(
        err.to_string().contains("exceeds the 16 byte daemon limit"),
        "unexpected error: {err}"
    );
}

#[test]
fn daemon_child_startup_env_uses_preassigned_session_context() {
    let _lock = crate::test_env_lock::TEST_ENV_LOCK
        .clone()
        .blocking_lock_owned();
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let parent_session_id = csa_session::new_session_id();
    let actual_session_id = csa_session::new_session_id();
    let parent_session_dir = temp
        .path()
        .join("sessions")
        .join(&parent_session_id)
        .display()
        .to_string();
    let startup_env = StartupSubtreeEnv::from_values(std::collections::HashMap::from([
        (
            csa_core::env::CSA_SESSION_ID_ENV_KEY,
            parent_session_id.clone(),
        ),
        (
            csa_core::env::CSA_SESSION_DIR_ENV_KEY,
            parent_session_dir.clone(),
        ),
    ]));

    let effective = daemon_child_startup_env(
        &startup_env,
        &actual_session_id,
        Some(temp.path().to_str().expect("temp path should be utf-8")),
    )
    .expect("daemon child startup env should resolve");

    let expected_session_dir = csa_session::get_session_dir(temp.path(), &actual_session_id)
        .expect("session dir should resolve")
        .display()
        .to_string();
    assert_eq!(effective.session_id(), Some(actual_session_id.as_str()));
    assert_eq!(effective.session_dir(), Some(expected_session_dir.as_str()));
    assert_eq!(effective.parent_session(), Some(parent_session_id.as_str()));
    assert_eq!(
        effective.parent_session_dir(),
        Some(parent_session_dir.as_str())
    );
}

#[test]
fn daemon_child_startup_env_preserves_trusted_pin_for_run_review_debate() {
    let _lock = crate::test_env_lock::TEST_ENV_LOCK
        .clone()
        .blocking_lock_owned();
    let xdg = tempfile::tempdir().expect("xdg tempdir");
    let _xdg_guard = crate::test_env_lock::ScopedEnvVarRestore::set("XDG_STATE_HOME", xdg.path());
    let project = tempfile::tempdir().expect("project tempdir");
    let startup_env = trusted_startup_env_for_daemon_parent(project.path(), PINNED_SPEC, true);
    let original_session_id = startup_env
        .session_id()
        .expect("trusted fixture has session id")
        .to_string();
    let wrapper_session_id = csa_session::new_session_id();

    let effective = daemon_child_startup_env(
        &startup_env,
        &wrapper_session_id,
        Some(project.path().to_str().expect("temp path should be utf-8")),
    )
    .expect("daemon child startup env should resolve");

    assert_eq!(effective.session_id(), Some(wrapper_session_id.as_str()));
    assert_eq!(
        effective.parent_session(),
        Some(original_session_id.as_str())
    );
    for command in ["run", "review", "debate"] {
        assert_daemon_wrapper_pin_resolves_for_command(command, &effective);
    }
}

#[test]
fn daemon_child_startup_env_does_not_trust_ambient_pin_without_sidecar() {
    let _lock = crate::test_env_lock::TEST_ENV_LOCK
        .clone()
        .blocking_lock_owned();
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let wrapper_session_id = csa_session::new_session_id();
    let startup_env = StartupSubtreeEnv::from_values(std::collections::HashMap::from([
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
            temp.path().display().to_string(),
        ),
        (
            csa_core::env::CSA_MODEL_SPEC_ENV_KEY,
            PINNED_SPEC.to_string(),
        ),
        (
            csa_core::env::CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY,
            "1".to_string(),
        ),
        (csa_core::env::CSA_NO_FAILOVER_ENV_KEY, "1".to_string()),
    ]));

    let effective = daemon_child_startup_env(
        &startup_env,
        &wrapper_session_id,
        Some(temp.path().to_str().expect("temp path should be utf-8")),
    )
    .expect("daemon child startup env should resolve");

    assert!(
        crate::run_cmd_model_pin::inherited_model_pin_from_startup(&effective).is_none(),
        "raw ambient pin env must still fail closed after daemon wrapper rewrite"
    );
}

#[test]
fn daemon_started_session_is_waitable_and_listed_before_marker_emit() {
    let _lock = crate::test_env_lock::TEST_ENV_LOCK
        .clone()
        .blocking_lock_owned();
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let project_root = temp.path().join("project");
    std::fs::create_dir_all(&project_root).expect("project root should be created");
    let session_id = csa_session::new_session_id();
    let session_root =
        csa_session::get_session_root(&project_root).expect("session root should resolve");
    let session_dir = session_root.join("sessions").join(&session_id);

    persist_daemon_placeholder_session(&project_root, &session_dir, &session_id, "review")
        .expect("placeholder session should persist");

    verify_daemon_session_waitable(&project_root, &session_id)
        .expect("placeholder session should be waitable before marker emission");
    let resolved = crate::session_cmds::resolve_session_prefix_with_global_fallback(
        &project_root,
        &session_id,
    )
    .expect("session wait prefix resolution should see the session");
    assert_eq!(resolved.session_id, session_id);

    let listed = csa_session::list_sessions(&project_root, None)
        .expect("session list should load persisted placeholder");
    assert!(
        listed
            .iter()
            .any(|session| session.meta_session_id == session_id),
        "session list must include a session id printed in CSA:SESSION_STARTED"
    );
    let _ = std::fs::remove_dir_all(session_root);
}

#[test]
fn daemon_started_marker_is_blocked_when_session_state_is_unreadable() {
    let _lock = crate::test_env_lock::TEST_ENV_LOCK
        .clone()
        .blocking_lock_owned();
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let project_root = temp.path().join("project");
    std::fs::create_dir_all(&project_root).expect("project root should be created");
    let session_id = csa_session::new_session_id();
    let session_root =
        csa_session::get_session_root(&project_root).expect("session root should resolve");
    let session_dir = session_root.join("sessions").join(&session_id);
    std::fs::create_dir_all(session_dir.join("input")).expect("session dir should be created");
    std::fs::create_dir_all(session_dir.join("output")).expect("session dir should be created");

    let err = verify_daemon_session_waitable(&project_root, &session_id)
        .expect_err("unreadable state must block CSA:SESSION_STARTED emission");
    let message = format!("{err:#}");
    assert!(
        message.contains("state is not readable by `csa session wait`")
            || message.contains("Session"),
        "error should explain why no usable wait command can be printed: {message}"
    );
    let _ = std::fs::remove_dir_all(session_root);
}
