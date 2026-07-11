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
fn verify_review_skill_missing_repo_local_pattern_uses_bundled_fallback() {
    let tmp = tempfile::TempDir::new().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    assert!(verify_review_skill_available(tmp.path(), false).is_ok());
    assert!(!tmp.path().join(".csa").exists());
    assert!(!tmp.path().join("patterns").exists());
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
fn build_review_instruction_for_project_injects_bundled_pattern_without_repo_local_pattern() {
    let project_dir = tempfile::TempDir::new().unwrap();
    let state_dir = tempfile::TempDir::new().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&state_dir);
    let pattern = verify_review_skill_available(project_dir.path(), false)
        .unwrap()
        .expect("bundled pattern should resolve");

    let (instruction, _routing) = build_review_instruction_for_project(
        "uncommitted",
        "review-only",
        "auto",
        ReviewMode::Standard,
        None,
        project_dir.path(),
        resolve::ReviewProjectPromptOptions {
            project_config: None,
            resolved_pattern: Some(&pattern),
            prior_rounds_section: None,
            current_session_id: None,
            full_consistency: false,
            review_depth: crate::cli::ReviewDepth::Standard,
            review_depth_auto_escalation: None,
            regression_context: None,
        },
    );

    assert!(instruction.contains("Use the csa-review skill. scope=uncommitted"));
    assert!(instruction.contains("<skill-mode>executor</skill-mode>"));
    assert!(instruction.contains("<skill-source path=\""));
    assert!(instruction.contains("CSA Review: Independent Code Review Orchestration"));
    assert!(!project_dir.path().join(".csa").exists());
    assert!(!project_dir.path().join("patterns").exists());
}

#[test]
fn build_review_instruction_for_project_injects_repo_local_pattern() {
    let project_dir = tempfile::TempDir::new().unwrap();
    let skill_dir = project_dir
        .path()
        .join(".csa")
        .join("patterns")
        .join("csa-review")
        .join("skills")
        .join("csa-review");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "# Repo Local Review Protocol\nUse this local reviewer.",
    )
    .unwrap();
    let pattern = verify_review_skill_available(project_dir.path(), false)
        .unwrap()
        .expect("repo-local pattern should resolve");

    let (instruction, _routing) = build_review_instruction_for_project(
        "uncommitted",
        "review-only",
        "auto",
        ReviewMode::Standard,
        None,
        project_dir.path(),
        resolve::ReviewProjectPromptOptions {
            project_config: None,
            resolved_pattern: Some(&pattern),
            prior_rounds_section: None,
            current_session_id: None,
            full_consistency: false,
            review_depth: crate::cli::ReviewDepth::Standard,
            review_depth_auto_escalation: None,
            regression_context: None,
        },
    );

    assert!(instruction.contains("Use the csa-review skill. scope=uncommitted"));
    assert!(instruction.contains("Repo Local Review Protocol"));
    assert!(instruction.contains(&skill_dir.display().to_string()));
    assert!(!instruction.contains("CSA Review: Independent Code Review Orchestration"));
}

#[test]
fn verify_review_skill_no_repo_local_pattern_uses_bundled_without_allow_fallback() {
    let tmp = tempfile::TempDir::new().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
    let result = verify_review_skill_available(tmp.path(), false);
    assert!(
        result.is_ok(),
        "missing repo-local review pattern should resolve via bundled fallback"
    );
}

#[test]
fn verify_review_skill_allow_fallback_without_skill() {
    let tmp = tempfile::TempDir::new().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&tmp);
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
    let project_config = project_config_with_enabled_tools(&["opencode"]);
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
        Some(&project_config),
        &global,
        None,
        ReviewRoutingMetadata {
            project_profile: ProjectProfile::Unknown,
            detection_method: "auto",
        },
        csa_process::StreamMode::BufferOnly,
        crate::pipeline::DEFAULT_IDLE_TIMEOUT_SECONDS,
        None,        // initial_response_timeout_seconds
        false,       // force_override_user_config
        false,       // force_ignore_tier_setting
        false,       // no_failover
        false,       // no_fs_sandbox
        false,       // readonly_project_root
        &[],         // extra_writable
        &[],         // extra_readable,
        Some(false), // error_marker_scan_override: force scan OFF for marker-bearing fixtures (#1745)
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

