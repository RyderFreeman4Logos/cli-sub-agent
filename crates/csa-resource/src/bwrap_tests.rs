use std::collections::HashMap;

use super::*;
use crate::sandbox::ResourceCapability;

/// Helper: extract the full argument list from a Command via Debug output.
fn command_args(cmd: &Command) -> Vec<String> {
    let debug = format!("{cmd:?}");
    // Debug format: "bwrap" "--ro-bind" "/" "/" ...
    // Parse quoted strings out of the debug representation.
    debug
        .split('"')
        .enumerate()
        .filter_map(|(i, s)| if i % 2 == 1 { Some(s.to_owned()) } else { None })
        .collect()
}

#[path = "bwrap_tests_virtual.rs"]
mod virtual_filesystem;

#[path = "bwrap_tests_pinning.rs"]
mod pinning;

#[test]
fn test_bwrap_command_basic() {
    let builder = BwrapCommandBuilder::new("/usr/bin/tool", &["--flag".into(), "arg".into()]);
    let cmd = builder.build().expect("valid bind paths");
    let args = command_args(&cmd);

    // Program is bwrap
    assert_eq!(args[0], "bwrap");

    // Core args present
    assert!(args.contains(&"--ro-bind".to_owned()));
    assert!(args.contains(&"--tmpfs".to_owned()));
    assert!(args.contains(&"--dev".to_owned()));
    assert!(args.contains(&"--proc".to_owned()));
    assert!(args.contains(&"--share-net".to_owned()));
    assert!(args.contains(&"--unshare-pid".to_owned()));
    assert!(args.contains(&"--die-with-parent".to_owned()));

    // Separator and tool binary
    assert!(args.contains(&"--".to_owned()));
    assert!(args.contains(&"/usr/bin/tool".to_owned()));
    assert!(args.contains(&"--flag".to_owned()));
    assert!(args.contains(&"arg".to_owned()));
}

#[test]
fn test_bwrap_command_with_writable_paths() {
    let mut builder = BwrapCommandBuilder::new("/usr/bin/tool", &[]);
    builder.with_writable_path(Path::new("/home/user/project"));
    builder.with_writable_path(Path::new("/tmp/session"));
    let cmd = builder.build().expect("valid bind paths");
    let args = command_args(&cmd);

    // Count --bind occurrences (writable bind mounts)
    let bind_positions: Vec<_> = args
        .iter()
        .enumerate()
        .filter(|(_, a)| *a == "--bind")
        .map(|(i, _)| i)
        .collect();

    assert_eq!(
        bind_positions.len(),
        2,
        "expected 2 writable --bind mounts, got {bind_positions:?}"
    );

    assert!(args.contains(&"/home/user/project".to_owned()));
    assert!(args.contains(&"/tmp/session".to_owned()));
}

#[test]
fn test_bwrap_non_tmp_writable_path_creates_parent_dir_before_bind() {
    let path = "/home/user/.local/state/cli-sub-agent/project/sessions/session-id";
    let parent = "/home/user/.local/state/cli-sub-agent/project/sessions";

    let mut builder = BwrapCommandBuilder::new("/usr/bin/tool", &[]);
    builder.with_writable_path(Path::new(path));
    let args = command_args(&builder.build().expect("valid bind paths"));

    let dir_pos = args
        .windows(2)
        .position(|window| window == ["--dir", parent])
        .expect("--dir writable path parent must be present");
    let bind_pos = args
        .windows(3)
        .position(|window| window == ["--bind", path, path])
        .expect("--bind writable path must be present");

    assert!(
        dir_pos < bind_pos,
        "--dir parent must precede --bind; args: {args:?}"
    );
}

#[test]
fn test_bwrap_from_isolation_plan_bwrap() {
    let plan = IsolationPlan {
        resource: ResourceCapability::None,
        filesystem: FilesystemCapability::Bwrap,
        writable_paths: vec![PathBuf::from("/project")],
        readable_paths: Vec::new(),
        env_overrides: HashMap::new(),
        degraded_reasons: Vec::new(),
        memory_max_mb: None,
        memory_swap_max_mb: None,
        pids_max: None,
        readonly_project_root: false,
        project_root: None,
        soft_limit_percent: None,
        memory_monitor_interval_seconds: None,
        user_daemon_ipc: false,
    };

    let result =
        from_isolation_plan(&plan, "/usr/bin/tool", &["run".into()]).expect("valid bind paths");
    assert!(result.is_some(), "Bwrap plan should produce Some(Command)");

    let cmd = result.unwrap();
    let args = command_args(&cmd);
    assert!(args.contains(&"/project".to_owned()));
    assert!(args.contains(&"/usr/bin/tool".to_owned()));
}

