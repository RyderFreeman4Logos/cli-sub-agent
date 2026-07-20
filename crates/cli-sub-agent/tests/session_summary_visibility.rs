use std::path::Path;
use std::process::Command;

use serial_test::serial;

const PRE_EXEC_SUMMARY: &str = "pre-exec: Direct --tool is blocked when tiers are configured.";
const HOST_ADMISSION_SUMMARY: &str = "pre-exec: CSA: host memory admission denied \u{2014} \
available=9929MB < required=15512MB (reserve=512MB + projected_spawn=15000MB). \
active_sessions=0 sampled_sessions=0 active_session_rss_mb=0 active_session_projected_mb=0 \
swap_available_mb=7204 combined_available_mb=17133. Free host memory, wait for active CSA \
sessions to finish, or lower tool memory limits before spawning more work. For one csa run, \
pass --memory-max-mb <MB> to lower projected_spawn or --min-free-memory-mb <MB> to lower the \
reserve. Persistent config keys: resources.memory_max_mb, tools.<tool>.memory_max_mb, \
resources.min_free_memory_mb.";
const SOFT_LIMIT_ADMISSION_SUMMARY: &str = "pre-exec: CSA: memory_soft_limit_admission denied \
-- codex writer soft memory threshold is 6300MB, below required=9000MB \
(memory_max_mb=9000MB, soft_limit_percent=70). This writer session is likely to be terminated \
by CSA's memory monitor after editing the worktree. Raise --memory-max-mb, \
resources.memory_max_mb, or tools.codex.memory_max_mb to at least 12858MB, or raise \
resources.soft_limit_percent only when host RAM makes that safe.";

struct EnvVarGuard {
    key: &'static str,
    original: Option<String>,
}

