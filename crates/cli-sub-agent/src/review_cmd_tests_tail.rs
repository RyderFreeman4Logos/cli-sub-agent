use super::output::{detect_tool_diagnostic, is_review_output_empty};
use super::*;
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_config::{ProjectProfile, ToolRestrictions};
use std::path::Path;
use tempfile::tempdir;

// --- is_worktree_submodule tests ---

#[test]
fn worktree_submodule_detected_when_gitdir_has_both_markers() {
    let dir = tempdir().unwrap();
    std::fs::write(
        dir.path().join(".git"),
        "gitdir: ../../../../.git/worktrees/my-wt/modules/crates/my-sub\n",
    )
    .unwrap();
    assert!(is_worktree_submodule(dir.path()));
}

#[test]
fn worktree_submodule_not_detected_for_normal_repo() {
    let dir = tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    assert!(!is_worktree_submodule(dir.path()));
}

#[test]
fn worktree_submodule_not_detected_for_plain_submodule() {
    let dir = tempdir().unwrap();
    std::fs::write(
        dir.path().join(".git"),
        "gitdir: ../../.git/modules/crates/my-sub\n",
    )
    .unwrap();
    assert!(!is_worktree_submodule(dir.path()));
}

#[test]
fn worktree_submodule_not_detected_for_plain_worktree() {
    let dir = tempdir().unwrap();
    std::fs::write(
        dir.path().join(".git"),
        "gitdir: /repo/.git/worktrees/my-wt\n",
    )
    .unwrap();
    assert!(!is_worktree_submodule(dir.path()));
}

// --- is_review_output_empty tests ---

#[test]
fn empty_string_is_empty_output() {
    assert!(is_review_output_empty(""));
}

#[test]
fn whitespace_only_is_empty_output() {
    assert!(is_review_output_empty("  \n  \n  "));
}

#[test]
fn prompt_guards_only_is_empty_output() {
    let output = r#"[csa-hook] reverse prompt injection for caller (guards=2)
<csa-caller-prompt-injection guards="2">
<prompt-guard name="branch-context">
BRANCH CONTEXT: The caller is on feature branch 'fix/test'.
</prompt-guard>
<prompt-guard name="commit-workflow">
WORKFLOW: 2 unpushed commit(s) on 'fix/test'.
</prompt-guard>
</csa-caller-prompt-injection>
"#;
    assert!(is_review_output_empty(output));
}

#[test]
fn substantive_output_is_not_empty() {
    let output =
        "## Review Findings\n\n1. Missing error handling in foo.rs:42\n\nVerdict: HAS_ISSUES";
    assert!(!is_review_output_empty(output));
}

#[test]
fn empty_section_wrappers_is_empty_output() {
    let output = "<!-- CSA:SECTION:summary -->\n<!-- CSA:SECTION:summary:END -->\n\
                  <!-- CSA:SECTION:details -->\n<!-- CSA:SECTION:details:END -->\n";
    assert!(is_review_output_empty(output));
}

#[test]
fn prompt_guards_plus_content_is_not_empty() {
    let output = r#"[csa-hook] reverse prompt injection for caller (guards=1)
<csa-caller-prompt-injection guards="1">
<prompt-guard name="branch-context">
BRANCH CONTEXT: feature branch
</prompt-guard>
</csa-caller-prompt-injection>
## Review Findings
No issues found. Verdict: CLEAN
"#;
    assert!(!is_review_output_empty(output));
}

// --- detect_tool_diagnostic tests ---

#[test]
fn test_detect_tool_diagnostic_empty_strings_returns_none() {
    assert!(detect_tool_diagnostic("", "").is_none());
}

#[test]
fn test_detect_tool_diagnostic_normal_output_returns_none() {
    let stdout = "## Review Findings\n\nNo issues found.\n\nVerdict: CLEAN";
    let stderr = "Loaded 3 MCP servers\n";
    assert!(detect_tool_diagnostic(stdout, stderr).is_none());
}

#[test]
fn test_detect_tool_diagnostic_mcp_issues_in_stdout_returns_diagnostic() {
    let stdout = "Some output\nMCP issues detected\nMore output";
    let result = detect_tool_diagnostic(stdout, "");
    assert!(result.is_some());
    let msg = result.unwrap();
    assert!(msg.contains("MCP init degraded"));
    assert!(msg.contains("csa doctor"));
    assert!(msg.contains("--force-ignore-tier-setting"));
}

