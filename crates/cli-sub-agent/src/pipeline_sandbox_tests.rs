use std::path::{Path, PathBuf};

use super::*;
use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};

/// Helper: parse a TOML string into a ProjectConfig for test setup.
fn parse_project_config(toml_str: &str) -> csa_config::ProjectConfig {
    toml::from_str(toml_str).expect("test TOML should parse")
}

fn current_project_root() -> PathBuf {
    std::env::current_dir().unwrap_or_default()
}

fn comparable_test_path(path: &Path) -> PathBuf {
    csa_resource::isolation_plan::canonicalize_through_existing_ancestors(path)
        .unwrap_or_else(|_| path.to_path_buf())
}

fn contains_equivalent_path(paths: &[PathBuf], expected: &Path) -> bool {
    let expected = comparable_test_path(expected);
    paths
        .iter()
        .any(|path| comparable_test_path(path) == expected)
}

struct CurrentDirGuard {
    original: PathBuf,
}

impl CurrentDirGuard {
    fn change_to(path: &Path) -> Self {
        let original = std::env::current_dir().expect("current dir");
        std::env::set_current_dir(path).expect("set current dir");
        Self { original }
    }
}

impl Drop for CurrentDirGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.original);
    }
}

#[test]
fn test_filesystem_sandbox_active_helper() {
    let active = csa_session::SandboxInfo {
        mode: "cgroup".to_string(),
        memory_max_mb: None,
        filesystem_mode: Some("bwrap".to_string()),
        readonly_project_root: None,
    };
    let inactive = csa_session::SandboxInfo {
        mode: "cgroup".to_string(),
        memory_max_mb: None,
        filesystem_mode: Some("none".to_string()),
        readonly_project_root: None,
    };

    assert!(filesystem_sandbox_active(Some(&active)));
    assert!(!filesystem_sandbox_active(None));
    assert!(!filesystem_sandbox_active(Some(&inactive)));
}

#[test]
fn user_daemon_ipc_audit_artifact_records_capability() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let tmp = tempfile::tempdir().expect("tempdir");
    let runtime_dir = tmp.path().join("runtime");
    let systemd_dir = runtime_dir.join("systemd");
    std::fs::create_dir_all(&systemd_dir).expect("systemd dir");
    let bus_socket = runtime_dir.join("bus");
    let systemd_socket = systemd_dir.join("private");
    std::fs::write(&bus_socket, "").expect("bus socket placeholder");
    std::fs::write(&systemd_socket, "").expect("systemd socket placeholder");
    let _runtime_guard = ScopedEnvVarRestore::set("XDG_RUNTIME_DIR", &runtime_dir);
    let plan = csa_resource::isolation_plan::IsolationPlanBuilder::new(
        csa_resource::isolation_plan::EnforcementMode::BestEffort,
    )
    .with_filesystem_capability(csa_resource::FilesystemCapability::Bwrap)
    .with_readable_path(bus_socket.clone())
    .with_readable_path(systemd_socket.clone())
    .with_user_daemon_ipc()
    .build()
    .expect("test isolation plan");

    let session_dir = tmp.path().join("session");
    write_user_daemon_ipc_audit_artifact(&session_dir, &plan).expect("audit artifact write");

    let raw = std::fs::read_to_string(
        session_dir
            .join("output")
            .join("sandbox-capability-audit.json"),
    )
    .expect("audit artifact");
    let value: serde_json::Value = serde_json::from_str(&raw).expect("audit json");
    assert_eq!(value["capability"], "user-daemon-ipc");
    assert_eq!(
        value["reason"],
        "verification session requested user daemon restart capability"
    );
    assert!(value["timestamp"].as_str().is_some_and(|s| !s.is_empty()));
    let exposed_paths = value["exposed_paths"]
        .as_array()
        .expect("exposed_paths array");
    assert_eq!(exposed_paths.len(), 2);
    let bus_socket = bus_socket.display().to_string();
    let systemd_socket = systemd_socket.display().to_string();
    assert!(
        exposed_paths
            .iter()
            .any(|path| path.as_str() == Some(bus_socket.as_str()))
    );
    assert!(
        exposed_paths
            .iter()
            .any(|path| path.as_str() == Some(systemd_socket.as_str()))
    );
}

