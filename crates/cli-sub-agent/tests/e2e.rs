#[path = "../src/cli.rs"]
mod cli_defs;
#[path = "../src/gc_args.rs"]
mod gc;

use clap::Parser;
use cli_defs::{AuditCommands, Cli, Commands, McpHubCommands, validate_command_args};
use csa_core::types::OutputFormat;
#[cfg(unix)]
use csa_session::{SessionPhase, ToolState};
use std::collections::HashMap;
#[cfg(unix)]
use std::ffi::OsString;
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;
use std::process::Command;
#[cfg(unix)]
use std::process::Output;
#[cfg(unix)]
use std::sync::Mutex;

#[cfg(unix)]
static E2E_ENV_LOCK: Mutex<()> = Mutex::new(());

/// Create a [`Command`] pointing at the built `csa` binary with HOME, XDG_STATE_HOME,
/// and XDG_CONFIG_HOME redirected to the given temp directory so tests never touch
/// real user state.
fn csa_cmd(tmp: &std::path::Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_csa"));
    scrub_inherited_csa_env(&mut cmd);
    cmd.env("HOME", tmp)
        .env("XDG_STATE_HOME", tmp.join(".local/state"))
        .env("XDG_CONFIG_HOME", tmp.join(".config"))
        .env("TOKIO_WORKER_THREADS", "1");
    cmd
}

fn scrub_inherited_csa_env(cmd: &mut Command) {
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("CSA_") {
            cmd.env_remove(key);
        }
    }
}

fn global_config_path(tmp: &Path) -> std::path::PathBuf {
    // Mirror the production resolver so the test writes the same platform-specific
    // global path that `csa config get` reads on Linux and macOS.
    if cfg!(target_os = "macos") {
        tmp.join("Library/Application Support/cli-sub-agent/config.toml")
    } else {
        tmp.join(".config/cli-sub-agent/config.toml")
    }
}

