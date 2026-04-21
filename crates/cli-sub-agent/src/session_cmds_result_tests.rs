use super::{
    StructuredOutputOpts, build_result_json_payload, compute_token_measurement,
    display_all_sections, display_single_section, display_summary_section, format_number,
    handle_session_artifacts, handle_session_result, render_result_sidecar_for_text,
};
use crate::test_env_lock::TEST_ENV_LOCK;
use csa_session::state::ReviewSessionMeta;
use csa_session::{SessionResult, SessionResultView, create_session, get_session_dir, load_result};
use tempfile::tempdir;

struct EnvVarGuard {
    key: &'static str,
    original: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let original = std::env::var(key).ok();
        // SAFETY: test-scoped env mutation guarded by a process-wide mutex.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: test-scoped env mutation guarded by a process-wide mutex.
        unsafe {
            match self.original.as_deref() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

// ── display_structured_output tests ───────────────────────────────

#[test]
fn display_summary_section_with_structured_output() {
    let tmp = tempdir().unwrap();
    let output =
        "<!-- CSA:SECTION:summary -->\nThis is the summary.\n<!-- CSA:SECTION:summary:END -->";
    csa_session::persist_structured_output(tmp.path(), output).unwrap();

    // Should succeed without error
    display_summary_section(tmp.path(), "test", false).unwrap();
}

#[test]
fn display_summary_section_falls_back_to_output_log() {
    let tmp = tempdir().unwrap();
    let session_dir = tmp.path();
    // Write output.log without structured markers
    std::fs::write(session_dir.join("output.log"), "Line 1\nLine 2\nLine 3\n").unwrap();

    // Should succeed (falls back to output.log)
    display_summary_section(session_dir, "test", false).unwrap();
}

#[test]
fn display_summary_section_handles_no_output() {
    let tmp = tempdir().unwrap();
    // No output.log, no index.toml — should print message to stderr
    display_summary_section(tmp.path(), "test", false).unwrap();
}

#[test]
fn display_single_section_returns_content() {
    let tmp = tempdir().unwrap();
    let output = "<!-- CSA:SECTION:details -->\nDetail content\n<!-- CSA:SECTION:details:END -->";
    csa_session::persist_structured_output(tmp.path(), output).unwrap();

    display_single_section(tmp.path(), "test", "details", false).unwrap();
}

#[test]
fn display_single_section_errors_on_missing_id() {
    let tmp = tempdir().unwrap();
    let output = "<!-- CSA:SECTION:summary -->\nContent\n<!-- CSA:SECTION:summary:END -->";
    csa_session::persist_structured_output(tmp.path(), output).unwrap();

    let err = display_single_section(tmp.path(), "test", "nonexistent", false).unwrap_err();
    assert!(err.to_string().contains("not found"));
    assert!(err.to_string().contains("summary")); // lists available sections
}

#[test]
fn display_single_section_errors_when_no_structured_output() {
    let tmp = tempdir().unwrap();
    let err = display_single_section(tmp.path(), "test", "any", false).unwrap_err();
    assert!(err.to_string().contains("No structured output"));
}

#[test]
fn display_all_sections_shows_all_in_order() {
    let tmp = tempdir().unwrap();
    let output = "<!-- CSA:SECTION:intro -->\nIntro\n<!-- CSA:SECTION:intro:END -->\n\
                   <!-- CSA:SECTION:body -->\nBody\n<!-- CSA:SECTION:body:END -->";
    csa_session::persist_structured_output(tmp.path(), output).unwrap();

    display_all_sections(tmp.path(), "test", false).unwrap();
}

#[test]
fn display_all_sections_falls_back_to_output_log() {
    let tmp = tempdir().unwrap();
    let session_dir = tmp.path();
    std::fs::write(session_dir.join("output.log"), "raw output here\n").unwrap();

    display_all_sections(session_dir, "test", false).unwrap();
}

// ── format_number tests ───────────────────────────────────────────

#[test]
fn format_number_small_values() {
    assert_eq!(format_number(0), "0");
    assert_eq!(format_number(42), "42");
    assert_eq!(format_number(999), "999");
}

#[test]
fn format_number_with_commas() {
    assert_eq!(format_number(1000), "1,000");
    assert_eq!(format_number(3456), "3,456");
    assert_eq!(format_number(1234567), "1,234,567");
}

// ── compute_token_measurement tests ───────────────────────────────

#[test]
fn measure_structured_output_with_summary() {
    let tmp = tempdir().unwrap();
    let output = "<!-- CSA:SECTION:summary -->\n\
                   Summary line one.\n\
                   Summary line two.\n\
                   <!-- CSA:SECTION:summary:END -->\n\
                   <!-- CSA:SECTION:analysis -->\n\
                   Analysis paragraph one with many words to increase token count.\n\
                   Analysis paragraph two with additional detail and explanation.\n\
                   <!-- CSA:SECTION:analysis:END -->\n\
                   <!-- CSA:SECTION:details -->\n\
                   Detailed implementation notes with code examples and references.\n\
                   More detail lines for testing purposes.\n\
                   <!-- CSA:SECTION:details:END -->\n\
                   <!-- CSA:SECTION:implementation -->\n\
                   Implementation code and final notes.\n\
                   <!-- CSA:SECTION:implementation:END -->";
    csa_session::persist_structured_output(tmp.path(), output).unwrap();

    let m = compute_token_measurement(tmp.path(), "01TEST123").unwrap();
    assert!(m.is_structured);
    assert_eq!(m.section_count, 4);
    assert_eq!(
        m.section_names,
        vec!["summary", "analysis", "details", "implementation"]
    );
    assert!(m.summary_tokens > 0);
    assert!(m.total_tokens > m.summary_tokens);
    assert!(m.savings_percent > 0.0);
    assert_eq!(m.savings_tokens, m.total_tokens - m.summary_tokens);
}

#[test]
fn measure_unstructured_output_no_savings() {
    let tmp = tempdir().unwrap();
    let output = "Plain text without any markers.\nSecond line.\nThird line.";
    csa_session::persist_structured_output(tmp.path(), output).unwrap();

    let m = compute_token_measurement(tmp.path(), "01TEST456").unwrap();
    assert!(!m.is_structured);
    assert_eq!(m.section_count, 1);
    assert_eq!(m.section_names, vec!["full"]);
    // For unstructured, summary_tokens = first section = total
    assert_eq!(m.summary_tokens, m.total_tokens);
    assert_eq!(m.savings_tokens, 0);
    assert_eq!(m.savings_percent, 0.0);
}

#[test]
fn measure_empty_output() {
    let tmp = tempdir().unwrap();
    csa_session::persist_structured_output(tmp.path(), "").unwrap();

    let m = compute_token_measurement(tmp.path(), "01EMPTY").unwrap();
    assert!(!m.is_structured);
    assert_eq!(m.total_tokens, 0);
    assert_eq!(m.summary_tokens, 0);
    assert_eq!(m.savings_tokens, 0);
    assert_eq!(m.savings_percent, 0.0);
}

#[test]
fn measure_no_index_falls_back_to_output_log() {
    let tmp = tempdir().unwrap();
    let session_dir = tmp.path();
    std::fs::write(
        session_dir.join("output.log"),
        "Some raw output content here.\n",
    )
    .unwrap();

    let m = compute_token_measurement(session_dir, "01NOINDEX").unwrap();
    assert!(!m.is_structured);
    assert!(m.total_tokens > 0);
    assert_eq!(m.summary_tokens, m.total_tokens);
    assert_eq!(m.savings_tokens, 0);
    assert!(m.section_names.is_empty());
}

#[test]
fn measure_no_output_at_all() {
    let tmp = tempdir().unwrap();
    let m = compute_token_measurement(tmp.path(), "01NOTHING").unwrap();
    assert!(!m.is_structured);
    assert_eq!(m.total_tokens, 0);
    assert_eq!(m.savings_tokens, 0);
}

#[test]
fn measure_single_named_section_is_structured() {
    let tmp = tempdir().unwrap();
    let output =
        "<!-- CSA:SECTION:report -->\nReport content here.\n<!-- CSA:SECTION:report:END -->";
    csa_session::persist_structured_output(tmp.path(), output).unwrap();

    let m = compute_token_measurement(tmp.path(), "01SINGLE").unwrap();
    // Single section that is NOT "full" counts as structured
    assert!(m.is_structured);
    assert_eq!(m.section_count, 1);
    assert_eq!(m.section_names, vec!["report"]);
}

#[cfg(unix)]
fn set_file_mtime_seconds_ago(path: &std::path::Path, seconds_ago: u64) {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before unix epoch");
    let target = now.saturating_sub(std::time::Duration::from_secs(seconds_ago));
    let tv_sec = target.as_secs() as libc::time_t;
    let tv_nsec = target.subsec_nanos() as libc::c_long;
    let times = [
        libc::timespec { tv_sec, tv_nsec },
        libc::timespec { tv_sec, tv_nsec },
    ];
    let c_path = CString::new(path.as_os_str().as_bytes()).expect("path contains NUL");
    // SAFETY: `utimensat` receives a valid C path pointer and valid timespec array.
    let rc = unsafe { libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), 0) };
    assert_eq!(rc, 0, "utimensat failed for {}", path.display());
}

