use super::*;
use crate::SessionConfig;
use crate::transport_gemini_retry::*;
use csa_resource::isolation_plan::IsolationPlan;

include!("transport_tests_tail.rs");
include!("transport_tests_ephemeral.rs");
include!("transport_tests_gemini_fallback.rs");
include!("transport_tests_gemini_sandbox_lockstep.rs");
include!("transport_tests_gemini_init_classification.rs");
include!("transport_tests_gemini_acp_mcp_retry.rs");
include!("transport_tests_gemini_oauth_prompt.rs");
include!("transport_tests_extra.rs");
include!("transport_tests_codex_config.rs");
include!("transport_tests_codex_acp_stall.rs");
include!("transport_tests_capabilities.rs");

fn legacy_session_named(project_path: &std::path::Path, meta_session_id: &str) -> MetaSessionState {
    MetaSessionState {
        meta_session_id: meta_session_id.to_string(),
        project_path: project_path.to_string_lossy().into_owned(),
        created_at: chrono::Utc::now(),
        last_accessed: chrono::Utc::now(),
        ..Default::default()
    }
}

fn legacy_unique_session_id(tag: &str) -> String {
    format!(
        "01T{:08}{:010}{tag}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis()
            % 10_000_000_000u128
    )
}

struct LegacyTmuxGateTestLock(std::path::PathBuf);

impl Drop for LegacyTmuxGateTestLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.0);
    }
}

fn lock_legacy_tmux_gate_test() -> LegacyTmuxGateTestLock {
    let path = std::env::temp_dir().join("csa-tmux-no-post-exec-gate-test.lockdir");
    for _ in 0..200 {
        if std::fs::create_dir(&path).is_ok() {
            return LegacyTmuxGateTestLock(path);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("timed out waiting for tmux gate test lock");
}

fn read_legacy_gate_dump(path: &std::path::Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
        .trim()
        .to_string()
}

fn legacy_tmux_transport_options<'a>(
    no_post_exec_gate: bool,
    sandbox: Option<&'a crate::SandboxTransportConfig>,
) -> crate::TransportOptions<'a> {
    crate::TransportOptions {
        stream_mode: csa_process::StreamMode::BufferOnly,
        idle_timeout_seconds: 30,
        acp_crash_max_attempts: 2,
        initial_response_timeout: crate::ResolvedTimeout::disabled(),
        liveness_dead_seconds: 30,
        stdin_write_timeout_seconds: 30,
        acp_init_timeout_seconds: 30,
        termination_grace_period_seconds: 1,
        output_spool: None,
        output_spool_max_bytes: csa_process::DEFAULT_SPOOL_MAX_BYTES,
        output_spool_keep_rotated: csa_process::DEFAULT_SPOOL_KEEP_ROTATED,
        error_marker_scan_enabled: true,
        setting_sources: None,
        sandbox,
        thinking_budget: None,
        subtree_pin: None,
        allow_git_push: false,
        no_post_exec_gate,
        cancellation: None,
    }
}

fn install_fake_codex(bin_dir: &std::path::Path, dump: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    let script = bin_dir.join("codex");
    std::fs::write(
        &script,
        format!(
            r#"#!/bin/sh
set -eu
if [ "${{CSA_NO_POST_EXEC_GATE+x}}" = x ]; then
  printf 'set:%s\n' "$CSA_NO_POST_EXEC_GATE" >"{}"
else
  printf 'unset\n' >"{}"
fi
printf 'codex fixture\n'
"#,
            dump.display(),
            dump.display(),
        ),
    )
    .expect("write fake codex");
    let mut permissions = std::fs::metadata(&script)
        .expect("fake codex metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&script, permissions).expect("make fake codex executable");
}

fn link_legacy_fixture_tools(bin_dir: &std::path::Path) {
    for tool in [
        "bash", "cat", "env", "mkdir", "mkfifo", "rm", "sleep", "tee", "tmux",
    ] {
        let target = which::which(tool).unwrap_or_else(|err| panic!("find {tool}: {err}"));
        std::os::unix::fs::symlink(target, bin_dir.join(tool))
            .unwrap_or_else(|err| panic!("link fixture {tool}: {err}"));
    }
}

async fn run_legacy_tmux_gate(
    no_post_exec_gate: bool,
    sandbox: Option<&crate::SandboxTransportConfig>,
) -> String {
    let temp = tempfile::tempdir().expect("tempdir");
    let bin_dir = temp.path().join("bin");
    let dump = temp.path().join("gate-dump");
    std::fs::create_dir(&bin_dir).expect("create fixture bin");
    install_fake_codex(&bin_dir, &dump);
    link_legacy_fixture_tools(&bin_dir);
    let tmux_tmpdir = tempfile::tempdir().expect("tmux tempdir");
    let mut extra_env = HashMap::new();
    extra_env.insert("PATH".to_string(), bin_dir.display().to_string());
    extra_env.insert(
        "TMUX_TMPDIR".to_string(),
        tmux_tmpdir.path().display().to_string(),
    );

    let mut executor = Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: crate::codex_runtime::codex_runtime_metadata(),
    };
    executor.set_codex_tmux_mode(true);
    let session_id = legacy_unique_session_id("legacy");
    let session = legacy_session_named(temp.path(), &session_id);
    let transport = crate::LegacyTransport::new(executor);
    let result = transport
        .execute(
            "legacy tmux gate regression",
            None,
            &session,
            Some(&extra_env),
            legacy_tmux_transport_options(no_post_exec_gate, sandbox),
        )
        .await
        .expect("legacy tmux execution");
    assert_eq!(result.execution.exit_code, 0, "{result:?}");
    read_legacy_gate_dump(&dump)
}

#[tokio::test]
async fn tmux_cold_start_keeps_no_post_exec_gate_out_of_global_server_env_legacy_transport_primary()
{
    which::which("tmux").expect("tmux must be on PATH");
    let _lock = lock_legacy_tmux_gate_test();
    assert_eq!(
        run_legacy_tmux_gate(true, None).await,
        "set:1",
        "LegacyTransport primary path must preserve the trusted gate decision"
    );
}

#[tokio::test]
async fn tmux_inner_child_isolates_no_post_exec_gate_from_global_server_env_legacy_transport_fallback()
 {
    which::which("tmux").expect("tmux must be on PATH");
    let _lock = lock_legacy_tmux_gate_test();
    let sandbox = crate::SandboxTransportConfig {
        isolation_plan: csa_resource::isolation_plan::IsolationPlan {
            resource: csa_resource::sandbox::ResourceCapability::CgroupV2,
            filesystem: csa_resource::filesystem_sandbox::FilesystemCapability::None,
            writable_paths: Vec::new(),
            readable_paths: Vec::new(),
            env_overrides: HashMap::new(),
            degraded_reasons: Vec::new(),
            memory_max_mb: None,
            memory_swap_max_mb: None,
            pids_max: None,
            readonly_project_root: false,
            user_daemon_ipc: false,
            project_root: None,
            soft_limit_percent: None,
            memory_monitor_interval_seconds: None,
        },
        tool_name: "codex".to_string(),
        best_effort: true,
        session_id: legacy_unique_session_id("fallback"),
    };
    assert_eq!(
        run_legacy_tmux_gate(true, Some(&sandbox)).await,
        "set:1",
        "LegacyTransport fallback path must preserve the trusted gate decision"
    );
}