#[test]
fn test_bwrap_from_isolation_plan_none() {
    let plan = IsolationPlan {
        resource: ResourceCapability::None,
        filesystem: FilesystemCapability::None,
        writable_paths: Vec::new(),
        readable_paths: Vec::new(),
        env_overrides: HashMap::new(),
        degraded_reasons: Vec::new(),
        memory_max_mb: None,
        memory_swap_max_mb: None,
        pids_max: None,
        readonly_project_root: false,
        project_root: None,
        soft_limit_percent: None,
        memory_monitor_interval_seconds: None,
        user_daemon_ipc: false,
    };

    let result = from_isolation_plan(&plan, "/usr/bin/tool", &[]).expect("valid bind paths");
    assert!(result.is_none(), "Non-Bwrap plan should produce None");
}

#[test]
fn test_bwrap_env_passthrough() {
    let builder = BwrapCommandBuilder::new("/usr/bin/tool", &[]);
    let cmd = builder.build().expect("valid bind paths");
    let args = command_args(&cmd);

    // Find --setenv CSA_FS_SANDBOXED 1 sequence
    let setenv_positions: Vec<_> = args
        .iter()
        .enumerate()
        .filter(|(_, a)| *a == "--setenv")
        .map(|(i, _)| i)
        .collect();

    let found_sandbox_env = setenv_positions.iter().any(|&pos| {
        args.get(pos + 1).map(|s| s.as_str()) == Some("CSA_FS_SANDBOXED")
            && args.get(pos + 2).map(|s| s.as_str()) == Some("1")
    });

    assert!(
        found_sandbox_env,
        "CSA_FS_SANDBOXED=1 must be set via --setenv; args: {args:?}"
    );
}

#[test]
fn test_bwrap_gh_aider_bind_targets_overridden_sandbox_home() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host_home = temp.path().join("host-home");
    let gh_aider = host_home.join(".config/gh-aider");
    std::fs::create_dir_all(&gh_aider).expect("create gh-aider config");

    let mut builder = BwrapCommandBuilder::new("/usr/bin/tool", &[]);
    builder.with_env("HOME", "/sandbox/runtime-home");
    let cmd = builder
        .build_with_home(Some(&host_home))
        .expect("valid bind paths");
    let args = command_args(&cmd);

    let found_bind = args.windows(3).any(|window| {
        (window[0] == "--ro-bind" || window[0] == "--ro-bind-fd")
            && window[2] == "/sandbox/runtime-home/.config/gh-aider"
    });

    assert!(
        found_bind,
        "gh-aider config should bind from the host path into the sandbox HOME; args: {args:?}"
    );

    // --dir must precede the bind to create the mount target inside sandbox
    let dir_pos = args
        .iter()
        .enumerate()
        .find(|(_, a)| *a == "--dir" || a.as_str() == "/sandbox/runtime-home/.config")
        .map(|(i, _)| i);
    let bind_pos = args
        .iter()
        .enumerate()
        .find(|(i, _)| args.get(*i + 1).map(|s| s.as_str()) == Some(&gh_aider.to_string_lossy()))
        .map(|(i, _)| i);
    if let (Some(d), Some(b)) = (dir_pos, bind_pos) {
        assert!(
            d < b,
            "--dir for mount target must precede --ro-bind; args: {args:?}"
        );
    }
}

#[test]
fn test_bwrap_gh_aider_bind_not_skipped_when_session_dir_writable() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host_home = temp.path().join("host-home");
    let gh_aider = host_home.join(".config/gh-aider");
    std::fs::create_dir_all(&gh_aider).expect("create gh-aider config");

    // Simulate Gemini layout: sandbox HOME is under a writable session dir
    let session_dir = temp.path().join("session");
    std::fs::create_dir_all(&session_dir).expect("create session dir");
    let sandbox_home = session_dir.join("runtime/home");

    let mut builder = BwrapCommandBuilder::new("/usr/bin/tool", &[]);
    builder.with_env("HOME", &sandbox_home.to_string_lossy());
    builder.writable_paths.push(session_dir);
    let cmd = builder
        .build_with_home(Some(&host_home))
        .expect("valid bind paths");
    let args = command_args(&cmd);

    let found_bind = args.windows(3).any(|window| {
        (window[0] == "--ro-bind" || window[0] == "--ro-bind-fd")
            && window[2] == sandbox_home.join(".config/gh-aider").to_string_lossy()
    });

    assert!(
        found_bind,
        "gh-aider bind must NOT be skipped just because sandbox HOME is under a writable session dir; args: {args:?}"
    );
}