#[test]
fn test_detect_tool_diagnostic_mcp_issues_in_stderr_returns_diagnostic() {
    let stderr = "Warning: MCP issues detected during startup";
    let result = detect_tool_diagnostic("", stderr);
    assert!(result.is_some());
    assert!(result.unwrap().contains("MCP init degraded"));
}

#[test]
fn test_detect_tool_diagnostic_mcp_list_in_stderr_returns_diagnostic() {
    let stderr = "Run /mcp list to check server status";
    let result = detect_tool_diagnostic("", stderr);
    assert!(result.is_some());
    assert!(result.unwrap().contains("csa doctor"));
}

#[test]
fn detect_tool_diagnostic_returns_quota_message_for_gemini_429() {
    let stderr = r#"{"code": 429, "reason": "QUOTA_EXHAUSTED", "message": "You have exhausted your capacity on this model. Your quota will reset after 16h8m32s."}"#;
    let result = detect_tool_diagnostic("", stderr);
    assert!(result.is_some());
    let msg = result.unwrap();
    assert!(msg.contains("OAuth quota exhausted"), "got: {msg}");
    assert!(msg.contains("GEMINI_API_KEY"), "got: {msg}");
}

#[test]
fn detect_tool_diagnostic_prefers_quota_over_mcp_when_both_present() {
    let stderr = "MCP issues detected. Run /mcp list for status.\ncause: {code: 429, reason: 'QUOTA_EXHAUSTED'}";
    let result = detect_tool_diagnostic("", stderr);
    assert!(result.is_some());
    let msg = result.unwrap();
    assert!(msg.contains("OAuth quota exhausted"), "got: {msg}");
    assert!(
        !msg.contains("Run `csa doctor`"),
        "should not emit MCP guidance when quota is root cause; got: {msg}"
    );
}

#[test]
fn detect_tool_diagnostic_still_returns_mcp_when_only_mcp_markers() {
    let stderr = "MCP issues detected. Run /mcp list for status.";
    let result = detect_tool_diagnostic("", stderr);
    assert!(result.is_some());
    let msg = result.unwrap();
    assert!(msg.contains("MCP init degraded"), "got: {msg}");
}

#[test]
fn test_detect_tool_diagnostic_unrelated_mcp_text_returns_none() {
    let stdout = "MCP servers loaded successfully";
    let stderr = "Connected to 2 MCP backends";
    assert!(detect_tool_diagnostic(stdout, stderr).is_none());
}

#[test]
fn test_is_review_output_empty_with_mcp_diagnostic_text_is_not_empty() {
    let output = "MCP issues detected\nRun /mcp list to check";
    assert!(!is_review_output_empty(output));
}

// --- review tier precedence tests ---

#[test]
fn resolve_review_model_prefers_cli_over_config() {
    let model = resolve_review_model(Some("sonnet"), Some("opus"), false);
    assert_eq!(model.as_deref(), Some("sonnet"));
}

#[test]
fn resolve_review_model_ignores_config_when_tier_active() {
    let model = resolve_review_model(None, Some("opus"), true);
    assert_eq!(model, None);
}

#[test]
fn resolve_review_tool_prefers_model_spec_override() {
    let global = GlobalConfig::default();
    let cfg = project_config_with_enabled_tools(&["codex"]);
    let tool = resolve_review_tool(
        None,
        Some("codex/openai/gpt-5.4/xhigh"),
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
        false,
        None,
        false,
    )
    .unwrap();
    assert!(matches!(tool.0, ToolName::Codex));
    assert_eq!(tool.1.as_deref(), Some("codex/openai/gpt-5.4/xhigh"));
}

#[test]
fn resolve_review_thinking_uses_config_without_tier() {
    let thinking = resolve_review_thinking(None, Some("high"), false);
    assert_eq!(thinking.as_deref(), Some("high"));
}

#[test]
fn resolve_review_thinking_ignores_config_when_tier_active() {
    let thinking = resolve_review_thinking(None, Some("high"), true);
    assert_eq!(thinking, None);
}