struct CsaEnvGuard {
    original: Vec<(std::ffi::OsString, std::ffi::OsString)>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let original = std::env::var(key).ok();
        // SAFETY: this e2e test is serialized while mutating process-wide env.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: this e2e test is serialized while restoring process-wide env.
        unsafe {
            match self.original.as_deref() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

impl CsaEnvGuard {
    fn clear() -> Self {
        let original: Vec<_> = std::env::vars_os()
            .filter(|(key, _)| key.to_string_lossy().starts_with("CSA_"))
            .collect();
        // SAFETY: these e2e tests are serialized while mutating process-wide env.
        unsafe {
            for (key, _) in &original {
                std::env::remove_var(key);
            }
        }
        Self { original }
    }
}

impl Drop for CsaEnvGuard {
    fn drop(&mut self) {
        // SAFETY: these e2e tests are serialized while restoring process-wide env.
        unsafe {
            for (key, _) in std::env::vars_os() {
                if key.to_string_lossy().starts_with("CSA_") {
                    std::env::remove_var(key);
                }
            }
            for (key, value) in &self.original {
                std::env::set_var(key, value);
            }
        }
    }
}

fn csa_cmd(tmp: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_csa"));
    scrub_inherited_csa_env(&mut cmd);
    cmd.env("HOME", tmp)
        .env("XDG_STATE_HOME", tmp.join(".local/state"))
        .env("XDG_CONFIG_HOME", tmp.join(".config"))
        .env("HERMES_MODEL_PROVIDER", "openai")
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

fn write_pre_exec_failure_result(session_dir: &Path) {
    write_failure_result(session_dir, PRE_EXEC_SUMMARY);
}

fn write_failure_result(session_dir: &Path, summary: &str) {
    std::fs::create_dir_all(session_dir.join("output")).expect("create empty output dir");
    std::fs::write(session_dir.join("stdout.log"), "").expect("write empty stdout log");
    std::fs::write(
        session_dir.join("result.toml"),
        format!(
            r#"status = "failure"
exit_code = 1
summary = {summary:?}
tool = "codex"
started_at = "2026-04-27T00:00:00Z"
completed_at = "2026-04-27T00:00:01Z"
gate_timeout = false
"#
        ),
    )
    .expect("write result.toml");
}

fn write_provider_usage_limit_verdict(session_dir: &Path, session_id: &str) {
    let output_dir = session_dir.join("output");
    std::fs::create_dir_all(&output_dir).expect("create output dir");
    let token_field = concat!("to", "ken");
    let fake_value = concat!("sk", "-", "sec", "...", "6789");
    std::fs::write(
        output_dir.join("review-verdict.json"),
        format!(
            r#"{{"schema_version":1,"session_id":"{session_id}","timestamp":"2026-04-01T00:00:00Z","decision":"unavailable","verdict_legacy":"UNAVAILABLE","severity_counts":{{"critical":0,"high":0,"medium":0,"low":0}},"primary_failure":"HTTP 429","failure_reason":"codex/openai/gpt-5.5/xhigh=You've hit your usage limit. Visit https://chatgpt.com/codex/settings/usage to purchase more credits or try again at Jun 20th, 2026 6:48 PM. {token_field}={fake_value}","prior_round_refs":[]}}"#
        ),
    )
    .expect("write review verdict");
}

fn create_empty_output_failure_session(
    tmp: &Path,
    name: &str,
) -> (std::path::PathBuf, String, std::path::PathBuf) {
    let state_home = tmp.join(".local/state");
    std::fs::create_dir_all(&state_home).expect("create state home");
    let _home_guard = EnvVarGuard::set("HOME", tmp);
    let _state_guard = EnvVarGuard::set("XDG_STATE_HOME", &state_home);
    let _csa_env_guard = CsaEnvGuard::clear();

    let project = tmp.join(name);
    std::fs::create_dir_all(&project).expect("create project");
    let session =
        csa_session::create_session(&project, Some(name), None, Some("codex")).expect("session");
    let session_id = session.meta_session_id;
    let session_dir = csa_session::get_session_dir(&project, &session_id).expect("session dir");
    write_pre_exec_failure_result(&session_dir);

    (project, session_id, session_dir)
}

fn create_codex_usage_limit_failure_session(
    tmp: &Path,
    name: &str,
) -> (std::path::PathBuf, String, std::path::PathBuf, String) {
    let (project, session_id, session_dir) = create_empty_output_failure_session(tmp, name);
    write_failure_result(
        &session_dir,
        r#"{"type":"turn.failed","error":"provider_error"}"#,
    );
    let api_field = concat!("api", "_", "key");
    let fake_value = concat!("sk", "-", "sec", "...", "6789").to_string();
    std::fs::write(
        session_dir.join("output.log"),
        format!(
            "provider prelude that must stay hidden\n\
             You've hit your usage limit. Visit https://chatgpt.com/codex/settings/usage \
             to purchase more credits or try again at Jun 24th, 2026 3:39 PM. \
             {api_field}={fake_value}\n\
             provider epilogue that must stay hidden\n"
        ),
    )
    .expect("write output log");

    (project, session_id, session_dir, fake_value)
}

fn assert_subcommand_surfaces_summary_for_session(
    tmp: &Path,
    project: &Path,
    subcommand: &str,
    session_id: &str,
) {
    let output = csa_cmd(tmp)
        .args([
            "session",
            subcommand,
            "--session",
            session_id,
            "--cd",
            project.to_str().expect("project path utf8"),
        ])
        .current_dir(project)
        .output()
        .expect("run csa session command");

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains(PRE_EXEC_SUMMARY),
        "session command should surface result.toml summary, got stdout={stdout} stderr={stderr}"
    );
}

fn assert_subcommand_surfaces_summary(subcommand: &str, session_name: &str) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (project, session_id, _) = create_empty_output_failure_session(tmp.path(), session_name);
    assert_subcommand_surfaces_summary_for_session(tmp.path(), &project, subcommand, &session_id);
}

fn assert_session_result_summary_surfaces_pre_exec_result(
    session_name: &str,
    summary: &str,
    expected_fragments: &[&str],
) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (project, session_id, session_dir) =
        create_empty_output_failure_session(tmp.path(), session_name);
    write_failure_result(&session_dir, summary);

    let output = csa_cmd(tmp.path())
        .args([
            "session",
            "result",
            "--summary",
            "--session",
            &session_id,
            "--cd",
            project.to_str().expect("project path utf8"),
        ])
        .current_dir(&project)
        .output()
        .expect("run csa session result --summary");

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    for expected in expected_fragments {
        assert!(
            combined.contains(expected),
            "session result summary should contain {expected:?}, got stdout={stdout} stderr={stderr}"
        );
    }
    assert!(
        !combined.contains("No output found"),
        "session result summary should use result.toml instead of missing-output fallback, got stdout={stdout} stderr={stderr}"
    );
    assert!(
        combined.len() <= 700,
        "session result summary should stay bounded, got {} bytes: stdout={stdout} stderr={stderr}",
        combined.len()
    );
}

#[test]
#[serial]
fn session_wait_surfaces_result_summary_when_failure_output_is_empty() {
    assert_subcommand_surfaces_summary("wait", "wait-empty-output-failure");
}