#[test]
fn test_bwrap_from_isolation_plan_sets_tmpdir_override() {
    let mut env_overrides = HashMap::new();
    env_overrides.insert("TMPDIR".to_string(), "/tmp".to_string());
    let plan = IsolationPlan {
        resource: ResourceCapability::None,
        filesystem: FilesystemCapability::Bwrap,
        writable_paths: vec![PathBuf::from("/project")],
        readable_paths: Vec::new(),
        env_overrides,
        degraded_reasons: Vec::new(),
        memory_max_mb: None,
        memory_swap_max_mb: None,
        pids_max: None,
        readonly_project_root: false,
        project_root: Some(PathBuf::from("/project")),
        soft_limit_percent: None,
        memory_monitor_interval_seconds: None,
        user_daemon_ipc: false,
    };

    let cmd = from_isolation_plan(&plan, "/usr/bin/tool", &[])
        .expect("valid bind paths")
        .expect("should produce command");
    let args = command_args(&cmd);
    let found_tmpdir_env = args
        .windows(3)
        .any(|window| window == ["--setenv", "TMPDIR", "/tmp"]);

    assert!(
        found_tmpdir_env,
        "TMPDIR must be pinned inside bwrap env overrides; args: {args:?}"
    );
}

#[test]
fn test_bwrap_from_isolation_plan_scrubs_subtree_contract_env_overrides() {
    let mut env_overrides = HashMap::new();
    env_overrides.insert("TMPDIR".to_string(), "/tmp".to_string());
    env_overrides.insert(
        csa_core::env::CSA_MODEL_SPEC_ENV_KEY.to_string(),
        "codex/openai/gpt-5.5/xhigh".to_string(),
    );
    env_overrides.insert(
        csa_core::env::CSA_DEPTH_ENV_KEY.to_string(),
        "99".to_string(),
    );
    env_overrides.insert(
        csa_core::env::CSA_GIT_PUSH_ALLOWED_ENV_KEY.to_string(),
        "true".to_string(),
    );
    env_overrides.insert(
        csa_core::env::CSA_RUN_GIT_PUSH_AUTHORIZED_ENV_KEY.to_string(),
        "true".to_string(),
    );
    let plan = IsolationPlan {
        resource: ResourceCapability::None,
        filesystem: FilesystemCapability::Bwrap,
        writable_paths: vec![PathBuf::from("/project")],
        readable_paths: Vec::new(),
        env_overrides,
        degraded_reasons: Vec::new(),
        memory_max_mb: None,
        memory_swap_max_mb: None,
        pids_max: None,
        readonly_project_root: false,
        project_root: Some(PathBuf::from("/project")),
        soft_limit_percent: None,
        memory_monitor_interval_seconds: None,
        user_daemon_ipc: false,
    };

    let cmd = from_isolation_plan(&plan, "/usr/bin/tool", &[])
        .expect("valid bind paths")
        .expect("should produce command");
    let args = command_args(&cmd);

    assert!(
        args.windows(3)
            .any(|window| window == ["--setenv", "TMPDIR", "/tmp"]),
        "non-contract env overrides must still be passed through; args: {args:?}"
    );
    for key in csa_core::env::STARTUP_SUBTREE_ENV_KEYS {
        assert!(
            !args
                .windows(3)
                .any(|window| window[0] == "--setenv" && window[1] == *key),
            "bwrap --setenv must not pass subtree-contract key {key}; args: {args:?}"
        );
    }
    for key in csa_core::env::GIT_PUSH_AUTHORIZATION_ENV_KEYS {
        assert!(
            !args
                .windows(3)
                .any(|window| window[0] == "--setenv" && window[1] == *key),
            "bwrap --setenv must not pass git-push authorization key {key}; args: {args:?}"
        );
    }
}