#[cfg(unix)]
struct E2eEnvGuard {
    originals: Vec<(&'static str, Option<OsString>)>,
}

#[cfg(unix)]
impl E2eEnvGuard {
    fn set(tmp: &Path) -> Self {
        let keys = [
            "HOME",
            "XDG_STATE_HOME",
            "XDG_CONFIG_HOME",
            "CSA_DAEMON_SESSION_ID",
            "CSA_DAEMON_SESSION_DIR",
            "CSA_DAEMON_PROJECT_ROOT",
        ];
        let originals = keys
            .iter()
            .map(|key| (*key, std::env::var_os(key)))
            .collect();

        // SAFETY: e2e tests that mutate process env hold E2E_ENV_LOCK.
        unsafe {
            std::env::set_var("HOME", tmp);
            std::env::set_var("XDG_STATE_HOME", tmp.join(".local/state"));
            std::env::set_var("XDG_CONFIG_HOME", tmp.join(".config"));
            std::env::remove_var("CSA_DAEMON_SESSION_ID");
            std::env::remove_var("CSA_DAEMON_SESSION_DIR");
            std::env::remove_var("CSA_DAEMON_PROJECT_ROOT");
        }

        Self { originals }
    }
}

#[cfg(unix)]
impl Drop for E2eEnvGuard {
    fn drop(&mut self) {
        for (key, value) in &self.originals {
            // SAFETY: e2e tests that mutate process env hold E2E_ENV_LOCK.
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

#[cfg(unix)]
struct PreviewScenario {
    active_id: String,
    live_id: String,
    dead_empty_id: String,
    dead_expired_id: String,
    active_dir: PathBuf,
    live_dir: PathBuf,
    dead_empty_dir: PathBuf,
    dead_expired_dir: PathBuf,
    live_orphan_dir: PathBuf,
    dead_orphan_dir: PathBuf,
    _live_lock: csa_lock::SessionLock,
    _live_orphan_lock: csa_lock::SessionLock,
}

#[cfg(unix)]
fn short_id(session_id: &str) -> &str {
    &session_id[..11.min(session_id.len())]
}

#[cfg(unix)]
fn set_file_mtime_seconds_ago(path: &Path, seconds_ago: u64) {
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
    // SAFETY: `utimensat` receives a valid NUL-terminated path and stack-allocated timespec array.
    let rc = unsafe { libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), 0) };
    assert_eq!(rc, 0, "utimensat failed for {}", path.display());
}

#[cfg(unix)]
fn backdate_tree(path: &Path, seconds_ago: u64) {
    if path.is_dir() {
        for entry in std::fs::read_dir(path).expect("read_dir") {
            let entry = entry.expect("dir entry");
            backdate_tree(&entry.path(), seconds_ago);
        }
    }
    set_file_mtime_seconds_ago(path, seconds_ago);
}

#[cfg(unix)]
fn seed_liveness_snapshot_candidate(session_dir: &Path) {
    let acp_path = session_dir.join("output").join("acp-events.jsonl");
    std::fs::create_dir_all(acp_path.parent().expect("acp path parent")).unwrap();
    std::fs::write(&acp_path, "{}\n").unwrap();
    set_file_mtime_seconds_ago(&acp_path, 120);
}

#[cfg(unix)]
fn seed_preview_session(
    project_root: &Path,
    description: &str,
    phase: SessionPhase,
    with_tool: bool,
) -> (String, PathBuf) {
    let last_accessed = chrono::Utc::now() - chrono::Duration::days(40);
    let mut session =
        csa_session::create_session(project_root, Some(description), None, None).unwrap();
    session.phase = phase;
    session.last_accessed = last_accessed;
    session.tools.clear();
    if with_tool {
        session.tools.insert(
            "codex".to_string(),
            ToolState {
                provider_session_id: Some(format!("provider-{}", session.meta_session_id)),
                last_action_summary: "completed".to_string(),
                last_exit_code: 0,
                updated_at: last_accessed,
                tool_version: None,
                token_usage: None,
            },
        );
    }
    csa_session::save_session(&session).unwrap();
    let session_dir = csa_session::get_session_dir(project_root, &session.meta_session_id).unwrap();
    backdate_tree(&session_dir, 120);

    (session.meta_session_id, session_dir)
}

#[cfg(unix)]
fn seed_preview_scenario(project_root: &Path) -> PreviewScenario {
    std::fs::create_dir_all(project_root).unwrap();

    let (active_id, active_dir) = seed_preview_session(
        project_root,
        "active preview guard",
        SessionPhase::Active,
        true,
    );
    let (live_id, live_dir) = seed_preview_session(
        project_root,
        "live preview guard",
        SessionPhase::Retired,
        false,
    );
    let (dead_empty_id, dead_empty_dir) = seed_preview_session(
        project_root,
        "dead empty preview",
        SessionPhase::Retired,
        false,
    );
    let (dead_expired_id, dead_expired_dir) = seed_preview_session(
        project_root,
        "dead expired preview",
        SessionPhase::Retired,
        true,
    );
    seed_liveness_snapshot_candidate(&dead_empty_dir);
    seed_liveness_snapshot_candidate(&dead_expired_dir);

    let live_lock = csa_lock::acquire_lock(&live_dir, "codex", "gc dry-run preview test").unwrap();
    assert!(csa_process::ToolLiveness::has_live_process(&live_dir));
    let sessions_dir = csa_session::get_session_root(project_root)
        .unwrap()
        .join("sessions");
    let live_orphan_dir = sessions_dir.join("01EEEE0000000000000000000F");
    let dead_orphan_dir = sessions_dir.join("01FFFF0000000000000000000G");
    std::fs::create_dir_all(&live_orphan_dir).unwrap();
    std::fs::create_dir_all(&dead_orphan_dir).unwrap();
    seed_liveness_snapshot_candidate(&dead_orphan_dir);
    let live_orphan_lock =
        csa_lock::acquire_lock(&live_orphan_dir, "codex", "gc dry-run orphan preview test")
            .unwrap();
    assert!(csa_process::ToolLiveness::has_live_process(
        &live_orphan_dir
    ));

    PreviewScenario {
        active_id,
        live_id,
        dead_empty_id,
        dead_expired_id,
        active_dir,
        live_dir,
        dead_empty_dir,
        dead_expired_dir,
        live_orphan_dir,
        dead_orphan_dir,
        _live_lock: live_lock,
        _live_orphan_lock: live_orphan_lock,
    }
}

#[cfg(unix)]
fn output_text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[cfg(unix)]
fn assert_command_success(output: &Output, command: &str) {
    assert!(
        output.status.success(),
        "{command} should exit 0\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
fn assert_gc_preview_matches_execution_candidates(preview: &str, scenario: &PreviewScenario) {
    let removal_lines: Vec<_> = preview
        .lines()
        .filter(|line| {
            line.contains("Would remove empty session")
                || line.contains("Would remove expired session")
        })
        .collect();
    assert!(
        !removal_lines
            .iter()
            .any(|line| line.contains(&scenario.active_id)),
        "dry-run removal preview must not list Active sessions: {preview}"
    );
    assert!(
        !removal_lines
            .iter()
            .any(|line| line.contains(&scenario.live_id)),
        "dry-run removal preview must not list live sessions: {preview}"
    );
    assert!(
        removal_lines
            .iter()
            .any(|line| line.contains(&scenario.dead_empty_id)),
        "dry-run preview must list the dead empty session that execution deletes: {preview}"
    );
    assert!(
        removal_lines
            .iter()
            .any(|line| line.contains(&scenario.dead_expired_id)),
        "dry-run preview must list the dead expired session that execution deletes: {preview}"
    );

    let orphan_lines: Vec<_> = preview
        .lines()
        .filter(|line| line.contains("Would remove orphan directory"))
        .collect();
    let live_orphan_dir = scenario.live_orphan_dir.display().to_string();
    let dead_orphan_dir = scenario.dead_orphan_dir.display().to_string();
    assert!(
        !orphan_lines
            .iter()
            .any(|line| line.contains(&live_orphan_dir)),
        "dry-run orphan preview must not list live orphan dirs: {preview}"
    );
    assert!(
        orphan_lines
            .iter()
            .any(|line| line.contains(&dead_orphan_dir)),
        "dry-run orphan preview must list the dead orphan dir execution deletes: {preview}"
    );
}

#[cfg(unix)]
fn assert_session_clean_preview_matches_execution_candidates(
    preview: &str,
    scenario: &PreviewScenario,
) {
    assert!(
        !preview.contains(short_id(&scenario.active_id)),
        "session clean dry-run preview must not list Active sessions: {preview}"
    );
    assert!(
        !preview.contains(short_id(&scenario.live_id)),
        "session clean dry-run preview must not list live sessions: {preview}"
    );
    assert!(
        preview.contains(short_id(&scenario.dead_empty_id)),
        "session clean dry-run preview must list the dead empty session execution deletes: {preview}"
    );
    assert!(
        preview.contains(short_id(&scenario.dead_expired_id)),
        "session clean dry-run preview must list the dead expired session execution deletes: {preview}"
    );
}

#[cfg(unix)]
fn assert_execution_deleted_only_preview_candidates(scenario: &PreviewScenario) {
    assert!(
        scenario.active_dir.join("state.toml").exists(),
        "execution must preserve Active sessions"
    );
    assert!(
        scenario.live_dir.join("state.toml").exists(),
        "execution must preserve live sessions"
    );
    assert!(
        !scenario.dead_empty_dir.exists(),
        "execution must delete the dead empty session shown in preview"
    );
    assert!(
        !scenario.dead_expired_dir.exists(),
        "execution must delete the dead expired session shown in preview"
    );
}

#[cfg(unix)]
fn assert_gc_execution_deleted_only_preview_candidates(scenario: &PreviewScenario) {
    assert_execution_deleted_only_preview_candidates(scenario);
    assert!(
        scenario.live_orphan_dir.exists(),
        "execution must preserve live orphan dirs"
    );
    assert!(
        !scenario.dead_orphan_dir.exists(),
        "execution must delete the dead orphan dir shown in preview"
    );
}

#[cfg(unix)]
fn preview_target_dirs(scenario: &PreviewScenario) -> Vec<&Path> {
    vec![
        scenario.active_dir.as_path(),
        scenario.live_dir.as_path(),
        scenario.dead_empty_dir.as_path(),
        scenario.dead_expired_dir.as_path(),
        scenario.live_orphan_dir.as_path(),
        scenario.dead_orphan_dir.as_path(),
    ]
}

#[cfg(unix)]
fn relative_file_set(dir: &Path) -> std::collections::BTreeSet<PathBuf> {
    fn visit(root: &Path, dir: &Path, files: &mut std::collections::BTreeSet<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                visit(root, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(root)
                        .expect("path under root")
                        .to_path_buf(),
                );
            }
        }
    }

    let mut files = std::collections::BTreeSet::new();
    visit(dir, dir, &mut files);
    files
}

#[cfg(unix)]
fn snapshot_preview_target_files(
    scenario: &PreviewScenario,
) -> Vec<(PathBuf, std::collections::BTreeSet<PathBuf>)> {
    preview_target_dirs(scenario)
        .into_iter()
        .map(|dir| (dir.to_path_buf(), relative_file_set(dir)))
        .collect()
}

#[cfg(unix)]
fn assert_preview_target_files_unchanged(
    before: &[(PathBuf, std::collections::BTreeSet<PathBuf>)],
) {
    for (dir, files_before) in before {
        assert_eq!(
            &relative_file_set(dir),
            files_before,
            "dry-run preview must not create or remove files under {}",
            dir.display()
        );
    }
}

#[cfg(unix)]
fn assert_no_liveness_snapshots(scenario: &PreviewScenario) {
    for dir in preview_target_dirs(scenario) {
        assert!(
            !dir.join(".liveness.snapshot").exists(),
            "dry-run preview must not write .liveness.snapshot in {}",
            dir.display()
        );
    }
}

/// Run `csa init` (minimal mode) inside the given temp directory.
fn init_project(tmp: &std::path::Path) {
    let status = csa_cmd(tmp)
        .arg("init")
        .current_dir(tmp)
        .status()
        .expect("failed to run csa init");
    assert!(status.success(), "csa init should succeed");
}

/// Run `csa init --full` inside the given temp directory (full auto-detection mode).
fn init_project_full(tmp: &std::path::Path) {
    let status = csa_cmd(tmp)
        .args(["init", "--full"])
        .current_dir(tmp)
        .status()
        .expect("failed to run csa init --full");
    assert!(status.success(), "csa init --full should succeed");
}

fn write_project_config_with_tier(project_root: &Path) {
    let mut config = csa_config::ProjectConfig {
        schema_version: csa_config::config::CURRENT_SCHEMA_VERSION,
        project: csa_config::ProjectMeta {
            name: "test".to_string(),
            created_at: chrono::Utc::now(),
            max_recursion_depth: 5,
        },
        resources: csa_config::ResourcesConfig::default(),
        acp: Default::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        tool_aliases: HashMap::new(),
        preferences: None,
        github: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        tool_state_dirs: HashMap::new(),
        filesystem_sandbox: Default::default(),
    };
    config.tiers.insert(
        "default".to_string(),
        csa_config::config::TierConfig {
            description: "Test tier".to_string(),
            models: vec!["codex/openai/gpt-5-codex/medium".to_string()],
            strategy: csa_config::TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );

    let config_path = csa_config::ProjectConfig::config_path(project_root);
    std::fs::create_dir_all(config_path.parent().expect("config dir")).expect("create config dir");
    std::fs::write(
        config_path,
        toml::to_string_pretty(&config).expect("serialize config"),
    )
    .expect("write config");
}

fn session_root_for_temp_home(tmp: &Path, project_root: &Path) -> std::path::PathBuf {
    let app_state = if cfg!(target_os = "macos") {
        "Library/Application Support/cli-sub-agent"
    } else {
        ".local/state/cli-sub-agent"
    };
    let project_key = std::fs::canonicalize(project_root)
        .unwrap_or_else(|_| project_root.to_path_buf())
        .to_string_lossy()
        .trim_start_matches('/')
        .replace('/', std::path::MAIN_SEPARATOR_STR);
    tmp.join(app_state).join(project_key)
}

fn write_tiered_session(tmp: &Path, project_root: &Path) {
    let session_root = session_root_for_temp_home(tmp, project_root);
    let now = chrono::Utc::now();
    let session = csa_session::MetaSessionState {
        meta_session_id: csa_session::new_session_id(),
        description: Some("Tiered session".to_string()),
        project_path: std::fs::canonicalize(project_root)
            .unwrap_or_else(|_| project_root.to_path_buf())
            .to_string_lossy()
            .to_string(),
        branch: Some("feature/tier-column".to_string()),
        created_at: now,
        last_accessed: now,
        phase: csa_session::SessionPhase::Available,
        task_context: csa_session::TaskContext {
            tier_name: Some("tier-4-critical".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    let session_dir = session_root.join("sessions").join(&session.meta_session_id);
    std::fs::create_dir_all(session_dir.join("input")).expect("create session input dir");
    std::fs::create_dir_all(session_dir.join("output")).expect("create session output dir");
    csa_session::save_session_in(&session_root, &session).expect("save tiered session");
}

#[test]
fn cli_help_displays_correctly() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let output = csa_cmd(tmp.path())
        .arg("--help")
        .output()
        .expect("failed to run csa --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("CLI Sub-Agent"));
    assert!(stdout.contains("run"));
    assert!(stdout.contains("session"));
    assert!(stdout.contains("init"));
    assert!(stdout.contains("gc"));
    assert!(stdout.contains("config"));
}

#[test]
fn run_help_shows_tool_options() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let output = csa_cmd(tmp.path())
        .args(["run", "--goal", "test", "--help"])
        .output()
        .expect("failed to run csa run --goal test --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--tool"));
    assert!(stdout.contains("--goal"));
    assert!(stdout.contains("--session"));
    assert!(stdout.contains("--ephemeral"));
    assert!(stdout.contains("--model"));
}

#[test]
fn mcp_hub_serve_parse_with_background_and_socket() {
    let cli = Cli::try_parse_from([
        "csa",
        "mcp-hub",
        "serve",
        "--background",
        "--socket",
        "/tmp/cli-sub-agent-1000/mcp-hub.sock",
    ])
    .expect("mcp-hub serve args should parse");

    match cli.command {
        Commands::McpHub {
            cmd:
                McpHubCommands::Serve {
                    background,
                    foreground,
                    socket,
                    http_bind,
                    http_port,
                    systemd_activation,
                },
        } => {
            assert!(background);
            assert!(!foreground);
            assert_eq!(
                socket.as_deref(),
                Some("/tmp/cli-sub-agent-1000/mcp-hub.sock")
            );
            assert!(http_bind.is_none());
            assert!(http_port.is_none());
            assert!(!systemd_activation);
        }
        _ => panic!("expected mcp-hub serve subcommand"),
    }
}

#[test]
fn mcp_hub_gen_skill_parse_with_socket() {
    let cli = Cli::try_parse_from([
        "csa",
        "mcp-hub",
        "gen-skill",
        "--socket",
        "/tmp/cli-sub-agent-1000/mcp-hub.sock",
    ])
    .expect("mcp-hub gen-skill args should parse");

    match cli.command {
        Commands::McpHub {
            cmd: McpHubCommands::GenSkill { socket },
        } => {
            assert_eq!(
                socket.as_deref(),
                Some("/tmp/cli-sub-agent-1000/mcp-hub.sock")
            );
        }
        _ => panic!("expected mcp-hub gen-skill subcommand"),
    }
}

#[test]
fn review_help_shows_options() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let output = csa_cmd(tmp.path())
        .args(["review", "--help"])
        .output()
        .expect("failed to run csa review --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Review code changes using an AI tool"));
    assert!(stdout.contains("--tool"));
    assert!(stdout.contains("--session"));
    assert!(stdout.contains("--diff"));
    assert!(stdout.contains("--branch"));
    assert!(stdout.contains("--commit"));
    assert!(stdout.contains("--model"));
}

#[test]
fn push_help_shows_review_gate_options() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let output = csa_cmd(tmp.path())
        .args(["push", "--help"])
        .output()
        .expect("failed to run csa push --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Push the current branch only after a passing review covers HEAD"));
    assert!(stdout.contains("--force"));
    assert!(stdout.contains("--force-with-lease"));
    assert!(stdout.contains("--check-only"));
}

