use super::*;
use csa_config::ProjectProfile;

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
        result.len() < 900,
        "Instruction should be concise (preamble + params), got {} chars",
        result.len()
    );
    assert!(!result.contains("git diff"));
    assert!(!result.contains("Pass 1:"));
}

#[test]
fn test_build_review_instruction_contains_anti_recursion_guard() {
    let result = build_review_instruction(
        "uncommitted",
        "review-only",
        "auto",
        ReviewMode::Standard,
        None,
    );
    assert!(result.contains("INSIDE a CSA subprocess"));
    assert!(result.contains("Do NOT invoke"));
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
        None,
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
        None,
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

    let _env_lock = REVIEW_ENV_LOCK.lock().expect("review env lock poisoned");
    let _session_guard = ScopedEnvVarRestore::set("CSA_SESSION_ID", "01K00000000000000000000000");

    let project_dir = tempdir().unwrap();
    let bin_dir = project_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let fake_gemini = bin_dir.join("gemini");
    std::fs::write(&fake_gemini, "#!/bin/sh\nprintf 'review-ok\\n'\n").unwrap();
    let mut perms = std::fs::metadata(&fake_gemini).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&fake_gemini, perms).unwrap();

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let patched_path = format!("{}:{inherited_path}", bin_dir.display());
    let _path_guard = ScopedEnvVarRestore::set("PATH", &patched_path);

    let global = GlobalConfig::default();
    let result = execute_review(
        ToolName::GeminiCli,
        "scope=uncommitted mode=review-only security=auto".to_string(),
        None,
        None,
        "review: stale-session-regression".to_string(),
        project_dir.path(),
        None,
        &global,
        ReviewRoutingMetadata {
            project_profile: ProjectProfile::Unknown,
            detection_method: "auto",
        },
        csa_process::StreamMode::BufferOnly,
        crate::pipeline::DEFAULT_IDLE_TIMEOUT_SECONDS,
        false,
    )
    .await;

    let execution = result.expect("review should ignore inherited stale CSA_SESSION_ID");
    assert_eq!(execution.exit_code, 0);
}
