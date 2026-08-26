use super::*;
use std::sync::{LazyLock, Mutex};

static SANDBOX_ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

struct ScopedEnvVar {
    key: &'static str,
    original: Option<String>,
}

impl ScopedEnvVar {
    fn set(key: &'static str, value: &str) -> Self {
        let original = std::env::var(key).ok();
        // SAFETY: test-scoped env mutation guarded by SANDBOX_ENV_LOCK.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }

    fn unset(key: &'static str) -> Self {
        let original = std::env::var(key).ok();
        // SAFETY: test-scoped env mutation guarded by SANDBOX_ENV_LOCK.
        unsafe { std::env::remove_var(key) };
        Self { key, original }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        // SAFETY: test-scoped env mutation guarded by SANDBOX_ENV_LOCK.
        unsafe {
            match self.original.take() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

fn sample_session() -> MetaSessionState {
    let now = chrono::Utc::now();
    MetaSessionState {
        meta_session_id: "01HTEST000000000000000000".to_string(),
        description: Some("test".to_string()),
        project_path: "/tmp/test".to_string(),
        branch: None,
        created_at: now,
        last_accessed: now,
        csa_version: None,
        genealogy: csa_session::state::Genealogy {
            parent_session_id: None,
            depth: 0,
            ..Default::default()
        },
        tools: HashMap::new(),
        context_status: csa_session::state::ContextStatus::default(),
        total_token_usage: None,
        phase: csa_session::state::SessionPhase::Active,
        task_context: csa_session::state::TaskContext::default(),
        turn_count: 0,
        token_budget: None,
        sandbox_info: None,
        termination_reason: None,
        is_seed_candidate: false,
        git_head_at_creation: None,
        pre_session_porcelain: None,
        last_return_packet: None,
        change_id: None,
        spec_id: None,
        fork_call_timestamps: Vec::new(),
        vcs_identity: None,
        identity_version: 1,
    }
}

fn sample_child_session() -> MetaSessionState {
    let mut session = sample_session();
    session.genealogy.parent_session_id = Some("01HPARENT000000000000000000".to_string());
    session
}

#[test]
fn build_env_ignores_spoofed_sandbox_marker_from_extra_env() {
    let _env_lock = SANDBOX_ENV_LOCK.lock().expect("sandbox env lock poisoned");
    let _sandbox_guard = ScopedEnvVar::unset(CSA_FS_SANDBOXED_ENV);
    let transport = AcpTransport::new("claude-code", None);
    let session = sample_session();
    let extra = HashMap::from([(CSA_FS_SANDBOXED_ENV.to_string(), "1".to_string())]);

    let env = transport.build_env(&session, Some(&extra), None, false, false);

    assert!(
        !env.contains_key(CSA_FS_SANDBOXED_ENV),
        "user extra_env must not be able to spoof CSA_FS_SANDBOXED"
    );
}

#[test]
fn build_env_strips_spoofed_git_push_authorization_from_extra_env() {
    let transport = AcpTransport::new("claude-code", None);
    let session = sample_session();
    let extra = HashMap::from([
        (
            csa_core::env::CSA_GIT_PUSH_ALLOWED_ENV_KEY.to_string(),
            "true".to_string(),
        ),
        (
            csa_core::env::CSA_RUN_GIT_PUSH_AUTHORIZED_ENV_KEY.to_string(),
            "true".to_string(),
        ),
    ]);

    let env = transport.build_env(&session, Some(&extra), None, false, false);

    assert!(!env.contains_key(csa_core::env::CSA_GIT_PUSH_ALLOWED_ENV_KEY));
    assert!(!env.contains_key(csa_core::env::CSA_RUN_GIT_PUSH_AUTHORIZED_ENV_KEY));
}

#[test]
fn build_env_applies_typed_git_push_authorization() {
    let transport = AcpTransport::new("claude-code", None);
    let session = sample_session();

    let env = transport.build_env(&session, None, None, true, false);

    assert_eq!(
        env.get(csa_core::env::CSA_GIT_PUSH_ALLOWED_ENV_KEY)
            .map(String::as_str),
        Some("true")
    );
    assert!(!env.contains_key(csa_core::env::CSA_RUN_GIT_PUSH_AUTHORIZED_ENV_KEY));
}

#[test]
fn build_env_reapplies_typed_no_post_exec_gate_after_extra_env_scrub() {
    let transport = AcpTransport::new("claude-code", None);
    let session = sample_session();
    let extra = HashMap::from([(
        csa_core::env::CSA_NO_POST_EXEC_GATE_ENV_KEY.to_string(),
        "0".to_string(),
    )]);

    let env = transport.build_env(&session, Some(&extra), None, false, true);

    assert_eq!(
        env.get(csa_core::env::CSA_NO_POST_EXEC_GATE_ENV_KEY)
            .map(String::as_str),
        Some("1")
    );
}

#[test]
fn build_env_preserves_system_sandbox_marker_over_extra_env() {
    let _env_lock = SANDBOX_ENV_LOCK.lock().expect("sandbox env lock poisoned");
    let _sandbox_guard = ScopedEnvVar::set(CSA_FS_SANDBOXED_ENV, "1");
    let transport = AcpTransport::new("claude-code", None);
    let session = sample_session();
    let extra = HashMap::from([(CSA_FS_SANDBOXED_ENV.to_string(), "0".to_string())]);

    let env = transport.build_env(&session, Some(&extra), None, false, false);

    assert_eq!(
        env.get(CSA_FS_SANDBOXED_ENV).map(String::as_str),
        Some("1"),
        "the process sandbox marker must override user extra_env"
    );
}

#[test]
fn build_env_reapplies_csa_owned_env_after_extra_env_merge() {
    let _env_lock = SANDBOX_ENV_LOCK.lock().expect("sandbox env lock poisoned");
    let _sandbox_guard = ScopedEnvVar::set(CSA_FS_SANDBOXED_ENV, "1");
    let _parent_tool_guard = ScopedEnvVar::set(CSA_TOOL_ENV, "parent-tool");
    let transport = AcpTransport::new("claude-code", None);
    let session = sample_child_session();
    let extra = HashMap::from([
        (
            CSA_SESSION_ID_ENV.to_string(),
            "spoofed-session".to_string(),
        ),
        (CSA_DEPTH_ENV.to_string(), "999".to_string()),
        (
            CSA_PROJECT_ROOT_ENV.to_string(),
            "/tmp/spoofed-root".to_string(),
        ),
        (CSA_INTERNAL_INVOCATION_ENV.to_string(), "0".to_string()),
        (CSA_TOOL_ENV.to_string(), "spoofed-tool".to_string()),
        (CSA_IS_SUBPROCESS_ENV.to_string(), "0".to_string()),
        (
            CSA_PARENT_TOOL_ENV.to_string(),
            "spoofed-parent-tool".to_string(),
        ),
        (
            CSA_PARENT_SESSION_ENV.to_string(),
            "spoofed-parent-session".to_string(),
        ),
        (
            CSA_DAEMON_SESSION_DIR_ENV.to_string(),
            "/tmp/spoofed-daemon-session-dir".to_string(),
        ),
        (CSA_FS_SANDBOXED_ENV.to_string(), "0".to_string()),
        (
            CSA_SESSION_DIR_ENV_KEY.to_string(),
            "/tmp/spoofed-session-dir".to_string(),
        ),
        (
            CSA_PARENT_SESSION_DIR_ENV_KEY.to_string(),
            "/tmp/spoofed-parent-session-dir".to_string(),
        ),
        (
            csa_session::RESULT_TOML_PATH_CONTRACT_ENV.to_string(),
            "/tmp/spoofed-result.toml".to_string(),
        ),
        ("CSA_SUPPRESS_NOTIFY".to_string(), "1".to_string()),
    ]);

    let env = transport.build_env(&session, Some(&extra), None, false, false);

    assert_eq!(
        env.get(CSA_SESSION_ID_ENV).map(String::as_str),
        Some("01HTEST000000000000000000")
    );
    assert_eq!(env.get(CSA_DEPTH_ENV).map(String::as_str), Some("1"));
    assert_eq!(
        env.get(CSA_PROJECT_ROOT_ENV).map(String::as_str),
        Some("/tmp/test")
    );
    assert_eq!(
        env.get(CSA_INTERNAL_INVOCATION_ENV).map(String::as_str),
        Some("1")
    );
    assert_eq!(
        env.get(CSA_TOOL_ENV).map(String::as_str),
        Some("claude-code")
    );
    assert_eq!(
        env.get(CSA_IS_SUBPROCESS_ENV).map(String::as_str),
        Some("1")
    );
    assert_eq!(
        env.get(CSA_PARENT_TOOL_ENV).map(String::as_str),
        Some("parent-tool")
    );
    assert_eq!(
        env.get(CSA_PARENT_SESSION_ENV).map(String::as_str),
        Some("01HPARENT000000000000000000")
    );
    assert!(
        !env.contains_key(CSA_DAEMON_SESSION_DIR_ENV),
        "CSA_DAEMON_SESSION_DIR must not flow into fresh ACP subprocess env"
    );
    assert_eq!(env.get(CSA_FS_SANDBOXED_ENV).map(String::as_str), Some("1"));
    assert_eq!(
        env.get("CSA_SUPPRESS_NOTIFY").map(String::as_str),
        Some("1"),
        "non-reserved CSA_* settings must still flow through extra_env"
    );

    let session_dir = env
        .get(CSA_SESSION_DIR_ENV_KEY)
        .expect("CSA_SESSION_DIR should be present");
    assert!(
        session_dir.contains("/sessions/"),
        "CSA_SESSION_DIR should be recomputed after merge, got: {session_dir}"
    );
    assert!(
        session_dir.contains("01HTEST000000000000000000"),
        "CSA_SESSION_DIR should include the session ID, got: {session_dir}"
    );

    let result_contract_path = env
        .get(csa_session::RESULT_TOML_PATH_CONTRACT_ENV)
        .expect("CSA_RESULT_TOML_PATH_CONTRACT should be present");
    assert!(
        result_contract_path.ends_with("/output/turns/turn-000001/result.toml"),
        "result contract path should be recomputed after merge, got: {result_contract_path}"
    );
    assert!(
        result_contract_path.contains("01HTEST000000000000000000"),
        "result contract path should include the session ID, got: {result_contract_path}"
    );
}