#[test]
fn sandbox_resolver_applies_user_daemon_ipc_capability() {
    let project_root = tempfile::tempdir().expect("project root tempdir");
    let cfg = parse_project_config(
        r#"
[resources]
memory_max_mb = 2048
enforcement_mode = "best-effort"
"#,
    );

    let result = resolve_sandbox_options_with_overrides(
        SandboxResolveInput {
            config: Some(&cfg),
            tool_name: "codex",
            session_id: "test-session",
            project_root: project_root.path(),
            stream_mode: StreamMode::BufferOnly,
            idle_timeout_seconds: 120,
            liveness_dead_seconds: 600,
            initial_response_timeout_seconds: Some(120),
            no_fs_sandbox: false,
            allow_user_daemon_ipc: true,
            readonly_project_root: false,
            extra_writable: &[],
            extra_readable: &[],
            execution_env: None,
        },
        RunResourceOverrides::default(),
    );

    let SandboxResolution::Ok(opts) = result else {
        panic!("Expected SandboxResolution::Ok");
    };
    let sandbox = opts.sandbox.expect("expected sandbox context");
    assert!(sandbox.isolation_plan.user_daemon_ipc);
    let audit_path = resolve_session_dir_for_sandbox(project_root.path(), "test-session")
        .join("output")
        .join("sandbox-capability-audit.json");
    assert!(audit_path.is_file(), "audit artifact should be written");
}

#[test]
fn record_sandbox_telemetry_overwrites_pre_spawn_projection() {
    let isolation_plan = csa_resource::isolation_plan::IsolationPlanBuilder::new(
        csa_resource::isolation_plan::EnforcementMode::BestEffort,
    )
    .with_resource_capability(csa_resource::ResourceCapability::None)
    .with_filesystem_capability(csa_resource::FilesystemCapability::None)
    .with_resource_limits(Some(8192), None, None)
    .with_readonly_project_root(true)
    .build()
    .expect("test isolation plan");
    let execute_options = csa_executor::ExecuteOptions::new(StreamMode::BufferOnly, 600)
        .with_sandbox(csa_executor::SandboxContext {
            isolation_plan,
            tool_name: "codex".to_string(),
            session_id: "test-session".to_string(),
            best_effort: true,
        });
    let mut session = csa_session::MetaSessionState {
        meta_session_id: "test-session".to_string(),
        sandbox_info: Some(csa_session::SandboxInfo {
            mode: "admission".to_string(),
            memory_max_mb: Some(12_288),
            filesystem_mode: None,
            readonly_project_root: None,
        }),
        ..Default::default()
    };

    assert!(record_sandbox_telemetry(&execute_options, &mut session));
    let info = session
        .sandbox_info
        .as_ref()
        .expect("sandbox telemetry should remain present");

    assert_ne!(info.mode, "admission");
    assert_eq!(info.memory_max_mb, Some(8192));
    assert_eq!(info.filesystem_mode.as_deref(), Some("none"));
    assert_eq!(info.readonly_project_root, Some(true));
}

#[test]
fn record_sandbox_telemetry_clears_pre_spawn_projection_without_sandbox() {
    let execute_options = csa_executor::ExecuteOptions::new(StreamMode::BufferOnly, 600);
    let mut session = csa_session::MetaSessionState {
        meta_session_id: "test-session".to_string(),
        sandbox_info: Some(csa_session::SandboxInfo {
            mode: "admission".to_string(),
            memory_max_mb: Some(12_288),
            filesystem_mode: None,
            readonly_project_root: None,
        }),
        ..Default::default()
    };

    assert!(record_sandbox_telemetry(&execute_options, &mut session));

    assert_eq!(session.sandbox_info, None);
}

#[test]
fn record_sandbox_telemetry_overwrites_pre_spawn_projection_without_memory_limit() {
    let isolation_plan = csa_resource::isolation_plan::IsolationPlanBuilder::new(
        csa_resource::isolation_plan::EnforcementMode::BestEffort,
    )
    .with_resource_capability(csa_resource::ResourceCapability::None)
    .with_filesystem_capability(csa_resource::FilesystemCapability::None)
    .with_resource_limits(None, None, None)
    .with_readonly_project_root(false)
    .build()
    .expect("test isolation plan");
    let execute_options = csa_executor::ExecuteOptions::new(StreamMode::BufferOnly, 600)
        .with_sandbox(csa_executor::SandboxContext {
            isolation_plan,
            tool_name: "gemini-cli".to_string(),
            session_id: "test-session".to_string(),
            best_effort: true,
        });
    let mut session = csa_session::MetaSessionState {
        meta_session_id: "test-session".to_string(),
        sandbox_info: Some(csa_session::SandboxInfo {
            mode: "admission".to_string(),
            memory_max_mb: Some(4096),
            filesystem_mode: None,
            readonly_project_root: None,
        }),
        ..Default::default()
    };

    assert!(record_sandbox_telemetry(&execute_options, &mut session));
    let info = session
        .sandbox_info
        .as_ref()
        .expect("runtime telemetry should replace admission marker");

    assert_ne!(info.mode, "admission");
    assert_eq!(info.memory_max_mb, None);
    assert_eq!(info.filesystem_mode.as_deref(), Some("none"));
    assert_eq!(info.readonly_project_root, Some(false));
}