#[test]
fn review_cli_validation_applies_red_team_defaults() {
    let cli = Cli::try_parse_from(["csa", "review", "--red-team", "--diff"])
        .expect("review args should parse");

    match &cli.command {
        Commands::Review(args) => {
            validate_command_args(&cli.command, 1800).expect("review args should validate");
            assert_eq!(args.effective_review_mode().as_str(), "red-team");
            assert_eq!(args.effective_security_mode(), "on");
        }
        _ => panic!("expected review subcommand"),
    }
}

#[test]
fn review_cli_validation_rejects_red_team_with_security_off() {
    let cli = Cli::try_parse_from([
        "csa",
        "review",
        "--red-team",
        "--diff",
        "--security-mode",
        "off",
    ])
    .expect("review args should parse before validation");

    let err =
        validate_command_args(&cli.command, 1800).expect_err("validation should reject conflict");
    assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
}

#[test]
fn config_show_exits_zero_after_init_minimal() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_project(tmp.path());

    let output = csa_cmd(tmp.path())
        .args(["config", "show"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa config show");

    assert!(output.status.success(), "csa config show should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("schema_version"),
        "should contain schema_version"
    );
    assert!(
        stdout.contains("[project]"),
        "should contain [project] section"
    );
}

#[test]
fn config_show_exits_zero_after_init_full() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_project_full(tmp.path());

    let output = csa_cmd(tmp.path())
        .args(["config", "show"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa config show");

    assert!(output.status.success(), "csa config show should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("schema_version"),
        "should contain schema_version"
    );
    assert!(
        stdout.contains("[project]"),
        "should contain [project] section"
    );
    assert!(
        stdout.contains("[tools"),
        "should contain [tools.*] sections"
    );
}

#[test]
fn config_show_renders_project_session_wait_override() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config_path = csa_config::ProjectConfig::config_path(tmp.path());
    std::fs::create_dir_all(config_path.parent().expect("config dir")).expect("create config dir");
    std::fs::write(
        &config_path,
        r#"
[project]
name = "test-project"

[session_wait]
memory_warn_mb = 8192
"#,
    )
    .expect("write config");

    let project_root = tmp.path().display().to_string();
    let output = csa_cmd(tmp.path())
        .args(["config", "show", "--cd", &project_root])
        .output()
        .expect("failed to run csa config show --cd");

    assert!(output.status.success(), "csa config show should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[session_wait]"),
        "should contain [session_wait] section"
    );
    assert!(
        stdout.contains("memory_warn_mb = 8192"),
        "should contain rendered session_wait override"
    );
}