#[test]
#[serial]
fn session_wait_surfaces_result_summary_when_only_output_log_has_content() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (project, session_id, session_dir) =
        create_empty_output_failure_session(tmp.path(), "wait-output-log-hidden-failure");
    std::fs::write(
        session_dir.join("output.log"),
        "hidden output.log content\n",
    )
    .expect("write output log");

    assert_subcommand_surfaces_summary_for_session(tmp.path(), &project, "wait", &session_id);
}

#[test]
#[serial]
fn session_wait_default_bounds_output_and_hides_stdout_log() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (project, session_id, session_dir) =
        create_empty_output_failure_session(tmp.path(), "wait-bounded-output");
    std::fs::write(
        session_dir.join("stdout.log"),
        format!("verbose-only {}\n", "x".repeat(10_000)),
    )
    .expect("write large stdout log");

    let output = csa_cmd(tmp.path())
        .args([
            "session",
            "wait",
            "--session",
            &session_id,
            "--cd",
            project.to_str().expect("project path utf8"),
        ])
        .current_dir(&project)
        .output()
        .expect("run csa session wait");

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let total_len = output.stdout.len() + output.stderr.len();
    assert!(
        total_len <= 2048,
        "wait output should be bounded to 2KB, got {total_len} bytes: stdout={stdout} stderr={stderr}"
    );
    assert!(stdout.contains("Session:"));
    assert!(stdout.contains("Status: failure"));
    assert!(stdout.contains("Exit code: 1"));
    assert!(stdout.contains("Tool: codex"));
    assert!(stdout.contains(PRE_EXEC_SUMMARY));
    assert!(!stdout.contains("verbose-only"));
}

#[test]
#[serial]
fn session_result_summary_surfaces_provider_usage_limit_reason() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (project, session_id, session_dir) =
        create_empty_output_failure_session(tmp.path(), "result-summary-provider-usage-limit");
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nUNAVAILABLE\n<!-- CSA:SECTION:summary:END -->",
    )
    .expect("persist structured summary");
    write_provider_usage_limit_verdict(&session_dir, &session_id);

    let output = csa_cmd(tmp.path())
        .args([
            "session",
            "result",
            "--summary",
            "--session",
            &session_id,
            "--cd",
            project.to_str().expect("project path utf8"),
        ])
        .current_dir(&project)
        .output()
        .expect("run csa session result --summary");

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("UNAVAILABLE"));
    assert!(stdout.contains("Unavailable reason: provider_usage_limit:"));
    assert!(stdout.contains("You've hit your usage limit."));
    assert!(stdout.contains("try again at Jun 20th, 2026 6:48 PM"));
    assert!(!stdout.contains(concat!("sk", "-", "sec", "...", "6789")));
    assert!(stdout.contains("[REDACTED]"));
    assert!(
        stdout.len() <= 700,
        "summary output should stay compact, got {} bytes: {stdout}",
        stdout.len()
    );
}

#[test]
#[serial]
fn session_result_summary_surfaces_pre_exec_host_admission_result() {
    assert_session_result_summary_surfaces_pre_exec_result(
        "result-summary-host-admission",
        HOST_ADMISSION_SUMMARY,
        &[
            "pre-exec: CSA: host memory admission denied",
            "Free host memory",
            "--memory-max-mb <MB> to lower projected_spawn",
            "--min-free-memory-mb <MB> to lower the reserve",
        ],
    );
}

#[test]
#[serial]
fn session_result_summary_surfaces_pre_exec_soft_limit_admission_result() {
    assert_session_result_summary_surfaces_pre_exec_result(
        "result-summary-soft-limit-admission",
        SOFT_LIMIT_ADMISSION_SUMMARY,
        &[
            "pre-exec: CSA: memory_soft_limit_admission denied",
            "codex writer soft memory threshold is 6300MB",
            "tools.codex.memory_max_mb to at least 12858MB",
            "resources.soft_limit_percent only when host RAM makes that safe",
        ],
    );
}

