use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn csa_cmd(tmp: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_csa"));
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("CSA_") {
            cmd.env_remove(key);
        }
    }
    cmd.env_remove("HERMES_MODEL_PROVIDER")
        .env("HOME", tmp)
        .env("XDG_STATE_HOME", tmp.join(".local/state"))
        .env("XDG_CONFIG_HOME", tmp.join(".config"))
        .env("TOKIO_WORKER_THREADS", "1");
    cmd
}

fn global_config_path(tmp: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        tmp.join("Library/Application Support/cli-sub-agent/config.toml")
    } else {
        tmp.join(".config/cli-sub-agent/config.toml")
    }
}

fn write_wait_config(tmp: &Path) {
    let path = global_config_path(tmp);
    std::fs::create_dir_all(path.parent().expect("config parent")).expect("create config dir");
    std::fs::write(
        path,
        r#"[kv_cache]
default_ttl_seconds = 240

[kv_cache.provider_ttls]
custom = 17
openai = 0
"#,
    )
    .expect("write wait config");
}

fn run_wait(tmp: &Path, extra_args: &[&str]) -> Output {
    let project = tmp.join("project");
    std::fs::create_dir_all(&project).expect("create project");
    let mut args = vec![
        "session",
        "wait",
        "--session",
        "01ARZ3NDEKTSV4RRFFQ69G5FBG",
        "--cd",
        project.to_str().expect("project path utf8"),
    ];
    args.extend_from_slice(extra_args);
    csa_cmd(tmp)
        .args(args)
        .output()
        .expect("run csa session wait")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn wait_without_provider_fails_closed_before_session_lookup() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_wait_config(tmp.path());

    let output = run_wait(tmp.path(), &[]);
    let stderr = stderr(&output);

    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("csa session wait requires --model-provider <key>"),
        "{stderr}"
    );
    assert!(stderr.contains("Configured keys"), "{stderr}");
    assert!(stderr.contains("custom=17"), "{stderr}");
    assert!(stderr.contains("CSA:CALLER_HINT"), "{stderr}");
    assert!(stderr.contains("dynamically on every wait"), "{stderr}");
    assert!(
        !stderr.contains("session registry lookup failed"),
        "{stderr}"
    );
    assert!(!stderr.contains("default_ttl_seconds = 240"), "{stderr}");
}

#[test]
fn wait_with_detected_but_unconfigured_provider_fails_closed() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_wait_config(tmp.path());

    let output = csa_cmd(tmp.path())
        .env("HERMES_MODEL_PROVIDER", "not-configured")
        .args(["session", "wait", "01ARZ3NDEKTSV4RRFFQ69G5FAV"])
        .output()
        .expect("run csa session wait");

    assert!(!output.status.success());
    let stderr = stderr(&output);
    assert!(
        stderr.contains("requires --model-provider <key>"),
        "{stderr}"
    );
    assert!(stderr.contains("not-configured"), "{stderr}");
    assert!(
        stderr.contains("not a configured key with TTL > 0"),
        "{stderr}"
    );
    assert!(stderr.contains("CSA:CALLER_HINT"), "{stderr}");
    assert!(!stderr.contains("Session registry entry"), "{stderr}");
}

#[test]
fn wait_with_unconfigured_provider_lists_only_positive_legal_keys() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_wait_config(tmp.path());

    for provider in ["not-configured", "openai"] {
        let output = run_wait(tmp.path(), &["--model-provider", provider]);
        let stderr = stderr(&output);

        assert_eq!(output.status.code(), Some(1), "{stderr}");
        assert!(stderr.contains(provider), "{stderr}");
        assert!(stderr.contains("custom=17"), "{stderr}");
        assert!(!stderr.contains("openai=0"), "{stderr}");
        assert!(stderr.contains("CSA:CALLER_HINT"), "{stderr}");
        assert!(
            !stderr.contains("session registry lookup failed"),
            "{stderr}"
        );
    }
}

#[test]
fn wait_with_legal_custom_provider_reaches_session_lookup() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_wait_config(tmp.path());

    let output = run_wait(tmp.path(), &["--model-provider", "custom"]);
    let stderr = stderr(&output);

    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("session registry lookup failed"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("requires --model-provider <key>"),
        "{stderr}"
    );
}

#[test]
fn session_wait_help_requires_a_configured_provider_ttl() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let output = csa_cmd(tmp.path())
        .args(["session", "wait", "--help"])
        .output()
        .expect("run csa session wait --help");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success(), "{stdout}");
    assert!(
        stdout.contains("configured [kv_cache.provider_ttls] key"),
        "{stdout}"
    );
    assert!(!stdout.contains("default_ttl_seconds"), "{stdout}");
}
