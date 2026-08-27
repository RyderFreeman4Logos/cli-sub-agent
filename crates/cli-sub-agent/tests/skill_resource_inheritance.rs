use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

#[path = "../src/test_bounded_command.rs"]
mod test_bounded_command;

const SKILL_RUN_TIMEOUT: Duration = Duration::from_secs(30);
const SKILL_RUN_TERMINATION_GRACE: Duration = Duration::from_secs(1);
const GIT_FIXTURE_TIMEOUT: Duration = Duration::from_secs(10);

fn csa_cmd(home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_csa"));
    cmd.env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("HOME", home)
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("TOKIO_WORKER_THREADS", "1")
        .env("CSA_DAEMON_INDEPENDENT_SCOPE", "0")
        // Keep the inherited 9000MB contract assertion hermetic: gate
        // concurrency can leave less physical memory than that intentionally
        // large parent contract. This debug-only hook bypasses host admission
        // for the fake child; release builds keep production admission intact.
        .env("CSA_TEST_SKIP_HOST_MEMORY_ADMISSION", "1");
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
    let which = bin_dir.join("which");
    std::fs::write(
        &which,
        "#!/bin/sh\nif [ \"${1:-}\" = bwrap ]; then exit 1; fi\nexec /usr/bin/which \"$@\"\n",
    )
    .expect("write rejecting which fixture");
    let mut permissions = std::fs::metadata(&which)
        .expect("rejecting which metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&which, permissions).expect("make rejecting which executable");
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

fn run_fixture_git(project: &Path, args: &[&str]) {
    let mut command = Command::new("/usr/bin/git");
    command
        .env_clear()
        .args([
            "-c",
            "commit.gpgSign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "init.templateDir=",
        ])
        .args(args)
        .current_dir(project)
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("HOME", project)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("XDG_CONFIG_HOME", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@test.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@test.com");
    let output = test_bounded_command::output_with_timeout(command, GIT_FIXTURE_TIMEOUT);
    assert!(
        output.status.success(),
        "fixture git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn skill_resource_git_fixture_ignores_hostile_ambient_config() {
    use std::os::unix::fs::PermissionsExt;

    let ambient = tempfile::tempdir().expect("temporary hostile Git environment");
    let hooks = ambient.path().join("hooks");
    std::fs::create_dir(&hooks).expect("create hostile hooks directory");
    let marker = ambient.path().join("ambient-git-ran");
    let hostile_program = hooks.join("pre-commit");
    std::fs::write(
        &hostile_program,
        "#!/bin/sh\nprintf reached > \"$HOSTILE_GIT_MARKER\"\nexit 91\n",
    )
    .expect("write hostile Git hook and signer");
    std::fs::set_permissions(&hostile_program, std::fs::Permissions::from_mode(0o755))
        .expect("make hostile Git hook and signer executable");
    let config = ambient.path().join("hostile.gitconfig");
    let config_contents = format!(
        "[core]\n\thooksPath = {}\n[commit]\n\tgpgSign = true\n[gpg]\n\tprogram = {}\n[init]\n\ttemplateDir = {}\n",
        hooks.display(),
        hostile_program.display(),
        ambient.path().display()
    );
    std::fs::write(&config, &config_contents).expect("write hostile Git config");
    std::fs::create_dir(ambient.path().join("git")).expect("create hostile XDG Git directory");
    std::fs::write(ambient.path().join("git/config"), config_contents)
        .expect("write hostile XDG Git config");

    let status = test_bounded_command::status_with_timeout(
        {
            let mut command = Command::new(std::env::current_exe().expect("current test binary"));
            command
                .args([
                    "--exact",
                    "skill_run_preserves_plan_parent_resource_snapshot_for_nested_child",
                    "--nocapture",
                ])
                .env("HOME", ambient.path())
                .env("XDG_CONFIG_HOME", ambient.path())
                .env("GIT_CONFIG", &config)
                .env("GIT_CONFIG_SYSTEM", &config)
                .env("GIT_CONFIG_GLOBAL", &config)
                .env("GIT_CONFIG_COUNT", "1")
                .env("GIT_CONFIG_KEY_0", "core.hooksPath")
                .env("GIT_CONFIG_VALUE_0", &hooks)
                .env("GIT_TEMPLATE_DIR", ambient.path())
                .env("HOSTILE_GIT_MARKER", &marker);
            command
        },
        Duration::from_secs(45),
    );

    assert!(
        status.success(),
        "fixture failed under hostile ambient Git state with {status}"
    );
    let source = include_str!("skill_resource_inheritance.rs");
    let unbounded_status = [".sta", "tus()"].concat();
    assert!(
        !source.contains(&unbounded_status),
        "fixture Git setup must not use unbounded Command::status"
    );
    let bounded_call = [
        "test_bounded_command::output_with_timeout",
        "(command, GIT_FIXTURE_TIMEOUT)",
    ]
    .concat();
    assert!(
        source.contains(&bounded_call),
        "fixture Git setup must use the existing bounded subprocess helper"
    );
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
        &project_config,
        r#"schema_version = 1

[resources]
min_free_memory_mb = 128
soft_limit_percent = 100

[filesystem_sandbox]
enforcement_mode = "off"

[tools.codex]
enabled = true
transport = "cli"
default_model = "gpt-5.4-mini"

[run.post_exec_gate]
enabled = false
"#,
    )
    .expect("write project config");
    let parsed_config: csa_config::ProjectConfig = toml::from_str(
        &std::fs::read_to_string(&project_config).expect("read project config fixture"),
    )
    .expect("parse project config fixture");
    assert_eq!(
        parsed_config.filesystem_sandbox.enforcement_mode.as_deref(),
        Some("off"),
        "resource inheritance fixture must disable unrelated filesystem enforcement"
    );

    run_fixture_git(
        &project,
        &["-c", "init.defaultBranch=feat/resource-probe", "init", "-q"],
    );
    let hostile_marker = std::env::var_os("HOSTILE_GIT_MARKER");
    if hostile_marker.is_some() {
        assert!(
            !project.join(".git/hooks/pre-commit").exists(),
            "ambient Git template altered fixture initialization"
        );
    }
    run_fixture_git(&project, &["commit", "--allow-empty", "-qm", "init"]);
    if let Some(marker) = hostile_marker {
        assert!(
            !Path::new(&marker).exists(),
            "ambient Git hook or signer altered fixture setup"
        );
    }

    let fake_bin = install_fake_codex(&project);
    let mut command = csa_cmd(home.path());
    command
        .current_dir(&project)
        .env("PATH", prepend_path(&fake_bin))
        .env("CSA_INTERNAL_INVOCATION", "1")
        .env("CSA_DEPTH", "1")
        .env(
            "CSA_INHERITED_RESOURCE_OVERRIDES",
            r#"{"memory_max_mb":9000,"min_free_memory_mb":0}"#,
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
        r#"{"memory_max_mb":9000,"min_free_memory_mb":0}"#,
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
    assert_eq!(
        resolution.inherited_memory_max_mb,
        Some(csa_session::SourcedResourceValue {
            value: 9000,
            source: csa_session::ResourceValueSource::InheritedParentExplicit,
        })
    );
    assert_eq!(
        resolution.effective_memory_max_mb,
        resolution.inherited_memory_max_mb
    );
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