#[cfg(unix)]
fn backdate_tree(path: &std::path::Path, seconds_ago: u64) {
    if path.is_dir() {
        for entry in std::fs::read_dir(path).expect("read_dir") {
            let entry = entry.expect("dir entry");
            backdate_tree(&entry.path(), seconds_ago);
        }
    }
    set_file_mtime_seconds_ago(path, seconds_ago);
}

#[cfg(unix)]
#[test]
fn handle_session_result_reconciles_orphaned_active_session() {
    let tmp = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = tmp.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", tmp.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = tmp.path();

    let session = create_session(project, Some("result-reconcile"), None, None).unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();
    backdate_tree(&session_dir, 120);

    handle_session_result(
        session_id.clone(),
        false,
        Some(project.to_string_lossy().into_owned()),
        StructuredOutputOpts::default(),
    )
    .unwrap();

    assert!(
        load_result(project, &session_id).unwrap().is_some(),
        "session result command should reconcile missing terminal result"
    );
}

#[cfg(unix)]
#[test]
fn handle_session_artifacts_reconciles_orphaned_active_session() {
    let tmp = tempdir().unwrap();
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let state_home = tmp.path().join("xdg-state");
    std::fs::create_dir_all(&state_home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", tmp.path());
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let project = tmp.path();

    let session = create_session(project, Some("artifacts-reconcile"), None, None).unwrap();
    let session_id = session.meta_session_id;
    let session_dir = get_session_dir(project, &session_id).unwrap();
    backdate_tree(&session_dir, 120);

    handle_session_artifacts(
        session_id.clone(),
        Some(project.to_string_lossy().into_owned()),
    )
    .unwrap();

    assert!(
        load_result(project, &session_id).unwrap().is_some(),
        "session artifacts command should reconcile missing terminal result"
    );
}

#[test]
fn build_result_json_payload_includes_review_iterations() {
    let now = chrono::Utc::now();
    let result = SessionResult {
        status: "success".to_string(),
        exit_code: 0,
        summary: "review completed".to_string(),
        tool: "codex".to_string(),
        started_at: now,
        completed_at: now,
        events_count: 0,
        artifacts: Vec::new(),
        peak_memory_mb: None,
        manager_fields: Default::default(),
    };
    let review_meta = ReviewSessionMeta {
        session_id: "01JTESTPAYLOAD00000000000001".to_string(),
        head_sha: "deadbeef".to_string(),
        decision: "pass".to_string(),
        verdict: "CLEAN".to_string(),
        status_reason: None,
        routed_to: None,
        primary_failure: None,
        failure_reason: None,
        tool: "codex".to_string(),
        scope: "range:main...HEAD".to_string(),
        exit_code: 0,
        fix_attempted: true,
        fix_rounds: 1,
        review_iterations: 4,
        timestamp: now,
        diff_fingerprint: Some("sha256:abc123".to_string()),
    };

    let payload = build_result_json_payload(
        &SessionResultView {
            envelope: result,
            manager_sidecar: None,
            legacy_sidecar: None,
        },
        None,
        Some(&review_meta),
    )
    .unwrap();
    assert_eq!(payload["review_meta"]["review_iterations"], 4);
    assert_eq!(payload["review_meta"]["fix_rounds"], 1);
}

#[test]
fn build_result_json_payload_includes_result_sidecars() {
    let now = chrono::Utc::now();
    let result = SessionResultView {
        envelope: SessionResult {
            status: "success".to_string(),
            exit_code: 0,
            summary: "review completed".to_string(),
            tool: "codex".to_string(),
            started_at: now,
            completed_at: now,
            events_count: 0,
            artifacts: Vec::new(),
            peak_memory_mb: None,
            manager_fields: Default::default(),
        },
        manager_sidecar: Some(
            toml::toml! {
                [report]
                summary = "manager-visible"
            }
            .into(),
        ),
        legacy_sidecar: Some(
            toml::toml! {
                [artifacts]
                count = 2
            }
            .into(),
        ),
    };

    let payload = build_result_json_payload(&result, None, None).unwrap();
    assert_eq!(
        payload["manager_sidecar"]["report"]["summary"],
        "manager-visible"
    );
    assert_eq!(payload["legacy_sidecar"]["artifacts"]["count"], 2);
}

#[test]
fn build_result_json_payload_redacts_result_sidecars() {
    let now = chrono::Utc::now();
    let result = SessionResultView {
        envelope: SessionResult {
            status: "success".to_string(),
            exit_code: 0,
            summary: "review completed".to_string(),
            tool: "codex".to_string(),
            started_at: now,
            completed_at: now,
            events_count: 0,
            artifacts: Vec::new(),
            peak_memory_mb: None,
            manager_fields: Default::default(),
        },
        manager_sidecar: Some(
            toml::toml! {
                [auth]
                api_key = "hunter2"
            }
            .into(),
        ),
        legacy_sidecar: None,
    };

    let payload = build_result_json_payload(&result, None, None).unwrap();
    let rendered = serde_json::to_string(&payload).unwrap();
    assert!(!rendered.contains("hunter2"));
    assert!(rendered.contains("[REDACTED]"));
}

#[test]
fn sidecar_from_manager_fields_rehydrates_manager_sections() {
    let sidecar = csa_session::SessionManagerFields {
        result: Some(
            toml::toml! {
                done = true
            }
            .into(),
        ),
        report: Some(
            toml::toml! {
                summary = "manager-visible"
            }
            .into(),
        ),
        timing: Some(
            toml::toml! {
                started_at = "2026-02-11T10:00:00Z"
            }
            .into(),
        ),
        tool: Some(
            toml::toml! {
                name = "claude-code"
            }
            .into(),
        ),
        review: Some(
            toml::toml! {
                reviewer_tool = "codex"
            }
            .into(),
        ),
        clarification: Some(
            toml::toml! {
                blocking_reason = "Need answer"
            }
            .into(),
        ),
        artifacts: Some(
            toml::toml! {
                count = 2
            }
            .into(),
        ),
    }
    .as_sidecar()
    .expect("sidecar should be rehydrated");

    assert_eq!(sidecar["result"]["done"], toml::Value::Boolean(true));
    assert_eq!(
        sidecar["report"]["summary"],
        toml::Value::String("manager-visible".to_string())
    );
    assert_eq!(
        sidecar["timing"]["started_at"],
        toml::Value::String("2026-02-11T10:00:00Z".to_string())
    );
    assert_eq!(
        sidecar["tool"]["name"],
        toml::Value::String("claude-code".to_string())
    );
    assert_eq!(
        sidecar["review"]["reviewer_tool"],
        toml::Value::String("codex".to_string())
    );
    assert_eq!(
        sidecar["clarification"]["blocking_reason"],
        toml::Value::String("Need answer".to_string())
    );
    assert_eq!(sidecar["artifacts"]["count"], toml::Value::Integer(2));
}

#[test]
fn sidecar_from_manager_fields_skips_empty_manager_sections() {
    assert!(
        csa_session::SessionManagerFields::default()
            .as_sidecar()
            .is_none()
    );
}

#[test]
fn render_result_sidecar_for_text_skips_empty_tables() {
    assert!(render_result_sidecar_for_text(&toml::Value::Table(toml::map::Map::new())).is_none());
}