#[test]
fn review_cli_parses_model_spec_and_no_failover_flags() {
    let args = parse_review_args(&[
        "csa",
        "review",
        "--diff",
        "--model-spec",
        "codex/openai/gpt-5.4/xhigh",
        "--no-failover",
    ]);
    assert_eq!(
        args.model_spec.as_deref(),
        Some("codex/openai/gpt-5.4/xhigh")
    );
    assert!(args.no_failover);
}

// --- --thinking silent acceptance tests ---

#[test]
fn thinking_flag_accepted_silently() {
    let args = parse_review_args(&["csa", "review", "--thinking", "xhigh", "--diff"]);
    assert_eq!(args.thinking.as_deref(), Some("xhigh"));
    assert!(args.diff);
}

#[test]
fn thinking_flag_optional() {
    let args = parse_review_args(&["csa", "review", "--diff"]);
    assert!(args.thinking.is_none());
}

// --- verify_review_skill_available tests ---

#[test]
fn review_cli_parses_explicit_review_mode() {
    let args = parse_review_args(&["csa", "review", "--diff", "--review-mode", "red-team"]);
    assert_eq!(args.effective_review_mode(), ReviewMode::RedTeam);
    assert_eq!(args.effective_security_mode(), "on");
}

#[test]
fn review_cli_parses_red_team_shorthand() {
    let args = parse_review_args(&["csa", "review", "--red-team", "--diff"]);
    assert!(args.red_team);
    assert_eq!(args.effective_review_mode(), ReviewMode::RedTeam);
    assert_eq!(args.effective_security_mode(), "on");
}

#[test]
fn review_cli_rejects_red_team_with_security_off() {
    let err = parse_or_validate_review_error(&[
        "csa",
        "review",
        "--red-team",
        "--diff",
        "--security-mode",
        "off",
    ]);
    assert_eq!(err.kind(), ErrorKind::ArgumentConflict);
}

#[test]
fn review_cli_rejects_review_mode_red_team_with_security_off() {
    let err = parse_or_validate_review_error(&[
        "csa",
        "review",
        "--review-mode",
        "red-team",
        "--diff",
        "--security-mode",
        "off",
    ]);
    assert_eq!(err.kind(), ErrorKind::ArgumentConflict);
}

// --- build_review_instruction tests ---

#[test]
fn test_build_review_instruction_basic() {
    let result = build_review_instruction(
        "uncommitted",
        "review-only",
        "auto",
        ReviewMode::Standard,
        None,
    );
    assert!(result.contains("scope=uncommitted"));
    assert!(result.contains("mode=review-only"));
    assert!(result.contains("security_mode=auto"));
    assert!(result.contains("review_mode=standard"));
    assert!(result.contains("csa-review skill"));
    assert!(!result.contains("git diff"));
    assert!(!result.contains("Pass 1:"));
}

#[test]
fn test_build_review_instruction_with_context() {
    let context = ResolvedReviewContext {
        path: "/path/to/TODO.md".to_string(),
        kind: ResolvedReviewContextKind::TodoMarkdown,
    };
    let result = build_review_instruction(
        "range:main...HEAD",
        "review-only",
        "on",
        ReviewMode::Standard,
        Some(&context),
    );
    assert!(result.contains("scope=range:main...HEAD"));
    assert!(result.contains("context=/path/to/TODO.md"));
}

#[test]
fn test_build_review_instruction_fix_mode() {
    let result = build_review_instruction(
        "uncommitted",
        "review-and-fix",
        "auto",
        ReviewMode::Standard,
        None,
    );
    assert!(result.contains("mode=review-and-fix"));
}

#[test]
fn test_build_review_instruction_no_diff_content() {
    let result = build_review_instruction(
        "uncommitted",
        "review-only",
        "auto",
        ReviewMode::Standard,
        None,
    );
    assert!(
        result.len() < 2200,
        "Instruction should stay bounded even with the design anchor, got {} chars",
        result.len()
    );
    assert!(!result.contains("git diff"));
    assert!(!result.contains("Pass 1:"));
    assert!(result.contains("Design preferences vs correctness bugs"));
}