#[test]
fn config_get_resolves_nested_resource_keys_from_effective_display_tree() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config_path = csa_config::ProjectConfig::config_path(tmp.path());
    std::fs::create_dir_all(config_path.parent().expect("config dir")).expect("create config dir");
    std::fs::write(
        &config_path,
        r#"
schema_version = 1
[resources]
memory_max_mb = 1024
"#,
    )
    .expect("write config");

    let output = csa_cmd(tmp.path())
        .args(["config", "get", "resources.slot_wait_timeout_seconds"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa config get resources.slot_wait_timeout_seconds");

    assert!(output.status.success(), "config get should exit 0");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "250");
}

#[test]
fn config_get_project_only_resolves_effective_project_defaults() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config_path = csa_config::ProjectConfig::config_path(tmp.path());
    std::fs::create_dir_all(config_path.parent().expect("config dir")).expect("create config dir");
    std::fs::write(
        &config_path,
        r#"
schema_version = 1
[resources]
memory_max_mb = 1024
"#,
    )
    .expect("write config");

    let output = csa_cmd(tmp.path())
        .args([
            "config",
            "get",
            "resources.slot_wait_timeout_seconds",
            "--project",
        ])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa config get resources.slot_wait_timeout_seconds --project");

    assert!(
        output.status.success(),
        "project-only config get should exit 0"
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "250");
}

