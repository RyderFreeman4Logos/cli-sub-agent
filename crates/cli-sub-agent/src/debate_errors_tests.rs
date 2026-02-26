use super::*;
use chrono::Utc;
use csa_process::ExecutionResult;
use csa_session::{
    ContextStatus, Genealogy, MetaSessionState, SandboxInfo, SessionPhase, TaskContext,
};
use std::collections::HashMap;

fn sample_session_state() -> MetaSessionState {
    MetaSessionState {
        meta_session_id: "01HTEST000000000000000000".to_string(),
        description: Some("debate".to_string()),
        project_path: "/tmp".to_string(),
        branch: None,
        created_at: Utc::now(),
        last_accessed: Utc::now(),
        genealogy: Genealogy::default(),
        tools: HashMap::new(),
        context_status: ContextStatus::default(),
        total_token_usage: None,
        phase: SessionPhase::Active,
        task_context: TaskContext::default(),
        turn_count: 0,
        token_budget: None,
        sandbox_info: None,
        termination_reason: None,
        is_seed_candidate: false,
        git_head_at_creation: None,
        last_return_packet: None,
        fork_call_timestamps: Vec::new(),
    }
}

#[test]
fn classify_exit_137_with_sandbox_memory_as_transient() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut state = sample_session_state();
    state.sandbox_info = Some(SandboxInfo {
        mode: "rlimit".to_string(),
        memory_max_mb: Some(1024),
    });
    let execution = ExecutionResult {
        output: String::new(),
        stderr_output: "killed".to_string(),
        summary: "killed".to_string(),
        exit_code: 137,
    };

    let classified = classify_execution_outcome(&execution, Some(&state), tmp.path());
    assert!(matches!(classified, DebateErrorKind::Transient(_)));
}

#[test]
fn classify_exit_1_as_deterministic_argument_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let execution = ExecutionResult {
        output: String::new(),
        stderr_output: "invalid argument".to_string(),
        summary: "invalid argument".to_string(),
        exit_code: 1,
    };

    let classified = classify_execution_outcome(&execution, None, tmp.path());
    assert!(matches!(classified, DebateErrorKind::Deterministic(_)));
}

#[test]
fn classify_timeout_error_with_alive_pid_as_still_working() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let locks_dir = tmp.path().join("locks");
    std::fs::create_dir_all(&locks_dir).expect("create locks");
    let lock_path = locks_dir.join("codex.lock");
    std::fs::write(&lock_path, format!("{{\"pid\": {}}}", std::process::id())).expect("write");

    let classified =
        classify_execution_error(&anyhow::anyhow!("wall-clock timeout"), Some(tmp.path()));
    assert_eq!(classified, DebateErrorKind::StillWorking);
}

#[test]
fn classify_exit_144_sigstkflt_as_transient() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let execution = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: String::new(),
        exit_code: 144,
    };

    let classified = classify_execution_outcome(&execution, None, tmp.path());
    assert!(
        matches!(classified, DebateErrorKind::Transient(_)),
        "exit 144 (SIGSTKFLT) should be transient, got: {classified:?}"
    );
}

#[test]
fn classify_exit_128_as_deterministic() {
    // Exit 128 = signal 0, which is not a real signal.
    let tmp = tempfile::tempdir().expect("tempdir");
    let execution = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: String::new(),
        exit_code: 128,
    };

    let classified = classify_execution_outcome(&execution, None, tmp.path());
    assert!(
        matches!(classified, DebateErrorKind::Deterministic(_)),
        "exit 128 (signal 0) should be deterministic, got: {classified:?}"
    );
}

#[test]
fn classify_exit_129_sighup_as_transient() {
    // Exit 129 = signal 1 (SIGHUP), a valid signal.
    let tmp = tempfile::tempdir().expect("tempdir");
    let execution = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: String::new(),
        exit_code: 129,
    };

    let classified = classify_execution_outcome(&execution, None, tmp.path());
    assert!(
        matches!(classified, DebateErrorKind::Transient(_)),
        "exit 129 (SIGHUP) should be transient, got: {classified:?}"
    );
}

#[test]
fn classify_exit_192_sigrtmax_as_transient() {
    // Exit 192 = signal 64 (SIGRTMAX), the highest valid real-time signal.
    let tmp = tempfile::tempdir().expect("tempdir");
    let execution = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: String::new(),
        exit_code: 192,
    };

    let classified = classify_execution_outcome(&execution, None, tmp.path());
    assert!(
        matches!(classified, DebateErrorKind::Transient(_)),
        "exit 192 (SIGRTMAX) should be transient, got: {classified:?}"
    );
}

#[test]
fn classify_exit_193_as_deterministic() {
    // Exit 193 = signal 65, beyond valid Unix signal range.
    let tmp = tempfile::tempdir().expect("tempdir");
    let execution = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: String::new(),
        exit_code: 193,
    };

    let classified = classify_execution_outcome(&execution, None, tmp.path());
    assert!(
        matches!(classified, DebateErrorKind::Deterministic(_)),
        "exit 193 (signal 65) should be deterministic, got: {classified:?}"
    );
}

#[test]
fn classify_exit_255_as_deterministic() {
    // Exit 255 = signal 127, not a real signal.
    let tmp = tempfile::tempdir().expect("tempdir");
    let execution = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: String::new(),
        exit_code: 255,
    };

    let classified = classify_execution_outcome(&execution, None, tmp.path());
    assert!(
        matches!(classified, DebateErrorKind::Deterministic(_)),
        "exit 255 (signal 127) should be deterministic, got: {classified:?}"
    );
}

#[test]
fn classify_exit_2_still_deterministic() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let execution = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: String::new(),
        exit_code: 2,
    };

    let classified = classify_execution_outcome(&execution, None, tmp.path());
    assert!(
        matches!(classified, DebateErrorKind::Deterministic(_)),
        "exit 2 should remain deterministic, got: {classified:?}"
    );
}
