#[cfg(test)]
mod tests {
    use crate::cli::{Cli, Commands, PlanCommands};
    use clap::Parser;
    use std::sync::{LazyLock, Mutex};

    static SA_MODE_ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    fn restore_env_var(key: &str, original: Option<String>) {
        // SAFETY: test-scoped env mutation guarded by process-wide mutex.
        unsafe {
            match original {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }

    #[test]
    fn sa_mode_parses_for_run_review_debate() {
        let run_cli = Cli::try_parse_from(["csa", "run", "--sa-mode", "true", "prompt"])
            .expect("run cli should parse");
        match run_cli.command {
            Commands::Run { sa_mode, .. } => assert_eq!(sa_mode, Some(true)),
            _ => panic!("expected run command"),
        }

        let review_cli = Cli::try_parse_from(["csa", "review", "--sa-mode", "false", "--diff"])
            .expect("review cli should parse");
        match review_cli.command {
            Commands::Review(args) => assert_eq!(args.sa_mode, Some(false)),
            _ => panic!("expected review command"),
        }

        let debate_cli = Cli::try_parse_from(["csa", "debate", "--sa-mode", "true", "question"])
            .expect("debate cli should parse");
        match debate_cli.command {
            Commands::Debate(args) => assert_eq!(args.sa_mode, Some(true)),
            _ => panic!("expected debate command"),
        }
    }

    #[test]
    fn sa_mode_parses_for_batch_plan_and_claude_sub_agent() {
        let batch_cli = Cli::try_parse_from(["csa", "batch", "task.toml", "--sa-mode", "true"])
            .expect("batch cli should parse");
        match batch_cli.command {
            Commands::Batch { sa_mode, .. } => assert_eq!(sa_mode, Some(true)),
            _ => panic!("expected batch command"),
        }

        let plan_cli =
            Cli::try_parse_from(["csa", "plan", "run", "flow.toml", "--sa-mode", "false"])
                .expect("plan run cli should parse");
        match plan_cli.command {
            Commands::Plan { cmd } => match cmd {
                PlanCommands::Run { sa_mode, .. } => assert_eq!(sa_mode, Some(false)),
            },
            _ => panic!("expected plan command"),
        }

        let claude_cli =
            Cli::try_parse_from(["csa", "claude-sub-agent", "--sa-mode", "true", "question"])
                .expect("claude-sub-agent cli should parse");
        match claude_cli.command {
            Commands::ClaudeSubAgent(args) => assert_eq!(args.sa_mode, Some(true)),
            _ => panic!("expected claude-sub-agent command"),
        }
    }

    #[test]
    fn claude_sub_agent_rejects_unknown_codex_model_at_clap_parse() {
        let result = Cli::try_parse_from([
            "csa",
            "claude-sub-agent",
            "--model-spec",
            "codex/openai/o3/xhigh",
            "question",
        ]);
        let err = match result {
            Ok(_) => panic!("unknown model should fail clap parsing"),
            Err(err) => err,
        };
        let msg = err.to_string();
        assert!(msg.contains("o3"), "missing offending model: {msg}");
        assert!(msg.contains("gpt-5.5"), "missing valid alternative: {msg}");
    }

    #[test]
    fn validate_sa_mode_requires_root_execution_flag() {
        let _env_lock = SA_MODE_ENV_LOCK.lock().expect("sa-mode env lock poisoned");
        let original_internal = std::env::var("CSA_INTERNAL_INVOCATION").ok();
        // SAFETY: test-scoped env mutation.
        unsafe { std::env::remove_var("CSA_INTERNAL_INVOCATION") };

        let cli = Cli::try_parse_from(["csa", "run", "prompt"]).expect("cli parse should pass");
        let err =
            crate::validate_sa_mode(&cli.command, 0).expect_err("root should require sa-mode");
        assert!(
            err.to_string()
                .contains("--sa-mode true|false is required for root callers")
        );

        restore_env_var("CSA_INTERNAL_INVOCATION", original_internal);
    }

    #[test]
    fn validate_sa_mode_requires_flag_for_all_root_execution_commands() {
        let _env_lock = SA_MODE_ENV_LOCK.lock().expect("sa-mode env lock poisoned");
        let original_internal = std::env::var("CSA_INTERNAL_INVOCATION").ok();
        // SAFETY: test-scoped env mutation.
        unsafe { std::env::remove_var("CSA_INTERNAL_INVOCATION") };

        let cases: &[&[&str]] = &[
            &["csa", "review", "--diff"],
            &["csa", "debate", "question"],
            &["csa", "batch", "task.toml"],
            &["csa", "plan", "run", "flow.toml"],
            &["csa", "claude-sub-agent", "question"],
        ];

        for argv in cases {
            let cli = Cli::try_parse_from(*argv).expect("cli parse should pass");
            let err = crate::validate_sa_mode(&cli.command, 0)
                .expect_err("root execution command should require --sa-mode");
            assert!(
                err.to_string()
                    .contains("--sa-mode true|false is required for root callers"),
                "unexpected error for argv={argv:?}: {err}"
            );
        }

        restore_env_var("CSA_INTERNAL_INVOCATION", original_internal);
    }

    #[test]
    fn validate_sa_mode_rejects_forged_depth_without_internal_marker() {
        let _env_lock = SA_MODE_ENV_LOCK.lock().expect("sa-mode env lock poisoned");
        let original_internal = std::env::var("CSA_INTERNAL_INVOCATION").ok();
        // SAFETY: test-scoped env mutation.
        unsafe { std::env::remove_var("CSA_INTERNAL_INVOCATION") };

        let cli = Cli::try_parse_from(["csa", "run", "prompt"]).expect("cli parse should pass");
        let err = crate::validate_sa_mode(&cli.command, 1)
            .expect_err("depth alone should not bypass sa-mode requirement");
        assert!(
            err.to_string()
                .contains("--sa-mode true|false is required for root callers")
        );

        restore_env_var("CSA_INTERNAL_INVOCATION", original_internal);
    }

    #[test]
    fn validate_sa_mode_allows_internal_default_false() {
        let _env_lock = SA_MODE_ENV_LOCK.lock().expect("sa-mode env lock poisoned");
        let original_internal = std::env::var("CSA_INTERNAL_INVOCATION").ok();

        // SAFETY: test-scoped env mutation.
        unsafe { std::env::set_var("CSA_INTERNAL_INVOCATION", "1") };

        let cli = Cli::try_parse_from(["csa", "run", "prompt"]).expect("cli parse should pass");
        let resolved = crate::validate_sa_mode(&cli.command, 1).expect("internal call should pass");
        assert!(!resolved);

        restore_env_var("CSA_INTERNAL_INVOCATION", original_internal);
    }

    #[test]
    fn validate_sa_mode_accepts_explicit_root_values() {
        let enabled_cli = Cli::try_parse_from(["csa", "run", "--sa-mode", "true", "prompt"])
            .expect("cli parse should pass");
        assert!(crate::validate_sa_mode(&enabled_cli.command, 0).expect("should pass"));

        let disabled_cli = Cli::try_parse_from(["csa", "run", "--sa-mode", "false", "prompt"])
            .expect("cli parse should pass");
        assert!(!crate::validate_sa_mode(&disabled_cli.command, 0).expect("should pass"));
    }

    #[test]
    fn validate_sa_mode_ignores_non_execution_commands() {
        let cli = Cli::try_parse_from(["csa", "doctor"]).expect("cli parse should pass");
        let resolved = crate::validate_sa_mode(&cli.command, 0).expect("doctor should pass");
        assert!(!resolved);
    }

    #[test]
    fn sa_mode_caller_guard_emits_at_root_depth_text_mode() {
        let emitted = crate::pipeline::prompt_guard::emit_sa_mode_caller_guard(true, 0, true);
        assert!(
            emitted,
            "guard should emit when sa_mode=true, depth=0, text_mode=true"
        );
    }

    #[test]
    fn sa_mode_caller_guard_skips_when_disabled() {
        let emitted = crate::pipeline::prompt_guard::emit_sa_mode_caller_guard(false, 0, true);
        assert!(!emitted, "guard should not emit when sa_mode=false");
    }

    #[test]
    fn sa_mode_caller_guard_skips_at_child_depth() {
        let emitted = crate::pipeline::prompt_guard::emit_sa_mode_caller_guard(true, 1, true);
        assert!(!emitted, "guard should not emit at child depth");

        let emitted2 = crate::pipeline::prompt_guard::emit_sa_mode_caller_guard(true, 3, true);
        assert!(!emitted2, "guard should not emit at depth 3");
    }

    #[test]
    fn sa_mode_caller_guard_skips_in_json_mode() {
        let emitted = crate::pipeline::prompt_guard::emit_sa_mode_caller_guard(true, 0, false);
        assert!(
            !emitted,
            "guard should not emit when text_mode=false (JSON)"
        );
    }

    #[test]
    fn sa_mode_caller_guard_content_has_required_markers() {
        // Verify the guard constant contains key structural markers.
        let guard = crate::pipeline::prompt_guard::SA_MODE_CALLER_GUARD;
        assert!(
            guard.contains("<csa-caller-sa-guard>"),
            "missing opening tag"
        );
        assert!(
            guard.contains("</csa-caller-sa-guard>"),
            "missing closing tag"
        );
        assert!(guard.contains("FORBIDDEN"), "missing FORBIDDEN section");
        assert!(
            guard.contains("NARROW EXCEPTION"),
            "missing narrow exception section"
        );
        assert!(
            guard.contains("<=5-line mechanical fixes"),
            "missing trivial-fix carve-out"
        );
        assert!(guard.contains("ALLOWED"), "missing ALLOWED section");
        assert!(guard.contains("Layer 0 Manager"), "missing role identifier");
        assert!(
            guard.contains("csa run --sa-mode true"),
            "missing dispatch command"
        );
        assert!(
            guard.contains("result.toml"),
            "missing result.toml reference"
        );
    }
}