#[test]
fn test_build_review_instruction_contains_safety_preamble() {
    let result = build_review_instruction(
        "uncommitted",
        "review-only",
        "auto",
        ReviewMode::Standard,
        None,
    );
    assert!(result.contains("INSIDE a CSA subprocess"));
    assert!(result.contains("REVIEW-ONLY SAFETY"));
    assert!(
        !result.contains("Do NOT invoke"),
        "Legacy blanket anti-csa text must not be reintroduced (breaks fractal recursion contract)"
    );
}

#[test]
fn test_build_review_instruction_for_project_includes_rust_profile() {
    let project_dir = tempdir().unwrap();
    std::fs::write(
        project_dir.path().join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    let (instruction, routing) = build_review_instruction_for_project(
        "uncommitted",
        "review-only",
        "auto",
        ReviewMode::Standard,
        None,
        project_dir.path(),
        resolve::ReviewProjectPromptOptions {
            project_config: None,
            prior_rounds_section: None,
        },
    );

    assert!(instruction.contains("[project_profile: rust]"));
    assert_eq!(routing.detection_method, "auto");
}

#[test]
fn test_build_review_instruction_for_project_includes_unknown_profile_for_empty_project() {
    let project_dir = tempdir().unwrap();

    let (instruction, routing) = build_review_instruction_for_project(
        "uncommitted",
        "review-only",
        "auto",
        ReviewMode::Standard,
        None,
        project_dir.path(),
        resolve::ReviewProjectPromptOptions {
            project_config: None,
            prior_rounds_section: None,
        },
    );

    assert!(instruction.contains("[project_profile: unknown]"));
    assert_eq!(routing.detection_method, "auto");
}

#[test]
fn resolve_review_context_parses_spec_toml() {
    let temp = tempdir().unwrap();
    let spec_path = temp.path().join("spec.toml");
    std::fs::write(
        &spec_path,
        toml::to_string_pretty(&sample_spec_document(
            "01JTESTPLAN0000000000000000",
            "criterion-login",
        ))
        .unwrap(),
    )
    .unwrap();

    let context = resolve_review_context(Some(spec_path.to_str().unwrap()), temp.path(), false)
        .unwrap()
        .unwrap();

    assert_eq!(context.path, spec_path.display().to_string());
    match context.kind {
        ResolvedReviewContextKind::SpecToml { spec } => {
            assert_eq!(spec.plan_ulid, "01JTESTPLAN0000000000000000");
            assert_eq!(spec.criteria.len(), 1);
        }
        other => panic!("expected spec context, got {other:?}"),
    }
}

#[test]
fn resolve_review_context_keeps_markdown_path_behavior() {
    let context = resolve_review_context(Some("/tmp/TODO.md"), std::path::Path::new("/tmp"), false)
        .unwrap()
        .unwrap();

    assert_eq!(context.path, "/tmp/TODO.md");
    assert!(matches!(
        context.kind,
        ResolvedReviewContextKind::TodoMarkdown
    ));
}

#[test]
fn discover_review_context_for_branch_prefers_latest_spec() {
    let temp = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(temp.path().to_path_buf());

    let first = manager
        .create("First", Some("feat/spec-intent-review"))
        .unwrap();
    manager
        .save_spec(
            &first.timestamp,
            &sample_spec_document("01JFIRSTPLAN000000000000000", "criterion-first"),
        )
        .unwrap();

    let second = manager
        .create("Second", Some("feat/spec-intent-review"))
        .unwrap();
    manager
        .save_spec(
            &second.timestamp,
            &sample_spec_document("01JSECONDPLAN00000000000000", "criterion-second"),
        )
        .unwrap();

    let context = discover_review_context_for_branch(&manager, "feat/spec-intent-review")
        .unwrap()
        .unwrap();

    assert_eq!(
        context.path,
        manager.spec_path(&second.timestamp).display().to_string()
    );
    match context.kind {
        ResolvedReviewContextKind::SpecToml { spec } => {
            assert_eq!(spec.plan_ulid, "01JSECONDPLAN00000000000000");
            assert_eq!(spec.criteria[0].id, "criterion-second");
        }
        other => panic!("expected spec context, got {other:?}"),
    }
}

#[test]
fn discover_review_context_for_branch_skips_when_spec_missing() {
    let temp = tempdir().unwrap();
    let manager = TodoManager::with_base_dir(temp.path().to_path_buf());
    manager.create("No Spec", Some("feat/no-spec")).unwrap();

    let context = discover_review_context_for_branch(&manager, "feat/no-spec").unwrap();
    assert!(context.is_none());
}

#[test]
fn verify_review_skill_missing_returns_actionable_error() {
    let tmp = tempfile::TempDir::new().unwrap();
    let err = verify_review_skill_available(tmp.path(), false).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("Review pattern not found"),
        "should mention missing pattern: {msg}"
    );
    assert!(
        msg.contains("weave install"),
        "should include install guidance: {msg}"
    );
    assert!(
        msg.contains("does NOT install"),
        "should clarify skill install vs pattern install: {msg}"
    );
    assert!(
        msg.contains("patterns/csa-review"),
        "should list searched paths: {msg}"
    );
}