#[test]
fn balloon_prewarm_skips_when_available_memory_is_low() {
    assert!(should_skip_balloon_prewarm(4096, 0));
}

#[test]
fn balloon_prewarm_skips_when_other_sessions_are_active() {
    assert!(should_skip_balloon_prewarm(16_384, 1));
}

#[test]
fn balloon_prewarm_allows_idle_host_with_memory_headroom() {
    assert!(!should_skip_balloon_prewarm(16_384, 0));
}

/// Heavyweight tools (claude-code) with no project config should get setting_sources=Some(vec![]).
#[test]
fn test_none_config_sets_setting_sources_for_heavyweight() {
    let result = resolve_sandbox_options(
        None,
        "claude-code",
        "test-session",
        &current_project_root(),
        StreamMode::BufferOnly,
        120,
        600,
        Some(120),
        false,
        false, // readonly_project_root
        &[],   // extra_writable
        &[],   // extra_readable
    );

    let SandboxResolution::Ok(opts) = result else {
        panic!("Expected SandboxResolution::Ok");
    };

    assert_eq!(
        opts.setting_sources,
        Some(vec![]),
        "Heavyweight tool should have setting_sources=Some(vec![]) (lean mode)"
    );
}

/// Lightweight tools (opencode) with no project config should NOT get a sandbox context.
#[test]
fn test_none_config_lightweight_skips_sandbox() {
    let result = resolve_sandbox_options(
        None,
        "opencode",
        "test-session",
        &current_project_root(),
        StreamMode::BufferOnly,
        120,
        600,
        Some(120),
        false,
        false, // readonly_project_root
        &[],   // extra_writable
        &[],   // extra_readable
    );

    let SandboxResolution::Ok(opts) = result else {
        panic!("Expected SandboxResolution::Ok");
    };

    assert_eq!(
        opts.setting_sources, None,
        "Lightweight tool should have setting_sources=None (load everything)"
    );
    assert!(
        opts.sandbox.is_none(),
        "Lightweight tool should have no sandbox context (enforcement=Off)"
    );
}

/// Heavyweight tools (claude-code) with no project config should get a sandbox context
/// when sandbox capability is available, or at least setting_sources when it is not.
#[test]
fn test_none_config_heavyweight_gets_sandbox() {
    let result = resolve_sandbox_options(
        None,
        "claude-code",
        "test-session",
        &current_project_root(),
        StreamMode::BufferOnly,
        120,
        600,
        Some(120),
        false,
        false, // readonly_project_root
        &[],   // extra_writable
        &[],   // extra_readable
    );

    let SandboxResolution::Ok(opts) = result else {
        panic!("Expected SandboxResolution::Ok");
    };

    // setting_sources is always set for heavyweight regardless of sandbox capability
    assert_eq!(
        opts.setting_sources,
        Some(vec![]),
        "Heavyweight tool should have setting_sources=Some(vec![])"
    );

    let capability = csa_resource::detect_resource_capability();
    if matches!(capability, csa_resource::ResourceCapability::None) {
        // On systems without sandbox capability, sandbox context is skipped
        assert!(
            opts.sandbox.is_none(),
            "No sandbox capability — should have no sandbox context"
        );
    } else {
        // On systems with sandbox capability, sandbox context should be present
        let ctx = opts
            .sandbox
            .as_ref()
            .expect("Expected SandboxContext for heavyweight tool");
        // IsolationPlan should have a resource capability set.
        assert_ne!(
            ctx.isolation_plan.resource,
            csa_resource::ResourceCapability::None,
            "Expected resource capability for heavyweight tool"
        );
        assert!(ctx.best_effort, "Profile defaults should use best-effort");
        assert_eq!(ctx.tool_name, "claude-code");
        assert_eq!(ctx.session_id, "test-session");

        let expected_tmpdir = match ctx.isolation_plan.filesystem {
            csa_resource::FilesystemCapability::Bwrap => PathBuf::from("/tmp"),
            csa_resource::FilesystemCapability::Landlock
            | csa_resource::FilesystemCapability::None => {
                csa_session::manager::get_session_dir(&current_project_root(), "test-session")
                    .expect("session dir")
                    .join("tmp")
            }
        };
        assert_eq!(
            ctx.isolation_plan.env_overrides.get("TMPDIR"),
            Some(&expected_tmpdir.to_string_lossy().into_owned()),
            "sandbox TMPDIR should match the active filesystem capability"
        );
    }
}

