use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const SKILL_RUN_TIMEOUT: Duration = Duration::from_secs(30);
const SKILL_RUN_TERMINATION_GRACE: Duration = Duration::from_secs(1);

fn csa_cmd(home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_csa"));
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("CSA_") {
            cmd.env_remove(key);
        }
    }
    cmd.env("HOME", home)
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("TOKIO_WORKER_THREADS", "1")
        .env("CSA_DAEMON_INDEPENDENT_SCOPE", "0");
    cmd
}

#[cfg(unix)]
fn install_fake_codex(project: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let bin_dir = project.join("bin");
    std::fs::create_dir_all(&bin_dir).expect("create fake tool directory");
    let codex = bin_dir.join("codex");
    std::fs::write(
        &codex,
        r#"#!/bin/sh
printf '%s' "${CSA_INHERITED_RESOURCE_OVERRIDES:-}" > "$CSA_SESSION_DIR/resource-overrides.json"
printf '%s\n' \
  '{"type":"thread.started","thread_id":"resource-inheritance-test"}' \
  '{"type":"item.completed","item":{"type":"agent_message","text":"done"}}' \
  '{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}'
"#,
    )
    .expect("write fake codex");
    let mut permissions = std::fs::metadata(&codex)
        .expect("fake codex metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&codex, permissions).expect("make fake codex executable");
    bin_dir
}

#[cfg(unix)]
fn prepend_path(bin_dir: &Path) -> std::ffi::OsString {
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![bin_dir.to_path_buf()];
    paths.extend(std::env::split_paths(&current));
    std::env::join_paths(paths).expect("join PATH")
}

fn find_file(root: &Path, name: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(root).ok()? {
        let path = entry.ok()?.path();
        if path.file_name().is_some_and(|candidate| candidate == name) {
            return Some(path);
        }
        if path.is_dir()
            && let Some(found) = find_file(&path, name)
        {
            return Some(found);
        }
    }
    None
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn skill_run_preserves_plan_parent_resource_snapshot_for_nested_child() {
    let home = tempfile::tempdir().expect("temporary home");
    let project = home.path().join("project");
    let skill_dir = project.join(".csa/skills/resource-probe");
    std::fs::create_dir_all(&skill_dir).expect("create test skill directory");
    std::fs::write(skill_dir.join("SKILL.md"), "# Resource probe\n").expect("write test skill");
    std::fs::write(
        skill_dir.join(".skill.toml"),
        r#"[skill]
name = "resource-probe"

[agent]
workspace_access = "read-only"
tools = [{ tool = "codex", model = "gpt-5.4-mini", thinking_budget = "low" }]
"#,
    )
    .expect("write test skill config");

    let project_config = project.join(".csa/config.toml");
    std::fs::write(
        project_config,
        r#"schema_version = 1

[resources]
min_free_memory_mb = 128

[tools.codex]
enabled = true
transport = "cli"
default_model = "gpt-5.4-mini"

[run.post_exec_gate]
enabled = false
"#,
    )
    .expect("write project config");

    let fake_bin = install_fake_codex(&project);
    let mut command = csa_cmd(home.path());
    command
        .current_dir(&project)
        .env("PATH", prepend_path(&fake_bin))
        .env("CSA_INTERNAL_INVOCATION", "1")
        .env("CSA_DEPTH", "1")
        .env("CSA_SKIP_BWRAP_PREFLIGHT", "1")
        .env(
            "CSA_INHERITED_RESOURCE_OVERRIDES",
            r#"{"min_free_memory_mb":0}"#,
        )
        .args(["skill", "run", "resource-probe", "inspect resources"]);
    let command_context = format!("{command:?}");
    let child = csa_process::spawn_tool(command.into(), None)
        .await
        .unwrap_or_else(|error| {
            panic!("spawn CSA skill command {command_context}: {error:#}");
        });
    let output = csa_process::wait_and_capture_with_idle_timeout(
        child,
        csa_process::StreamMode::BufferOnly,
        SKILL_RUN_TIMEOUT,
        SKILL_RUN_TIMEOUT,
        SKILL_RUN_TERMINATION_GRACE,
        None,
        csa_process::SpawnOptions::default(),
        Some(SKILL_RUN_TIMEOUT),
    )
    .await
    .unwrap_or_else(|error| {
        panic!("wait for CSA skill command {command_context}: {error:#}");
    });

    assert!(
        output.exit_code == 0,
        "skill run failed ({command_context}); stdout={} stderr={}",
        output.output,
        output.stderr_output
    );
    let capture = find_file(
        &home.path().join(".local/state/cli-sub-agent"),
        "resource-overrides.json",
    )
    .expect("locate child resource snapshot");
    assert_eq!(
        std::fs::read_to_string(&capture).expect("read child resource snapshot"),
        r#"{"min_free_memory_mb":0}"#,
        "the skill child must receive the plan parent's explicit resource snapshot"
    );

    let state: csa_session::MetaSessionState = toml::from_str(
        &std::fs::read_to_string(
            capture
                .parent()
                .expect("capture must live in a session directory")
                .join("state.toml"),
        )
        .expect("read session state"),
    )
    .expect("parse session state");
    let resolution = state
        .sandbox_info
        .and_then(|info| info.resource_resolution)
        .expect("session state must persist resource provenance");
    assert_eq!(resolution.inherited_memory_max_mb, None);
    assert_eq!(
        resolution.inherited_min_free_memory_mb,
        Some(csa_session::SourcedResourceValue {
            value: 0,
            source: csa_session::ResourceValueSource::InheritedParentExplicit,
        })
    );
    assert_eq!(
        resolution.effective_min_free_memory_mb,
        resolution.inherited_min_free_memory_mb
    );
}
