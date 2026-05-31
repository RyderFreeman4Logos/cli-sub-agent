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
            fallback_chain: None,
        gate_timeout: false,
            warnings: Vec::new(),
            raw_process_exit_code: None,
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

    assert_eq!(finalized.exit_code, 2);
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

#[test]
fn debate_extractor_uses_final_assistant_message_over_protocol_and_hook_noise() {
    // Spec test #3: a codex-style JSON event transcript whose first non-empty
    // line is `thread.started`, interleaved with a `[other] {...hook...}` event
    // envelope and a `tool_result` item, ending with the final assistant message
    // carrying the structured `CSA_VERDICT: CONFIRMED` success marker. The
    // extractor MUST source summary/verdict from the assistant message, never
    // from the protocol JSON / hook / tool_result lines (#161).
    let transcript = [
        r#"{"type":"thread.started","thread_id":"thread_1"}"#,
        r#"[other] {"type":"hook_started","hook":"SessionStart"}"#,
        r#"{"type":"item.completed","item":{"id":"i1","type":"tool_result","text":"secret shell output that must not leak into the summary"}}"#,
        r#"{"type":"item.completed","item":{"id":"i2","type":"agent_message","text":"Summary: both reviewers agree the fix is sound.\nCSA_VERDICT: CONFIRMED"}}"#,
    ]
    .join("\n");

    let summary = crate::debate_cmd_output::extract_debate_summary(
        &transcript,
        "fallback summary must not be used",
        debate_cmd::DebateMode::Heterogeneous,
    );

    // Summary comes from the assistant message, not protocol/hook/tool_result.
    assert_eq!(summary.summary, "both reviewers agree the fix is sound.");
    assert!(!summary.summary.contains("secret shell output"));
    assert!(!summary.summary.contains("thread.started"));
    assert!(!summary.summary.contains("hook_started"));
    assert!(!summary.summary.contains("fallback summary"));

    // `CSA_VERDICT: CONFIRMED` is recognized as the success verdict.
    assert_eq!(summary.verdict, "CONFIRMED");
    assert_eq!(
        crate::debate_cmd_output::extract_verdict(
            "Summary: both reviewers agree the fix is sound.\nCSA_VERDICT: CONFIRMED"
        ),
        "CONFIRMED"
    );

    // The recognized verdict maps to a success exit code (#161).
    assert_eq!(
        crate::verdict_exit_code::exit_code_from_debate_verdict(summary.verdict.as_str(), None),
        0
    );
}

#[test]
fn debate_extractor_rejects_bare_protocol_envelope_in_prose_output() {
    // Companion to spec test #3: when the output is NOT a codex transcript
    // (first line is plain prose), the raw-prose summary scan must still skip a
    // bare `[other] {...event...}` / `{"type":...}` envelope line and pick the
    // real prose line instead (#161).
    let output = [
        "Both models converged on the same recommendation.",
        r#"[other] {"type":"hook_completed"}"#,
        r#"{"type":"turn.completed","usage":{"input_tokens":10}}"#,
        "Verdict: CONFIRMED",
    ]
    .join("\n");

    let summary = crate::debate_cmd_output::extract_debate_summary(
        &output,
        "unused fallback",
        debate_cmd::DebateMode::Heterogeneous,
    );

    assert_eq!(
        summary.summary,
        "Both models converged on the same recommendation."
    );
    assert!(!summary.summary.contains("hook_completed"));
    assert!(!summary.summary.contains("turn.completed"));
    assert_eq!(summary.verdict, "CONFIRMED");
}

