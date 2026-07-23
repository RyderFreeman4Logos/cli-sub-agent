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
fn wait_requires_explicit_provider_even_when_environment_provider_is_configured() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_wait_config(tmp.path());

    let output = csa_cmd(tmp.path())
        .env("HERMES_MODEL_PROVIDER", "custom")
        .args(["session", "wait", "01ARZ3NDEKTSV4RRFFQ69G5FAV"])
        .output()
        .expect("run csa session wait");

    let stderr = stderr(&output);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("requires --model-provider <key>"),
        "{stderr}"
    );
    assert!(stderr.contains("custom=17"), "{stderr}");
    assert!(
        !stderr.contains("Detected hints"),
        "ambient provider detection must not affect the wait contract: {stderr}"
    );
    assert!(
        !stderr.contains("session registry lookup failed"),
        "{stderr}"
    );
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
fn wait_with_explicit_provider_fails_closed_when_config_is_missing_or_invalid() {
    let tmp = tempfile::tempdir().expect("tempdir");

    let missing = run_wait(tmp.path(), &["--model-provider", "xai"]);
    let missing_stderr = stderr(&missing);
    assert_eq!(missing.status.code(), Some(1), "{missing_stderr}");
    assert!(
        missing_stderr.contains("requires --model-provider <key>"),
        "{missing_stderr}"
    );
    assert!(
        !missing_stderr.contains("session registry lookup failed"),
        "{missing_stderr}"
    );

    let config_path = global_config_path(tmp.path());
    std::fs::create_dir_all(config_path.parent().expect("config parent"))
        .expect("create config dir");
    std::fs::write(
        &config_path,
        "[kv_cache.provider_ttls]\nxai = \"invalid\"\n",
    )
    .expect("write invalid provider config");

    let invalid = run_wait(tmp.path(), &["--model-provider", "xai"]);
    let invalid_stderr = stderr(&invalid);
    assert_eq!(invalid.status.code(), Some(1), "{invalid_stderr}");
    assert!(
        invalid_stderr.contains("requires --model-provider <key>"),
        "{invalid_stderr}"
    );
    assert!(
        invalid_stderr.contains("Config load error"),
        "{invalid_stderr}"
    );
    assert!(
        !invalid_stderr.contains("session registry lookup failed"),
        "{invalid_stderr}"
    );
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
fn wait_uses_only_explicitly_configured_provider_ttl_keys() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = global_config_path(tmp.path());
    std::fs::create_dir_all(path.parent().expect("config parent")).expect("create config dir");
    std::fs::write(path, "[kv_cache.provider_ttls]\ncustom = 17\n")
        .expect("write explicit-only wait config");

    let rejected = run_wait(tmp.path(), &["--model-provider", "claude"]);
    let rejected_stderr = stderr(&rejected);
    assert_eq!(rejected.status.code(), Some(1), "{rejected_stderr}");
    assert!(
        rejected_stderr.contains("requires --model-provider <key>"),
        "{rejected_stderr}"
    );
    assert!(rejected_stderr.contains("custom=17"), "{rejected_stderr}");
    assert!(
        !rejected_stderr.contains("session registry lookup failed"),
        "{rejected_stderr}"
    );

    let accepted = run_wait(tmp.path(), &["--model-provider", "custom"]);
    let accepted_stderr = stderr(&accepted);
    assert_eq!(accepted.status.code(), Some(1), "{accepted_stderr}");
    assert!(
        accepted_stderr.contains("session registry lookup failed"),
        "{accepted_stderr}"
    );
    assert!(
        !accepted_stderr.contains("requires --model-provider <key>"),
        "{accepted_stderr}"
    );
}

#[test]
fn session_wait_help_requires_an_explicit_configured_provider_ttl() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let output = csa_cmd(tmp.path())
        .args(["session", "wait", "--help"])
        .output()
        .expect("run csa session wait --help");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success(), "{stdout}");
    assert!(
        stdout.contains("Every wait requires an explicit normalized `--model-provider`"),
        "{stdout}"
    );
    assert!(
        stdout.contains("configured `[kv_cache.provider_ttls]` entry is > 0"),
        "{stdout}"
    );
    assert!(
        stdout.contains("Its TTL is resolved exactly from that entry"),
        "{stdout}"
    );
    assert!(
        stdout.contains("missing, unconfigured, or zero values fail closed"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("best-effort detection"),
        "obsolete ambient-provider wording must not appear: {stdout}"
    );
    assert!(!stdout.contains("default_ttl_seconds"), "{stdout}");
}
