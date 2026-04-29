use csa_session::{SessionArtifact, create_session, save_result};

struct DebateExactEnvVarGuard {
    key: &'static str,
    original: Option<String>,
}

impl DebateExactEnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let original = std::env::var(key).ok();
        // SAFETY: test-scoped env mutation guarded by TEST_ENV_LOCK.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }
}

impl Drop for DebateExactEnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: test-scoped env mutation guarded by TEST_ENV_LOCK.
        unsafe {
            match self.original.as_deref() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

fn setup_unrelated_debate_session(
    project_root: &std::path::Path,
) -> (String, std::path::PathBuf, Vec<u8>, Vec<u8>, Vec<u8>) {
    let unrelated =
        create_session(project_root, Some("unrelated review"), None, Some("codex")).unwrap();
    let unrelated_session_dir =
        csa_session::get_session_dir(project_root, &unrelated.meta_session_id).unwrap();
    let unrelated_output_dir = unrelated_session_dir.join("output");
    std::fs::create_dir_all(&unrelated_output_dir).unwrap();
    std::fs::write(
        unrelated_output_dir.join("debate-verdict.json"),
        "{\n  \"verdict\": \"APPROVE\"\n}\n",
    )
    .unwrap();
    std::fs::write(
        unrelated_output_dir.join("debate-transcript.md"),
        "original unrelated transcript\n",
    )
    .unwrap();
    save_result(
        project_root,
        &unrelated.meta_session_id,
        &csa_session::SessionResult {
            status: "success".to_string(),
            exit_code: 0,
            summary: "original unrelated summary".to_string(),
            tool: "codex".to_string(),
            original_tool: None,
            fallback_tool: None,
            fallback_reason: None,
            started_at: chrono::Utc::now(),
            completed_at: chrono::Utc::now(),
            events_count: 0,
            artifacts: vec![
                SessionArtifact::new("output/debate-verdict.json"),
                SessionArtifact::new("output/debate-transcript.md"),
            ],
            peak_memory_mb: None,
            manager_fields: Default::default(),
        },
    )
    .unwrap();

    let verdict_before =
        std::fs::read(unrelated_output_dir.join("debate-verdict.json")).expect("read verdict");
    let transcript_before =
        std::fs::read(unrelated_output_dir.join("debate-transcript.md")).expect("read transcript");
    let result_before =
        std::fs::read(unrelated_session_dir.join("result.toml")).expect("read result");

    (
        unrelated.meta_session_id,
        unrelated_output_dir,
        verdict_before,
        transcript_before,
        result_before,
    )
}

#[test]
fn debate_tier_all_fail_does_not_overwrite_unrelated_latest_session() {
    let temp = tempfile::TempDir::new().unwrap();
    let _env_lock = test_env_lock::TEST_ENV_LOCK.blocking_lock();
    let state_home = temp.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = DebateExactEnvVarGuard::set("HOME", temp.path());
    let _state_guard = DebateExactEnvVarGuard::set("XDG_STATE_HOME", &state_home);

    let project_root = temp.path();
    let (unrelated_session_id, unrelated_output_dir, verdict_before, transcript_before, result_before) =
        setup_unrelated_debate_session(project_root);
    let unrelated_session_dir =
        csa_session::get_session_dir(project_root, &unrelated_session_id).unwrap();

    let missing_owned_session_id = "01KPQTESTMISSINGOWNEDSESSION";
    let persistable_session_id =
        debate_cmd::resolve_persisted_debate_session_id(project_root, missing_owned_session_id, true)
            .expect("all-tier-fail missing session should degrade to no persistence");
    assert!(
        persistable_session_id.is_none(),
        "missing owned session must not fall back to unrelated latest session"
    );

    let verdict_after =
        std::fs::read(unrelated_output_dir.join("debate-verdict.json")).expect("read verdict");
    let transcript_after =
        std::fs::read(unrelated_output_dir.join("debate-transcript.md")).expect("read transcript");
    let result_after =
        std::fs::read(unrelated_session_dir.join("result.toml")).expect("read result");
    assert_eq!(verdict_after, verdict_before);
    assert_eq!(transcript_after, transcript_before);
    assert_eq!(result_after, result_before);
}

#[test]
fn debate_pre_session_all_fail_yields_unavailable() {
    let temp = tempfile::TempDir::new().unwrap();
    let _env_lock = test_env_lock::TEST_ENV_LOCK.blocking_lock();
    let state_home = temp.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = DebateExactEnvVarGuard::set("HOME", temp.path());
    let _state_guard = DebateExactEnvVarGuard::set("XDG_STATE_HOME", &state_home);

    let project_root = temp.path();
    let (unrelated_session_id, unrelated_output_dir, verdict_before, transcript_before, result_before) =
        setup_unrelated_debate_session(project_root);

    let failures = vec![
        tier_model_fallback::TierAttemptFailure {
            model_spec: "bad_pre_session_a".to_string(),
            reason: "AUTH_EXPIRED".to_string(),
        },
        tier_model_fallback::TierAttemptFailure {
            model_spec: "bad_pre_session_b".to_string(),
            reason: "QUOTA_EXHAUSTED".to_string(),
        },
        tier_model_fallback::TierAttemptFailure {
            model_spec: "bad_pre_session_c".to_string(),
            reason: "PERMISSION_DENIED".to_string(),
        },
    ];
    let finalized = debate_cmd::finalize_debate_outcome(
        project_root,
        csa_core::types::OutputFormat::Json,
        None,
        debate_cmd::DebateFinalizeContext {
            all_tier_models_failed: true,
            resolved_tier_name: Some("quality"),
            failures: &failures,
            debate_mode: debate_cmd::DebateMode::Heterogeneous,
            output_header: None,
            original_tool: None,
            fallback_tool: None,
            fallback_reason: None,
        },
    )
    .expect("pre-session all-fail should synthesize unavailable");

    assert_eq!(finalized.exit_code, 1);
    let rendered_json: serde_json::Value =
        serde_json::from_str(&finalized.rendered_output).expect("json output");
    assert_eq!(rendered_json["verdict"], "UNAVAILABLE");
    assert_eq!(rendered_json["decision"], "unavailable");
    assert_eq!(rendered_json["meta_session_id"], "unknown");
    let failure_reason = rendered_json["failure_reason"]
        .as_str()
        .expect("failure_reason string");
    assert!(failure_reason.contains("bad_pre_session_a=AUTH_EXPIRED"));
    assert!(failure_reason.contains("bad_pre_session_b=QUOTA_EXHAUSTED"));
    assert!(failure_reason.contains("bad_pre_session_c=PERMISSION_DENIED"));
    assert!(
        !finalized
            .rendered_output
            .contains("all models failed before producing a resumable session")
    );

    let unrelated_session_dir =
        csa_session::get_session_dir(project_root, &unrelated_session_id).unwrap();
    let verdict_after =
        std::fs::read(unrelated_output_dir.join("debate-verdict.json")).expect("read verdict");
    let transcript_after =
        std::fs::read(unrelated_output_dir.join("debate-transcript.md")).expect("read transcript");
    let result_after =
        std::fs::read(unrelated_session_dir.join("result.toml")).expect("read result");
    assert_eq!(verdict_after, verdict_before);
    assert_eq!(transcript_after, transcript_before);
    assert_eq!(result_after, result_before);

    let sessions = csa_session::list_sessions(project_root, None).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].meta_session_id, unrelated_session_id);
}