#[test]
fn debate_nonzero_with_explicit_verdict_is_reclassified_success() {
    let temp = tempfile::TempDir::new().unwrap();
    let _env_lock = test_env_lock::TEST_ENV_LOCK.blocking_lock();
    let state_home = temp.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = DebateExactEnvVarGuard::set("HOME", temp.path());
    let _state_guard = DebateExactEnvVarGuard::set("XDG_STATE_HOME", &state_home);

    let project_root = temp.path();
    let session = create_session(project_root, Some("debate"), None, Some("codex")).unwrap();
    save_result(
        project_root,
        &session.meta_session_id,
        &csa_session::SessionResult {
            status: "failure".to_string(),
            exit_code: 1,
            summary: "tool exited non-zero".to_string(),
            tool: "codex".to_string(),
            original_tool: None,
            fallback_tool: None,
            fallback_reason: None,
            started_at: chrono::Utc::now(),
            completed_at: chrono::Utc::now(),
            events_count: 0,
            artifacts: Vec::new(),
            peak_memory_mb: None,
            fallback_chain: None,
        gate_timeout: false,
            warnings: Vec::new(),
            raw_process_exit_code: None,
            manager_fields: Default::default(),
        },
    )
    .unwrap();

    let output = r#"<!-- CSA:SECTION:summary -->
Verdict: APPROVE
<!-- CSA:SECTION:summary:END -->
"#;
    let finalized = debate_cmd::finalize_debate_outcome(
        project_root,
        csa_core::types::OutputFormat::Text,
        Some(pipeline::SessionExecutionResult {
            execution: csa_process::ExecutionResult {
                output: output.to_string(),
                stderr_output: String::new(),
                summary: "debate verdict produced".to_string(),
                exit_code: 1,
                peak_memory_mb: None,
                ..Default::default()
            },
            meta_session_id: session.meta_session_id.clone(),
            provider_session_id: None,
            changed_paths: None,
        }),
        debate_cmd::DebateFinalizeContext {
            all_tier_models_failed: false,
            resolved_tier_name: None,
            failures: &[],
            debate_mode: debate_cmd::DebateMode::Heterogeneous,
            output_header: None,
            original_tool: None,
            fallback_tool: None,
            fallback_reason: None,
        },
    )
    .expect("explicit verdict should finalize");

    assert_eq!(finalized.exit_code, 0);
    let saved = csa_session::load_result(project_root, &session.meta_session_id)
        .unwrap()
        .expect("saved result");
    assert_eq!(saved.status, "success");
    assert_eq!(saved.exit_code, 0);
}

#[test]
fn debate_finalize_persists_categorized_fallback_chain_for_multi_skip() {
    let temp = tempfile::TempDir::new().unwrap();
    let _env_lock = test_env_lock::TEST_ENV_LOCK.blocking_lock();
    let state_home = temp.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = DebateExactEnvVarGuard::set("HOME", temp.path());
    let _state_guard = DebateExactEnvVarGuard::set("XDG_STATE_HOME", &state_home);

    let project_root = temp.path();
    let session = create_session(project_root, Some("debate"), None, Some("claude-code")).unwrap();
    save_result(
        project_root,
        &session.meta_session_id,
        &csa_session::SessionResult {
            status: "failure".to_string(),
            exit_code: 1,
            summary: "tool exited non-zero".to_string(),
            tool: "claude-code".to_string(),
            original_tool: None,
            fallback_tool: None,
            fallback_reason: None,
            started_at: chrono::Utc::now(),
            completed_at: chrono::Utc::now(),
            events_count: 0,
            artifacts: Vec::new(),
            peak_memory_mb: None,
            fallback_chain: None,
            gate_timeout: false,
            warnings: Vec::new(),
            raw_process_exit_code: None,
            manager_fields: Default::default(),
        },
    )
    .unwrap();

    let failures = vec![
        tier_model_fallback::TierAttemptFailure {
            model_spec: "gemini-cli/google/gemini-3.1-pro-preview/xhigh".to_string(),
            reason: "monthly spending cap reached".to_string(),
        },
        tier_model_fallback::TierAttemptFailure {
            model_spec: "codex/openai/gpt-5/high".to_string(),
            reason: "acp server shut down unexpectedly".to_string(),
        },
    ];
    let output = r#"<!-- CSA:SECTION:summary -->
Verdict: APPROVE
<!-- CSA:SECTION:summary:END -->
"#;

    let finalized = debate_cmd::finalize_debate_outcome(
        project_root,
        csa_core::types::OutputFormat::Text,
        Some(pipeline::SessionExecutionResult {
            execution: csa_process::ExecutionResult {
                output: output.to_string(),
                stderr_output: String::new(),
                summary: "debate verdict produced".to_string(),
                exit_code: 0,
                peak_memory_mb: None,
                ..Default::default()
            },
            meta_session_id: session.meta_session_id.clone(),
            provider_session_id: None,
            changed_paths: None,
        }),
        debate_cmd::DebateFinalizeContext {
            all_tier_models_failed: false,
            resolved_tier_name: Some("quality"),
            failures: &failures,
            debate_mode: debate_cmd::DebateMode::Heterogeneous,
            output_header: None,
            original_tool: Some(csa_core::types::ToolName::GeminiCli),
            fallback_tool: Some(csa_core::types::ToolName::ClaudeCode),
            fallback_reason: None,
        },
    )
    .expect("fallback chain should finalize");

    assert_eq!(finalized.exit_code, 0);
    let saved = csa_session::load_result(project_root, &session.meta_session_id)
        .unwrap()
        .expect("saved result");
    assert_eq!(saved.original_tool.as_deref(), Some("gemini-cli"));
    assert_eq!(saved.fallback_tool.as_deref(), Some("claude-code"));
    assert!(saved.fallback_reason.is_none());
    let persisted_chain = saved.fallback_chain.expect("fallback_chain should persist");
    assert_eq!(persisted_chain.len(), 2);
    assert_eq!(persisted_chain[0].tool, "gemini-cli");
    assert_eq!(persisted_chain[0].skip_reason, "oauth-quota");
    assert!(persisted_chain[0].quota_exhausted);
    assert_eq!(persisted_chain[1].tool, "codex");
    assert_eq!(persisted_chain[1].skip_reason, "transport-error");
    assert!(!persisted_chain[1].quota_exhausted);
}

