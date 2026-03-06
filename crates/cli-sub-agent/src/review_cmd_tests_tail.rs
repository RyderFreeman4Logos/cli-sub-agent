use super::*;

// --- verify_review_skill_available tests ---

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