#[test]
fn test_bwrap_readonly_project_root() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp
        .path()
        .canonicalize()
        .expect("resolve fixture root")
        .join("project");
    std::fs::create_dir(&project).expect("create project root");
    let project_str = project.to_string_lossy().into_owned();
    let plan = crate::isolation_plan::IsolationPlanBuilder::new(
        crate::isolation_plan::EnforcementMode::BestEffort,
    )
    .with_filesystem_capability(FilesystemCapability::Bwrap)
    .with_writable_path(project.clone())
    .with_writable_path(PathBuf::from("/tmp/session"))
    .with_project_root(&project)
    .with_readonly_project_root(true)
    .build()
    .expect("pinned readonly project plan");

    let cmd = from_isolation_plan(&plan, "/usr/bin/tool", &[])
        .expect("valid bind paths")
        .expect("should produce command");
    let args = command_args(&cmd);

    // The project root should be mounted read-only through its retained FD.
    let project_after_ro_fd = args
        .windows(3)
        .any(|window| window[0] == "--ro-bind-fd" && window[2] == project_str);
    assert!(
        project_after_ro_fd,
        "project root should be --ro-bind-fd when readonly_project_root is true; args: {args:?}"
    );

    // /tmp/session should still be --bind (writable)
    let bind_positions: Vec<_> = args
        .iter()
        .enumerate()
        .filter(|(_, a)| *a == "--bind")
        .map(|(i, _)| i)
        .collect();
    let session_after_bind = bind_positions
        .iter()
        .any(|&pos| args.get(pos + 1).map(|s| s.as_str()) == Some("/tmp/session"));
    assert!(
        session_after_bind,
        "/tmp/session should be --bind (writable); args: {args:?}"
    );
}

#[test]
fn test_bwrap_extra_writable_under_tmp_ordering() {
    let mut builder = BwrapCommandBuilder::new("/usr/bin/tool", &[]);
    builder.with_writable_path(Path::new("/home/user/project"));
    builder.with_writable_path(Path::new("/tmp/foo"));
    let cmd = builder.build().expect("valid bind paths");
    let args = command_args(&cmd);

    // Locate key positions
    let pos = |needle: &str| args.iter().position(|a| a == needle);

    let tmpfs_pos = args
        .iter()
        .enumerate()
        .find(|(_, a)| *a == "--tmpfs")
        .map(|(i, _)| i)
        .expect("--tmpfs must be present");
    assert_eq!(
        args[tmpfs_pos + 1],
        "/tmp",
        "--tmpfs must be followed by /tmp"
    );

    // --tmpfs /tmp must appear BEFORE --bind /tmp/foo /tmp/foo
    let bind_tmp_foo_pos = args
        .iter()
        .enumerate()
        .filter(|(_, a)| *a == "--bind")
        .find(|(i, _)| args.get(i + 1).map(|s| s.as_str()) == Some("/tmp/foo"))
        .map(|(i, _)| i)
        .expect("--bind /tmp/foo must be present");
    assert!(
        tmpfs_pos < bind_tmp_foo_pos,
        "--tmpfs /tmp (pos {tmpfs_pos}) must come BEFORE --bind /tmp/foo (pos {bind_tmp_foo_pos}); args: {args:?}"
    );

    // --dir /tmp/foo must appear between --tmpfs and --bind for /tmp paths
    let dir_tmp_foo_pos = args
        .iter()
        .enumerate()
        .filter(|(_, a)| *a == "--dir")
        .find(|(i, _)| args.get(i + 1).map(|s| s.as_str()) == Some("/tmp/foo"))
        .map(|(i, _)| i)
        .expect("--dir /tmp/foo must be present for /tmp sub-paths");
    assert!(
        tmpfs_pos < dir_tmp_foo_pos && dir_tmp_foo_pos < bind_tmp_foo_pos,
        "--dir /tmp/foo (pos {dir_tmp_foo_pos}) must be between --tmpfs (pos {tmpfs_pos}) and --bind (pos {bind_tmp_foo_pos}); args: {args:?}"
    );

    // Non-/tmp writable path should NOT have --dir
    let dir_project = args
        .iter()
        .enumerate()
        .filter(|(_, a)| *a == "--dir")
        .any(|(i, _)| args.get(i + 1).map(|s| s.as_str()) == Some("/home/user/project"));
    assert!(
        !dir_project,
        "non-/tmp path /home/user/project should NOT have --dir; args: {args:?}"
    );

    // Non-/tmp writable path should still have --bind
    let bind_project = args
        .iter()
        .enumerate()
        .filter(|(_, a)| *a == "--bind")
        .any(|(i, _)| args.get(i + 1).map(|s| s.as_str()) == Some("/home/user/project"));
    assert!(
        bind_project,
        "/home/user/project should have --bind; args: {args:?}"
    );

    // Verify --ro-bind / / is still first
    assert_eq!(args[1], "--ro-bind");
    assert_eq!(args[2], "/");
    assert_eq!(args[3], "/");

    // Verify overall order: --ro-bind < --tmpfs < --bind < --
    let separator_pos = pos("--").expect("-- separator must be present");
    assert!(
        bind_tmp_foo_pos < separator_pos,
        "--bind must come before -- separator"
    );
}

#[path = "bwrap_tests_paths.rs"]
mod path_tests;