#[test]
fn debate_nonzero_with_revise_artifact_exits_failure() {
    let temp = tempfile::TempDir::new().unwrap();
    let _env_lock = test_env_lock::TEST_ENV_LOCK.blocking_lock();
    let state_home = temp.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = DebateExactEnvVarGuard::set("HOME", temp.path());
    let _state_guard = DebateExactEnvVarGuard::set("XDG_STATE_HOME", &state_home);

    let project_root = temp.path();
    let session = create_session(project_root, Some("debate"), None, Some("codex")).unwrap();
    save_result(
        project_root,
        &session.meta_session_id,
        &csa_session::SessionResult {
            status: "failure".to_string(),
            exit_code: 1,
            summary: "tool exited non-zero".to_string(),
            tool: "codex".to_string(),
            original_tool: None,
            fallback_tool: None,
            fallback_reason: None,
            started_at: chrono::Utc::now(),
            completed_at: chrono::Utc::now(),
            events_count: 0,
            artifacts: Vec::new(),
            peak_memory_mb: None,
            fallback_chain: None,
        gate_timeout: false,
            warnings: Vec::new(),
            raw_process_exit_code: None,
            manager_fields: Default::default(),
        },
    )
    .unwrap();

    let output = r#"<!-- CSA:SECTION:summary -->
Verdict: REVISE
<!-- CSA:SECTION:summary:END -->
"#;
    let finalized = debate_cmd::finalize_debate_outcome(
        project_root,
        csa_core::types::OutputFormat::Text,
        Some(pipeline::SessionExecutionResult {
            execution: csa_process::ExecutionResult {
                output: output.to_string(),
                stderr_output: String::new(),
                summary: "debate verdict produced".to_string(),
                exit_code: 1,
                peak_memory_mb: None,
                ..Default::default()
            },
            meta_session_id: session.meta_session_id.clone(),
            provider_session_id: None,
            changed_paths: None,
        }),
        debate_cmd::DebateFinalizeContext {
            all_tier_models_failed: false,
            resolved_tier_name: None,
            failures: &[],
            debate_mode: debate_cmd::DebateMode::Heterogeneous,
            output_header: None,
            original_tool: None,
            fallback_tool: None,
            fallback_reason: None,
        },
    )
    .expect("revise verdict should finalize");

    assert_eq!(finalized.exit_code, 1);
    let saved = csa_session::load_result(project_root, &session.meta_session_id)
        .unwrap()
        .expect("saved result");
    assert_eq!(saved.status, "failure");
    assert_eq!(saved.exit_code, 1);
}
