use super::*;

#[test]
fn run_daemon_options_detect_omitted_stdin_prompt_without_skill() {
    let options = DaemonSpawnOptions::for_run(None, None, None, None, false, &[]);
    assert_eq!(options.run_stdin_prompt, RunStdinPrompt::Omitted);
}

#[test]
fn run_daemon_options_do_not_capture_stdin_for_skill_only_run() {
    let options = DaemonSpawnOptions::for_run(Some("demo"), None, None, None, false, &[]);
    assert_eq!(options.run_stdin_prompt, RunStdinPrompt::None);
}

#[test]
fn run_daemon_options_detect_positional_stdin_sentinel() {
    let options = DaemonSpawnOptions::for_run(None, Some("-"), None, None, false, &[]);
    assert_eq!(options.run_stdin_prompt, RunStdinPrompt::PositionalSentinel);
}

#[test]
fn run_daemon_options_detect_prompt_file_stdin_sentinel() {
    let options = DaemonSpawnOptions::for_run(None, None, None, Some(Path::new("-")), false, &[]);
    assert_eq!(options.run_stdin_prompt, RunStdinPrompt::PromptFileSentinel);
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
    assert!(message.contains("--question-file <path>"), "{message}");
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
        (csa_core::env::CSA_SESSION_DIR_ENV_KEY, parent_session_dir),
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
}