#[cfg(unix)]
#[tokio::test]
async fn handle_review_fix_clean_initial_persists_no_fix_attempt() {
    use std::os::unix::fs::PermissionsExt;

    let project_dir = setup_git_repo();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    let bin_dir = project_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let codex_stub = "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf 'codex-cli 1.0.0\\n'\n  exit 0\nfi\nprintf '%s\\n' '<!-- CSA:SECTION:summary -->' 'PASS' '<!-- CSA:SECTION:summary:END -->' '<!-- CSA:SECTION:details -->' 'No blocking findings remain.' '<!-- CSA:SECTION:details:END -->'\n";
    for binary in ["codex", "codex-acp"] {
        let path = bin_dir.join(binary);
        std::fs::write(&path, codex_stub).unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
    }

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let patched_path = format!("{}:{inherited_path}", bin_dir.display());
    let _path_guard = ScopedEnvVarRestore::set("PATH", &patched_path);

    let mut config = project_config_with_enabled_tools(&["codex"]);
    configure_codex_cli_review_test_tool(&mut config);
    write_review_project_config(project_dir.path(), &config);
    install_pattern(project_dir.path(), "csa-review");

    let cd = project_dir.path().display().to_string();
    let args = parse_review_args(&[
        "csa",
        "review",
        "--cd",
        &cd,
        "--files",
        "tracked.txt",
        "--tool",
        "codex",
        "--single",
        "--fix",
        "--no-fs-sandbox",
    ]);

    let exit_code = handle_review(args, 0, &crate::startup_env::EMPTY_STARTUP_SUBTREE_ENV)
        .await
        .expect("clean initial --fix review should succeed");
    assert_eq!(exit_code, 0);

    let sessions = csa_session::list_sessions(project_dir.path(), None).unwrap();
    assert_eq!(sessions.len(), 1, "expected one review session");
    let session_id = &sessions[0].meta_session_id;
    let session_dir = csa_session::get_session_dir(project_dir.path(), session_id).unwrap();
    let meta: ReviewSessionMeta = serde_json::from_str(
        &std::fs::read_to_string(session_dir.join("review_meta.json")).unwrap(),
    )
    .unwrap();
    let artifact: csa_session::ReviewVerdictArtifact = serde_json::from_str(
        &std::fs::read_to_string(session_dir.join("output").join("review-verdict.json")).unwrap(),
    )
    .unwrap();

    assert_eq!(meta.decision, ReviewDecision::Pass.as_str());
    assert_eq!(meta.exit_code, 0);
    assert!(
        !meta.fix_attempted,
        "clean initial --fix must not require fix convergence metadata"
    );
    assert_eq!(meta.fix_rounds, 0);
    assert!(meta.fix_convergence.is_none());
    assert!(meta.accepts_clean_review_verdict(artifact.decision));

    let identity = csa_session::create_vcs_backend(project_dir.path())
        .identity(project_dir.path())
        .expect("resolve git identity");
    let branch = identity.ref_name.expect("branch name");
    let marker_path = crate::review_gate::marker_path(project_dir.path(), &branch, &meta.head_sha);
    assert!(
        marker_path.exists(),
        "clean initial --fix must write the review gate marker"
    );
}

#[tokio::test]
async fn handle_review_rejects_direct_tool_tier_before_session_creation() {
    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    let mut config = project_config_with_enabled_tools(&["opencode", "codex"]);
    config.tiers.insert(
        "default".to_string(),
        csa_config::config::TierConfig {
            description: "Test tier".to_string(),
            models: vec!["opencode/openai/gpt-5/xhigh".to_string()],
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

    let err = handle_review(args, 0, &crate::startup_env::EMPTY_STARTUP_SUBTREE_ENV)
        .await
        .expect_err("direct --tool tier rejection must fail");
    assert!(
        err.chain().any(|cause| cause
            .to_string()
            .contains("restricted when tiers are configured")),
        "unexpected error chain: {err:#}"
    );

    let sessions = csa_session::list_sessions(project_dir.path(), None).unwrap();
    assert!(
        sessions.is_empty(),
        "direct --tool tier rejection must happen before review session creation"
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