#[test]
fn config_get_prefers_effective_tool_state_over_raw_project_value() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let global_config_path = global_config_path(tmp.path());
    let global_dir = global_config_path.parent().expect("global config dir");
    std::fs::create_dir_all(global_dir).expect("create global config dir");
    std::fs::write(
        &global_config_path,
        r#"
[tools.codex]
enabled = false
"#,
    )
    .expect("write global config");

    let config_path = csa_config::ProjectConfig::config_path(tmp.path());
    std::fs::create_dir_all(config_path.parent().expect("config dir")).expect("create config dir");
    std::fs::write(
        &config_path,
        r#"
schema_version = 1
[tools.codex]
enabled = true
"#,
    )
    .expect("write project config");

    let output = csa_cmd(tmp.path())
        .args(["config", "get", "tools.codex.enabled"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa config get tools.codex.enabled");

    assert!(output.status.success(), "config get should exit 0");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "false");
}

#[test]
fn config_get_redacts_global_memory_api_keys_in_project_scoped_lookups() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let global_config_path = global_config_path(tmp.path());
    let global_dir = global_config_path.parent().expect("global config dir");
    std::fs::create_dir_all(global_dir).expect("create global config dir");
    std::fs::write(
        &global_config_path,
        r#"
[memory.llm]
enabled = true
api_key = "sk-super-secret-5982"
"#,
    )
    .expect("write global config");

    let config_path = csa_config::ProjectConfig::config_path(tmp.path());
    std::fs::create_dir_all(config_path.parent().expect("config dir")).expect("create config dir");
    std::fs::write(
        &config_path,
        r#"
schema_version = 1
[memory]
inject = true
"#,
    )
    .expect("write project config");

    let output = csa_cmd(tmp.path())
        .args(["config", "get", "memory"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa config get memory");

    assert!(output.status.success(), "config get should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("sk-super-secret-5982"),
        "config get leaked raw api key: {stdout}"
    );
    assert!(
        stdout.contains("api_key") && stdout.contains("..."),
        "config get should render a masked api key: {stdout}"
    );
}

#[test]
fn config_get_falls_back_to_raw_project_value_when_global_config_is_invalid() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let global_config_path = global_config_path(tmp.path());
    let global_dir = global_config_path.parent().expect("global config dir");
    std::fs::create_dir_all(global_dir).expect("create global config dir");
    std::fs::write(&global_config_path, "{{invalid toml").expect("write invalid global config");

    let config_path = csa_config::ProjectConfig::config_path(tmp.path());
    std::fs::create_dir_all(config_path.parent().expect("config dir")).expect("create config dir");
    std::fs::write(
        &config_path,
        r#"
schema_version = 1
[resources]
memory_max_mb = 1024
"#,
    )
    .expect("write project config");

    let output = csa_cmd(tmp.path())
        .args(["config", "get", "resources.memory_max_mb"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa config get resources.memory_max_mb");

    assert!(output.status.success(), "config get should exit 0");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "1024");
}

#[test]
fn config_get_reads_unknown_raw_project_sections() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config_path = csa_config::ProjectConfig::config_path(tmp.path());
    std::fs::create_dir_all(config_path.parent().expect("config dir")).expect("create config dir");
    std::fs::write(
        &config_path,
        r#"
schema_version = 1
[pr_review]
cloud_bot_name = "gemini-code-assist"
cloud_bot_trigger = "comment"
merge_strategy = "merge"
delete_branch = false
"#,
    )
    .expect("write config");

    let output = csa_cmd(tmp.path())
        .args(["config", "get", "pr_review.cloud_bot_name"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa config get pr_review.cloud_bot_name");

    assert!(output.status.success(), "config get should exit 0");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "gemini-code-assist"
    );
}

#[test]
fn config_get_returns_default_for_missing_keys() {
    let tmp = tempfile::tempdir().expect("tempdir");

    let output = csa_cmd(tmp.path())
        .args(["config", "get", "missing.key", "--default", "fallback"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa config get --default");

    assert!(
        output.status.success(),
        "config get --default should exit 0"
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "fallback");
}

#[test]
fn config_get_suggests_close_matches_for_missing_keys() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config_path = csa_config::ProjectConfig::config_path(tmp.path());
    std::fs::create_dir_all(config_path.parent().expect("config dir")).expect("create config dir");
    std::fs::write(
        &config_path,
        r#"
schema_version = 1
[resources]
memory_max_mb = 1024
"#,
    )
    .expect("write config");

    let output = csa_cmd(tmp.path())
        .args(["config", "get", "resources.slot_wait_timeout_second"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa config get with typo");

    assert!(!output.status.success(), "config get typo should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Closest matches:"),
        "stderr should include suggestions, got: {stderr}"
    );
    assert!(
        stderr.contains("resources.slot_wait_timeout_seconds"),
        "stderr should mention the closest key, got: {stderr}"
    );
}

#[test]
fn gc_dry_run_exits_zero() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let output = csa_cmd(tmp.path())
        .args(["gc", "--dry-run"])
        .output()
        .expect("failed to run csa gc --dry-run");

    assert!(output.status.success(), "csa gc --dry-run should exit 0");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        combined.contains("dry-run"),
        "output should mention dry-run mode"
    );
}

#[cfg(unix)]
#[test]
fn gc_dry_run_preview_excludes_active_and_live_sessions() {
    let _env_lock = E2E_ENV_LOCK.lock().expect("e2e env lock");
    let tmp = tempfile::tempdir().expect("tempdir");
    let _env = E2eEnvGuard::set(tmp.path());
    let project_root = tmp.path().join("project");
    let scenario = seed_preview_scenario(&project_root);
    let cd = project_root.to_string_lossy().to_string();
    let dry_run_files = snapshot_preview_target_files(&scenario);

    let preview_output = csa_cmd(tmp.path())
        .args(["gc", "--dry-run", "--max-age-days", "1", "--cd", &cd])
        .output()
        .expect("failed to run csa gc --dry-run");
    assert_command_success(&preview_output, "csa gc --dry-run");
    let preview = output_text(&preview_output);

    assert_gc_preview_matches_execution_candidates(&preview, &scenario);
    assert_preview_target_files_unchanged(&dry_run_files);
    assert_no_liveness_snapshots(&scenario);

    let execution_output = csa_cmd(tmp.path())
        .args(["gc", "--max-age-days", "1", "--cd", &cd])
        .output()
        .expect("failed to run csa gc");
    assert_command_success(&execution_output, "csa gc");
    assert_gc_execution_deleted_only_preview_candidates(&scenario);
}

#[cfg(unix)]
#[test]
fn gc_global_dry_run_preview_excludes_active_and_live_sessions() {
    let _env_lock = E2E_ENV_LOCK.lock().expect("e2e env lock");
    let tmp = tempfile::tempdir().expect("tempdir");
    let _env = E2eEnvGuard::set(tmp.path());
    let project_root = tmp.path().join("project");
    let scenario = seed_preview_scenario(&project_root);
    let dry_run_files = snapshot_preview_target_files(&scenario);

    let preview_output = csa_cmd(tmp.path())
        .args(["gc", "--global", "--dry-run", "--max-age-days", "1"])
        .output()
        .expect("failed to run csa gc --global --dry-run");
    assert_command_success(&preview_output, "csa gc --global --dry-run");
    let preview = output_text(&preview_output);

    assert_gc_preview_matches_execution_candidates(&preview, &scenario);
    assert_preview_target_files_unchanged(&dry_run_files);
    assert_no_liveness_snapshots(&scenario);

    let execution_output = csa_cmd(tmp.path())
        .args(["gc", "--global", "--max-age-days", "1"])
        .output()
        .expect("failed to run csa gc --global");
    assert_command_success(&execution_output, "csa gc --global");
    assert_gc_execution_deleted_only_preview_candidates(&scenario);
}

#[cfg(unix)]
#[test]
fn session_clean_dry_run_preview_excludes_active_and_live_sessions() {
    let _env_lock = E2E_ENV_LOCK.lock().expect("e2e env lock");
    let tmp = tempfile::tempdir().expect("tempdir");
    let _env = E2eEnvGuard::set(tmp.path());
    let project_root = tmp.path().join("project");
    let scenario = seed_preview_scenario(&project_root);
    let cd = project_root.to_string_lossy().to_string();
    let dry_run_files = snapshot_preview_target_files(&scenario);

    let preview_output = csa_cmd(tmp.path())
        .args(["session", "clean", "--days", "1", "--dry-run", "--cd", &cd])
        .output()
        .expect("failed to run csa session clean --dry-run");
    assert_command_success(&preview_output, "csa session clean --dry-run");
    let preview = output_text(&preview_output);

    assert_session_clean_preview_matches_execution_candidates(&preview, &scenario);
    assert_preview_target_files_unchanged(&dry_run_files);
    assert_no_liveness_snapshots(&scenario);

    let execution_output = csa_cmd(tmp.path())
        .args(["session", "clean", "--days", "1", "--cd", &cd])
        .output()
        .expect("failed to run csa session clean");
    assert_command_success(&execution_output, "csa session clean");
    assert_execution_deleted_only_preview_candidates(&scenario);
}

#[test]
fn tiers_list_exits_zero_after_init_full() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_project_full(tmp.path());

    let output = csa_cmd(tmp.path())
        .args(["tiers", "list"])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa tiers list");

    assert!(output.status.success(), "csa tiers list should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Full init defines at least these tiers.
    assert!(stdout.contains("tier-1"), "should list tier-1");
    assert!(stdout.contains("tier-2"), "should list tier-2");
    assert!(stdout.contains("tier-3"), "should list tier-3");
}

#[test]
fn skill_list_exits_zero() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let output = csa_cmd(tmp.path())
        .args(["skill", "list"])
        .output()
        .expect("failed to run csa skill list");

    assert!(output.status.success(), "csa skill list should exit 0");
}