#[test]
#[serial]
fn session_result_summary_json_surfaces_provider_usage_limit_reason_without_output() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (project, session_id, session_dir) =
        create_empty_output_failure_session(tmp.path(), "result-summary-json-provider-limit");
    write_provider_usage_limit_verdict(&session_dir, &session_id);

    let output = csa_cmd(tmp.path())
        .args([
            "session",
            "result",
            "--summary",
            "--json",
            "--session",
            &session_id,
            "--cd",
            project.to_str().expect("project path utf8"),
        ])
        .current_dir(&project)
        .output()
        .expect("run csa session result --summary --json");

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let summary: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|err| panic!("stdout should be JSON: {err}; stdout={stdout}"));
    assert_eq!(summary["section"], "summary");
    assert!(summary["content"].is_null());
    let reason = summary["unavailable_reason"]
        .as_str()
        .expect("unavailable_reason string");
    assert!(reason.starts_with("provider_usage_limit:"));
    assert!(reason.contains("You've hit your usage limit."));
    assert!(reason.contains("try again at Jun 20th, 2026 6:48 PM"));
    assert!(!reason.contains(concat!("sk", "-", "sec", "...", "6789")));
    assert!(reason.contains("[REDACTED]"));
}

#[test]
#[serial]
fn session_result_summary_surfaces_codex_usage_limit_from_bounded_output() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (project, session_id, _, fake_value) = create_codex_usage_limit_failure_session(
        tmp.path(),
        "result-summary-codex-usage-limit-output",
    );

    let output = csa_cmd(tmp.path())
        .args([
            "session",
            "result",
            "--summary",
            "--session",
            &session_id,
            "--cd",
            project.to_str().expect("project path utf8"),
        ])
        .current_dir(&project)
        .output()
        .expect("run csa session result --summary");

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("provider quota exhausted: Codex usage limit hit"));
    assert!(stdout.contains("retry_after=try again at Jun 24th, 2026 3:39 PM"));
    assert!(stdout.contains("Hint: do not retry CSA-Codex sessions until cooldown expires"));
    assert!(!stdout.contains(&fake_value));
    assert!(!stdout.contains("provider prelude"));
    assert!(!stdout.contains("provider epilogue"));
    assert!(
        stdout.len() <= 700,
        "summary output should stay compact, got {} bytes: {stdout}",
        stdout.len()
    );
}

#[test]
#[serial]
fn session_wait_verbose_streams_stdout_log() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (project, session_id, session_dir) =
        create_empty_output_failure_session(tmp.path(), "wait-verbose-output");
    std::fs::write(session_dir.join("stdout.log"), "verbose visible\n").expect("write stdout log");

    let output = csa_cmd(tmp.path())
        .args([
            "session",
            "wait",
            "--verbose",
            "--session",
            &session_id,
            "--cd",
            project.to_str().expect("project path utf8"),
        ])
        .current_dir(&project)
        .output()
        .expect("run csa session wait --verbose");

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("verbose visible"),
        "verbose wait should stream stdout.log, got: {stdout}"
    );
}

#[test]
#[serial]
fn session_wait_env_verbose_streams_stdout_log() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (project, session_id, session_dir) =
        create_empty_output_failure_session(tmp.path(), "wait-env-verbose-output");
    std::fs::write(session_dir.join("stdout.log"), "env verbose visible\n")
        .expect("write stdout log");

    let output = csa_cmd(tmp.path())
        .env("CSA_WAIT_VERBOSE", "1")
        .args([
            "session",
            "wait",
            "--session",
            &session_id,
            "--cd",
            project.to_str().expect("project path utf8"),
        ])
        .current_dir(&project)
        .output()
        .expect("run csa session wait with env verbose");

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("env verbose visible"),
        "CSA_WAIT_VERBOSE=1 should stream stdout.log, got: {stdout}"
    );
}

#[test]
#[serial]
fn session_wait_json_outputs_parseable_summary() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (project, session_id, _) = create_empty_output_failure_session(tmp.path(), "wait-json");

    let output = csa_cmd(tmp.path())
        .args([
            "session",
            "wait",
            "--json",
            "--session",
            &session_id,
            "--cd",
            project.to_str().expect("project path utf8"),
        ])
        .current_dir(&project)
        .output()
        .expect("run csa session wait --json");

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    let summary: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|err| panic!("stdout should be JSON: {err}; stdout={stdout}"));
    assert_eq!(summary["session_id"], session_id);
    assert_eq!(summary["status"], "failure");
    assert_eq!(summary["exit_code"], 1);
    assert_eq!(summary["tool"], "codex");
    assert_eq!(summary["summary"], PRE_EXEC_SUMMARY);
}

#[test]
#[serial]
fn session_attach_surfaces_result_summary_when_failure_output_is_empty() {
    assert_subcommand_surfaces_summary("attach", "attach-empty-output-failure");
}
