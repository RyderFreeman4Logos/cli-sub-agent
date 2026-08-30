use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

#[path = "../src/test_bounded_command.rs"]
#[expect(
    dead_code,
    reason = "the integration test captures command output with a deadline"
)]
mod test_bounded_command;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const GIT_TIMEOUT: Duration = Duration::from_secs(10);

fn csa_cmd(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_csa"));
    scrub_inherited_csa_env(&mut command);
    command
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("HOME", home)
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("TOKIO_WORKER_THREADS", "1")
        .env("CSA_DAEMON_INDEPENDENT_SCOPE", "0")
        .env("CSA_TEST_SKIP_HOST_MEMORY_ADMISSION", "1");
    command
}

fn scrub_inherited_csa_env(command: &mut Command) {
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("CSA_") {
            command.env_remove(key);
        }
    }
}

fn run_git(project: &Path, args: &[&str]) {
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
    let output = test_bounded_command::output_with_timeout(command, GIT_TIMEOUT);
    assert!(
        output.status.success(),
        "fixture git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_project(project: &Path) {
    fs::create_dir_all(project.join(".csa")).expect("create fixture config directory");
    fs::write(project.join("README.md"), "# fixture\n").expect("write fixture readme");
    fs::write(project.join("lefthook.yml"), "pre-commit:\n").expect("write fixture hooks");
    fs::write(
        project.join(".csa/config.toml"),
        r#"schema_version = 1

[resources]
min_free_memory_mb = 0
memory_max_mb = 9000
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
    .expect("write fixture config");
    run_git(project, &["-c", "init.defaultBranch=main", "init"]);
    run_git(project, &["config", "user.email", "test@example.com"]);
    run_git(project, &["config", "user.name", "Test User"]);
    run_git(project, &["add", "."]);
    run_git(project, &["commit", "-m", "initial"]);
    run_git(project, &["checkout", "-b", "fix/caller-guard-summary"]);
}

#[cfg(unix)]
fn install_fake_codex(home: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let bin = home.join("bin");
    fs::create_dir_all(&bin).expect("create fake tool directory");
    let codex = bin.join("codex");
    fs::write(
        &codex,
        r#"#!/bin/sh
printf '%s\n' 'wrapper control prelude' '<csa-caller-sa-guard>' '</csa-caller-sa-guard>'
printf '%s\n' '{"type":"turn.failed","error":{"message":"fixture codex startup rejected"}}' >&2
exit 86
"#,
    )
    .expect("write fake codex");
    let mut permissions = fs::metadata(&codex)
        .expect("fake codex metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&codex, permissions).expect("make fake codex executable");

    let which = bin.join("which");
    fs::write(
        &which,
        "#!/bin/sh\nif [ \"${1:-}\" = bwrap ]; then exit 1; fi\nexec /usr/bin/which \"$@\"\n",
    )
    .expect("write fake which");
    let mut permissions = fs::metadata(&which)
        .expect("fake which metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&which, permissions).expect("make fake which executable");
    bin
}

fn session_dir(home: &Path, project: &Path, session_id: &str) -> PathBuf {
    let canonical = project
        .canonicalize()
        .unwrap_or_else(|_| project.to_path_buf());
    let normalized = canonical
        .to_string_lossy()
        .trim_start_matches('/')
        .replace('/', std::path::MAIN_SEPARATOR_STR);
    home.join(".local/state/cli-sub-agent")
        .join(normalized)
        .join("sessions")
        .join(session_id)
}

fn output_diagnostic(output: &Output) -> String {
    format!(
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[cfg(unix)]
#[test]
fn daemon_run_persists_causal_summary_when_selected_summary_is_caller_guard() {
    let home = tempfile::tempdir().expect("temporary home");
    let fake_bin = install_fake_codex(home.path());
    let project = home.path().join("project");
    init_project(&project);
    let global_config = home.path().join(".config/cli-sub-agent/config.toml");
    fs::create_dir_all(global_config.parent().expect("global config parent"))
        .expect("create global config directory");
    fs::write(
        &global_config,
        format!(
            "[kv_cache.provider_ttls]\nfixture = 30\n\n[tools.codex.env]\nPATH = \"{}\"\n",
            fake_bin.display()
        ),
    )
    .expect("write global config");

    let launch = test_bounded_command::output_with_timeout(
        {
            let mut command = csa_cmd(home.path());
            command.current_dir(&project).args([
                "run",
                "--sa-mode",
                "true",
                "--no-post-exec-gate",
                "--tool",
                "codex",
                "--min-free-memory-mb",
                "0",
                "fixture failure",
            ]);
            command
        },
        COMMAND_TIMEOUT,
    );
    assert!(
        launch.status.success(),
        "daemon launch failed: {}",
        output_diagnostic(&launch)
    );
    let session_id = String::from_utf8_lossy(&launch.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .last()
        .expect("daemon launcher must end stdout with its session ID")
        .to_string();
    assert_eq!(
        session_id.len(),
        26,
        "unexpected daemon session ID: {session_id}"
    );
    assert!(
        String::from_utf8_lossy(&launch.stderr).contains("CSA:SESSION_STARTED"),
        "daemon launch did not publish a session marker: {}",
        output_diagnostic(&launch)
    );

    let wait = test_bounded_command::output_with_timeout(
        {
            let mut command = csa_cmd(home.path());
            command.current_dir(&project).args([
                "session",
                "wait",
                "--session",
                &session_id,
                "--model-provider",
                "fixture",
                "--cd",
                project.to_str().expect("UTF-8 fixture path"),
            ]);
            command
        },
        COMMAND_TIMEOUT,
    );
    assert!(
        !wait.status.success(),
        "failed tool session unexpectedly succeeded"
    );

    let result_path =
        session_dir(home.path(), &project, &session_id).join(csa_session::result::RESULT_FILE_NAME);
    assert!(
        result_path.exists(),
        "wait did not publish {}: {}",
        result_path.display(),
        output_diagnostic(&wait)
    );
    let result: csa_session::SessionResult =
        toml::from_str(&fs::read_to_string(&result_path).expect("read published result.toml"))
            .expect("parse durable result.toml");
    assert_eq!(result.status, "failure");
    assert_ne!(result.exit_code, 0);
    assert!(!result.summary.contains("csa-caller-sa-guard"));
    assert!(
        result.summary.contains("codex tool failure"),
        "expected causal caller-guard summary, got {:?}",
        result.summary
    );
    assert!(result.summary.contains("fixture codex startup rejected"));
    assert!(result.summary.chars().count() <= 240);
}