#[test]
fn session_list_exits_zero() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let output = csa_cmd(tmp.path())
        .args(["session", "list"])
        .output()
        .expect("failed to run csa session list");

    assert!(output.status.success(), "csa session list should exit 0");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        combined.contains("No sessions found"),
        "empty state should report no sessions"
    );
}

#[test]
fn session_list_text_shows_tier_column_next_to_tools() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).expect("create project");
    write_tiered_session(tmp.path(), &project);

    let output = csa_cmd(tmp.path())
        .args(["session", "list", "--cd"])
        .arg(&project)
        .output()
        .expect("failed to run csa session list");

    assert!(
        output.status.success(),
        "csa session list should exit 0\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let header = stdout.lines().next().unwrap_or_default();
    let tools_idx = header.find("TOOLS").expect("TOOLS header");
    let tier_idx = header.find("TIER").expect("TIER header");
    let branch_idx = header.find("BRANCH").expect("BRANCH header");
    assert!(
        tools_idx < tier_idx && tier_idx < branch_idx,
        "TIER column should be between TOOLS and BRANCH, header: {header}",
    );
    assert!(
        stdout.contains("tier-4-critical"),
        "tier name should appear in session list output: {stdout}",
    );
}

#[test]
fn run_direct_tool_tier_rejection_surfaces_cause_and_session_id() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_project_config_with_tier(tmp.path());

    let output = csa_cmd(tmp.path())
        .args([
            "run",
            "--sa-mode",
            "true",
            "--no-daemon",
            "--tool",
            "codex",
            "--allow-base-branch-working",
            "--no-idle-timeout",
            "--timeout",
            "1800",
            "inspect the repository",
        ])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa run");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected exit code 1, got {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Direct --tool is blocked when tiers are configured"),
        "user-facing cause should be shown, stderr: {stderr}"
    );
    assert!(
        stderr.contains("--auto-route <intent>"),
        "actionable guidance should remain visible, stderr: {stderr}"
    );
    assert!(
        stderr.contains("Session ID:"),
        "session id should be preserved for diagnostics, stderr: {stderr}"
    );
    assert!(
        !stderr
            .lines()
            .find(|line| line.starts_with("Error:"))
            .unwrap_or_default()
            .contains("meta_session_id="),
        "top-level error line should not be opaque metadata, stderr: {stderr}"
    );
}