#[test]
fn clean_room_sandbox_fails_closed_on_missing_or_degraded_capability() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project");
    let session = temp.path().join("state/session");
    let evidence = temp.path().join("evidence.md");
    std::fs::create_dir_all(&project).expect("project");
    std::fs::create_dir_all(&session).expect("session");
    std::fs::write(&evidence, "evidence").expect("evidence");
    let config = parse_project_config(
        r#"
[resources]
min_free_memory_mb = 1
enforcement_mode = "best-effort"
"#,
    );
    let input = CleanRoomSandboxInput {
        config: Some(&config),
        tool_name: "opencode",
        session_id: "clean-session",
        project_root: &project,
        evidence_bundle: &evidence,
        session_dir: &session,
        idle_timeout_seconds: 30,
        initial_response_timeout_seconds: Some(10),
    };

    assert!(
        resolve_clean_room_sandbox_options_with_capabilities(
            input,
            RunResourceOverrides::default(),
            csa_resource::FilesystemCapability::None,
            csa_resource::ResourceCapability::None,
        )
        .is_err(),
        "missing filesystem isolation must fail closed"
    );
    assert!(
        resolve_clean_room_sandbox_options_with_capabilities(
            input,
            RunResourceOverrides::default(),
            csa_resource::FilesystemCapability::Bwrap,
            csa_resource::ResourceCapability::None,
        )
        .is_err(),
        "resource best-effort degradation must fail closed"
    );
}

#[test]
fn clean_room_sandbox_plan_is_strict_readonly_and_has_exact_exposures() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project");
    let session = temp.path().join("state/session");
    let evidence = temp.path().join("evidence.md");
    std::fs::create_dir_all(&project).expect("project");
    std::fs::create_dir_all(&session).expect("session");
    std::fs::write(&evidence, "evidence").expect("evidence");
    let config = parse_project_config(
        r#"
[resources]
min_free_memory_mb = 1
enforcement_mode = "off"
"#,
    );

    let options = resolve_clean_room_sandbox_options_with_capabilities(
        CleanRoomSandboxInput {
            config: Some(&config),
            tool_name: "opencode",
            session_id: "clean-session",
            project_root: &project,
            evidence_bundle: &evidence,
            session_dir: &session,
            idle_timeout_seconds: 30,
            initial_response_timeout_seconds: Some(10),
        },
        RunResourceOverrides::default(),
        csa_resource::FilesystemCapability::Bwrap,
        csa_resource::ResourceCapability::None,
    )
    .expect("strict clean-room plan");
    let sandbox = options.sandbox.expect("required sandbox context");
    let plan = sandbox.isolation_plan;

    assert!(!sandbox.best_effort);
    assert!(plan.readonly_project_root);
    assert_eq!(plan.project_root.as_deref(), Some(project.as_path()));
    assert!(!plan.user_daemon_ipc);
    assert!(plan.degraded_reasons.is_empty());
    assert!(plan.env_overrides.is_empty());
    assert_eq!(plan.readable_paths, vec![evidence.canonicalize().unwrap()]);
    assert_eq!(
        plan.writable_paths,
        vec![
            project.canonicalize().unwrap(),
            session.canonicalize().unwrap()
        ]
    );
}

#[test]
fn clean_room_sandbox_rejects_read_write_overlap_before_spawn() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project");
    let session = temp.path().join("state/session");
    std::fs::create_dir_all(&project).expect("project");
    std::fs::create_dir_all(&session).expect("session");
    let config =
        parse_project_config("[resources]\nmin_free_memory_mb = 1\nenforcement_mode = \"off\"\n");
    let input = CleanRoomSandboxInput {
        config: Some(&config),
        tool_name: "opencode",
        session_id: "clean-session",
        project_root: &project,
        evidence_bundle: &session,
        session_dir: &session,
        idle_timeout_seconds: 30,
        initial_response_timeout_seconds: Some(10),
    };

    assert!(
        resolve_clean_room_sandbox_options_with_capabilities(
            input,
            RunResourceOverrides::default(),
            csa_resource::FilesystemCapability::Bwrap,
            csa_resource::ResourceCapability::None,
        )
        .is_err()
    );
}

// ---------------------------------------------------------------------------
// Per-tool filesystem sandbox integration tests
include!("pipeline_sandbox_tests_tail.rs");