#[test]
fn verify_review_skill_present_succeeds() {
    let tmp = tempfile::TempDir::new().unwrap();
    // Pattern layout: .csa/patterns/csa-review/skills/csa-review/SKILL.md
    let skill_dir = tmp
        .path()
        .join(".csa")
        .join("patterns")
        .join("csa-review")
        .join("skills")
        .join("csa-review");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "# CSA Review Skill\nStructured code review.",
    )
    .unwrap();

    assert!(verify_review_skill_available(tmp.path(), false).is_ok());
}

#[test]
fn verify_review_skill_no_fallback_without_skill() {
    // Ensure no execution path silently downgrades when skill is missing.
    // The verify function must return Err — it must NOT return Ok with a warning.
    let tmp = tempfile::TempDir::new().unwrap();
    let result = verify_review_skill_available(tmp.path(), false);
    assert!(
        result.is_err(),
        "missing skill must be a hard error, not a warning"
    );
}

#[test]
fn verify_review_skill_allow_fallback_without_skill() {
    let tmp = tempfile::TempDir::new().unwrap();
    let result = verify_review_skill_available(tmp.path(), true);
    assert!(
        result.is_ok(),
        "missing skill should downgrade to warning when fallback is enabled"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn execute_review_ignores_inherited_csa_session_id_without_explicit_session() {
    use std::os::unix::fs::PermissionsExt;

    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    let _session_guard = ScopedEnvVarRestore::set("CSA_SESSION_ID", "01K00000000000000000000000");
    let bin_dir = project_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let fake_opencode = bin_dir.join("opencode");
    std::fs::write(&fake_opencode, "#!/bin/sh\nprintf 'review-ok\\n'\n").unwrap();
    let mut perms = std::fs::metadata(&fake_opencode).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&fake_opencode, perms).unwrap();

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let patched_path = format!("{}:{inherited_path}", bin_dir.display());
    let _path_guard = ScopedEnvVarRestore::set("PATH", &patched_path);
    let global = GlobalConfig::default();
    let result = execute_review(
        ToolName::Opencode,
        "scope=uncommitted mode=review-only security=auto".to_string(),
        None,
        None,
        None,  // tier_model_spec
        None,  // tier_name
        false, // tier_fallback_enabled
        None,  // thinking
        "review: stale-session-regression".to_string(),
        project_dir.path(),
        None,
        &global,
        None,
        ReviewRoutingMetadata {
            project_profile: ProjectProfile::Unknown,
            detection_method: "auto",
        },
        csa_process::StreamMode::BufferOnly,
        crate::pipeline::DEFAULT_IDLE_TIMEOUT_SECONDS,
        None,  // initial_response_timeout_seconds
        false, // force_override_user_config
        false, // force_ignore_tier_setting
        false, // no_failover
        false, // no_fs_sandbox
        false, // readonly_project_root
        &[],   // extra_writable
        &[],   // extra_readable
    )
    .await;

    let execution = result.expect("review should ignore inherited stale CSA_SESSION_ID");
    assert_eq!(execution.execution.execution.exit_code, 0);
}

fn write_review_project_config(project_root: &Path, config: &ProjectConfig) {
    let config_path = ProjectConfig::config_path(project_root);
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(config_path, toml::to_string_pretty(config).unwrap()).unwrap();
}

fn install_pattern(project_root: &Path, name: &str) {
    let skill_dir = project_root
        .join(".csa")
        .join("patterns")
        .join(name)
        .join("skills")
        .join(name);
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# test pattern\n").unwrap();
}

#[tokio::test]
async fn handle_review_persists_result_for_direct_tool_tier_rejection() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    let mut config = project_config_with_enabled_tools(&["gemini-cli", "codex"]);
    config.tiers.insert(
        "default".to_string(),
        csa_config::config::TierConfig {
            description: "Test tier".to_string(),
            models: vec!["gemini-cli/google/default/xhigh".to_string()],
            strategy: csa_config::TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );
    write_review_project_config(project_dir.path(), &config);
    install_pattern(project_dir.path(), "csa-review");

    let cd = project_dir.path().display().to_string();
    let args = parse_review_args(&[
        "csa",
        "review",
        "--cd",
        &cd,
        "--files",
        "src/lib.rs",
        "--tool",
        "codex",
    ]);

    let err = handle_review(args, 0)
        .await
        .expect_err("direct --tool tier rejection must fail");
    assert!(
        err.chain().any(|cause| cause
            .to_string()
            .contains("restricted when tiers are configured")),
        "unexpected error chain: {err:#}"
    );

    let sessions = csa_session::list_sessions(project_dir.path(), None).unwrap();
    assert_eq!(sessions.len(), 1, "expected one failed review session");

    let result = csa_session::load_result(project_dir.path(), &sessions[0].meta_session_id)
        .unwrap()
        .expect("result.toml must be written for review tier rejection");
    assert_eq!(result.status, "failure");
    assert_eq!(result.exit_code, 1);
    assert!(result.summary.contains("pre-exec:"));
    assert!(
        result
            .summary
            .contains("restricted when tiers are configured")
    );
}

// --- fix loop restriction gate tests ---

/// When a tool has `allow_edit_existing_files = false`, the fix loop gate
/// condition (`config.is_none_or(|cfg| cfg.can_tool_edit_existing(...))`)
/// must evaluate to `false`, causing the fix loop to be skipped.
#[test]
fn fix_gate_blocks_restricted_tool() {
    let mut cfg = project_config_with_enabled_tools(&["gemini-cli"]);
    cfg.tools.get_mut("gemini-cli").unwrap().restrictions = Some(ToolRestrictions {
        allow_edit_existing_files: false,
        allow_write_new_files: false,
    });

    let can_edit = Some(&cfg).is_none_or(|c| c.can_tool_edit_existing("gemini-cli"));
    assert!(!can_edit, "restricted tool must block fix loop");
}

#[test]
fn fix_gate_allows_unrestricted_tool() {
    let cfg = project_config_with_enabled_tools(&["claude-code"]);

    let can_edit = Some(&cfg).is_none_or(|c| c.can_tool_edit_existing("claude-code"));
    assert!(can_edit, "unrestricted tool must allow fix loop");
}

#[test]
fn fix_gate_allows_when_no_config() {
    let can_edit: bool =
        Option::<&ProjectConfig>::None.is_none_or(|c| c.can_tool_edit_existing("gemini-cli"));
    assert!(can_edit, "absent config must default to allowing fix");
}

// --- transport routing defaults ---

#[test]
fn gemini_cli_routes_to_legacy_transport() {
    use csa_executor::transport::TransportFactory;
    let mode = TransportFactory::mode_for_tool("gemini-cli");
    assert_eq!(
        mode,
        csa_executor::transport::TransportMode::Legacy,
        "gemini-cli must route to Legacy transport"
    );
}

#[test]
fn claude_code_also_routes_to_acp_transport() {
    use csa_executor::transport::TransportFactory;
    let mode = TransportFactory::mode_for_tool("claude-code");
    assert_eq!(
        mode,
        csa_executor::transport::TransportMode::Acp,
        "claude-code must route to ACP transport"
    );
}

#[path = "review_cmd_tests_iteration.rs"]
mod review_cmd_tests_iteration;

#[path = "review_cmd_tests_pre_exec.rs"]
mod review_cmd_tests_pre_exec;

#[path = "review_cmd_tests_fix_fallback.rs"]
mod review_cmd_tests_fix_fallback;