#[test]
fn test_audit_help() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let output = csa_cmd(tmp.path())
        .args(["audit", "--help"])
        .output()
        .expect("failed to run csa audit --help");

    assert!(output.status.success(), "csa audit --help should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Manage audit manifest lifecycle"));
    assert!(stdout.contains("init"));
    assert!(stdout.contains("status"));
    assert!(stdout.contains("sync"));
}

#[test]
fn test_audit_init_parse() {
    let cli = Cli::try_parse_from(["csa", "audit", "init", "--root", "."])
        .expect("audit init args should parse");

    match cli.command {
        Commands::Audit {
            command:
                AuditCommands::Init {
                    root,
                    ignore,
                    mirror_dir,
                },
        } => {
            assert_eq!(root, ".");
            assert!(ignore.is_empty());
            assert!(mirror_dir.is_none());
        }
        _ => panic!("expected audit init subcommand"),
    }
}

#[test]
fn test_audit_status_parse() {
    let cli = Cli::try_parse_from(["csa", "audit", "status", "--format", "json"])
        .expect("audit status args should parse");

    match cli.command {
        Commands::Audit {
            command:
                AuditCommands::Status {
                    format,
                    filter,
                    order,
                },
        } => {
            assert!(matches!(format, OutputFormat::Json));
            assert_eq!(filter, None);
            assert_eq!(order, "topo");
        }
        _ => panic!("expected audit status subcommand"),
    }
}
