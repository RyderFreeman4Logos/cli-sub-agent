use super::*;
use crate::test_session_sandbox::ScopedSessionSandbox;

#[cfg(unix)]
#[tokio::test]
async fn handle_review_fix_loop_uses_effective_fallback_tool() {
    use std::os::unix::fs::PermissionsExt;

    let project_dir = setup_git_repo();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    if which::which("bwrap").is_err() {
        eprintln!("skipping: bwrap not installed (CI gap, see #987)");
        return;
    }
    let bin_dir = project_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let opencode_count_path = project_dir.path().join("opencode-count.txt");

    std::fs::write(
        bin_dir.join("gemini"),
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf 'gemini-cli 1.0.0\\n'\n  exit 0\nfi\nprintf \"reason: 'QUOTA_EXHAUSTED'\\n\" >&2\nexit 1\n",
    )
    .unwrap();
    let opencode_stub = format!(
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf 'opencode 1.0.0\\n'\n  exit 0\nfi\ncount=$(cat \"{}\" 2>/dev/null || printf '0')\ncount=$((count + 1))\nprintf '%s' \"$count\" > \"{}\"\nif [ \"$count\" -eq 1 ]; then\n  printf '%s\\n' '<!-- CSA:SECTION:summary -->' 'FAIL' '<!-- CSA:SECTION:summary:END -->' '<!-- CSA:SECTION:details -->' 'Found issue in tracked.txt.' '<!-- CSA:SECTION:details:END -->'\nelse\n  printf '%s\\n' '<!-- CSA:SECTION:summary -->' 'PASS' '<!-- CSA:SECTION:summary:END -->' '<!-- CSA:SECTION:details -->' 'Issue fixed.' '<!-- CSA:SECTION:details:END -->'\nfi\n",
        opencode_count_path.display(),
        opencode_count_path.display()
    );
    std::fs::write(bin_dir.join("opencode"), &opencode_stub).unwrap();
    for binary in ["gemini", "opencode"] {
        let path = bin_dir.join(binary);
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let patched_path = format!("{}:{inherited_path}", bin_dir.display());
    let _path_guard = ScopedEnvVarRestore::set("PATH", &patched_path);

    let mut config = project_config_with_enabled_tools(&["gemini-cli", "opencode"]);
    config.review = Some(csa_config::ReviewConfig {
        gate_command: Some("true".to_string()),
        ..Default::default()
    });
    config.tools.get_mut("gemini-cli").unwrap().restrictions = Some(ToolRestrictions {
        allow_edit_existing_files: false,
        allow_write_new_files: false,
    });
    config.tiers.insert(
        "quality".to_string(),
        csa_config::config::TierConfig {
            description: "quality".to_string(),
            models: vec![
                "gemini-cli/google/gemini-3.1-pro-preview/xhigh".to_string(),
                "opencode/anthropic/claude-sonnet-4-5-20250929/default".to_string(),
            ],
            strategy: csa_config::TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );

    let global = GlobalConfig::default();
    let initial = execute_review_for_tests(
        ToolName::GeminiCli,
        "scope=files:tracked.txt mode=review-and-fix security=auto".to_string(),
        None,
        None,
        Some("gemini-cli/google/gemini-3.1-pro-preview/xhigh".to_string()),
        Some("quality".to_string()),
        true,
        None,
        "review: fix-loop-effective-fallback-tool".to_string(),
        project_dir.path(),
        Some(&config),
        &global,
        ReviewRoutingMetadata {
            project_profile: ProjectProfile::Unknown,
            detection_method: "auto",
        },
        csa_process::StreamMode::BufferOnly,
        crate::pipeline::DEFAULT_IDLE_TIMEOUT_SECONDS,
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
    .expect("initial review should fall back to opencode");
    assert_eq!(initial.executed_tool, ToolName::Opencode);

    let exit_code = super::fix::run_fix_loop(super::fix::FixLoopContext {
        effective_tool: initial.executed_tool,
        config: Some(&config),
        global_config: &global,
        review_model: None,
        effective_tier_model_spec: initial.routed_to.clone(),
        review_thinking: None,
        review_routing: ReviewRoutingMetadata {
            project_profile: ProjectProfile::Unknown,
            detection_method: "auto",
        },
        stream_mode: csa_process::StreamMode::BufferOnly,
        idle_timeout_seconds: crate::pipeline::DEFAULT_IDLE_TIMEOUT_SECONDS,
        initial_response_timeout_seconds: None,
        force_override_user_config: false,
        force_ignore_tier_setting: false,
        no_failover: false,
        no_fs_sandbox: false,
        extra_writable: &[],
        extra_readable: &[],
        timeout: None,
        project_root: project_dir.path(),
        scope: "files:tracked.txt".to_string(),
        decision: ReviewDecision::Fail.as_str().to_string(),
        verdict: "HAS_ISSUES".to_string(),
        max_rounds: 1,
        initial_session_id: initial.execution.meta_session_id.clone(),
        review_iterations: 0,
    })
    .await
    .expect("fix loop should use fallback tool");
    assert_eq!(exit_code, 0);
    assert_eq!(
        std::fs::read_to_string(&opencode_count_path).unwrap(),
        "2",
        "opencode must handle both the fallback review round and the fix round"
    );
}
