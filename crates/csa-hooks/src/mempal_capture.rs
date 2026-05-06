//! Best-effort mempal capture for lifecycle hook artifacts.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use std::{fs, io};

use anyhow::{Context, anyhow};

use csa_config::{GlobalConfig, MemoryBackend, MemoryConfig, ProjectConfig};

const INGEST_TIMEOUT: Duration = Duration::from_secs(30);
const WING: &str = "cli-sub-agent";
const SOURCE_FILE: &str = "stdin://csa-hook";
const CLAUDE_CODE_TOOL: &str = "claude-code";

pub fn tool_has_own_mempal(tool: &str) -> bool {
    tool == CLAUDE_CODE_TOOL
}

/// Return the effective memory config using the same project-over-global
/// precedence as the execution pipeline.
pub fn load_effective_memory_config(project_root: &Path) -> Option<MemoryConfig> {
    if let Ok(Some(config)) = ProjectConfig::load(project_root)
        && !config.memory.is_default()
    {
        return Some(config.memory);
    }

    GlobalConfig::load()
        .ok()
        .map(|config| config.memory)
        .filter(|memory| !memory.is_default())
}

/// Spawn a non-blocking mempal ingest for a hook artifact path.
///
/// Failures are logged and never propagated. The worker thread enforces its own
/// timeout because hook capture must not delay the lifecycle action that fired it.
pub fn spawn_mempal_ingest(
    config: &MemoryConfig,
    room: &'static str,
    input_path: &Path,
    project_root: &Path,
    tool_name: Option<&str>,
) {
    let _ = spawn_mempal_ingest_with_resolver(
        config,
        room,
        input_path,
        project_root,
        tool_name,
        resolve_mempal_binary,
    );
}

fn spawn_mempal_ingest_with_resolver<F>(
    config: &MemoryConfig,
    room: &'static str,
    input_path: &Path,
    project_root: &Path,
    tool_name: Option<&str>,
    resolve_binary: F,
) -> Option<thread::JoinHandle<()>>
where
    F: FnOnce(&MemoryConfig) -> Option<PathBuf>,
{
    if let Some(tool) = tool_name
        && tool_has_own_mempal(tool)
    {
        tracing::debug!(
            tool,
            room,
            "skipping mempal capture for {tool} (has own integration)"
        );
        return None;
    }

    if !config.auto_capture {
        return None;
    }

    let binary_path = resolve_binary(config)?;

    let input_path = input_path.to_path_buf();
    let project_root = project_root.to_path_buf();
    Some(thread::spawn(move || {
        if let Err(err) = run_mempal_ingest(
            &binary_path,
            room,
            &input_path,
            &project_root,
            INGEST_TIMEOUT,
        ) {
            tracing::warn!(
                room,
                input = %input_path.display(),
                error = %err,
                "mempal ingest failed; continuing"
            );
        }
    }))
}

/// Convenience wrapper for merge-guard capture, where only the current working
/// directory is available.
pub fn spawn_mempal_ingest_for_project(
    project_root: &Path,
    room: &'static str,
    input_path: &Path,
    tool_name: Option<&str>,
) {
    if let Some(config) = load_effective_memory_config(project_root) {
        spawn_mempal_ingest(&config, room, input_path, project_root, tool_name);
    }
}

fn resolve_mempal_binary(config: &MemoryConfig) -> Option<PathBuf> {
    match config.backend {
        MemoryBackend::Legacy => None,
        MemoryBackend::Mempal | MemoryBackend::Auto => {
            csa_memory::detect_mempal().map(|info| PathBuf::from(&info.binary_path))
        }
    }
}

fn run_mempal_ingest(
    binary_path: &Path,
    room: &str,
    input_path: &Path,
    project_root: &Path,
    timeout: Duration,
) -> anyhow::Result<()> {
    let stdin_result = build_mempal_payload(room, input_path, project_root)
        .and_then(|payload| run_mempal_ingest_stdin(binary_path, &payload, timeout));
    match stdin_result {
        Ok(()) => Ok(()),
        Err(stdin_err) => {
            tracing::debug!(
                input = %input_path.display(),
                error = %stdin_err,
                "mempal stdin JSON ingest failed; falling back to path ingest"
            );
            let legacy_input_path = legacy_ingest_path(input_path);
            run_mempal_ingest_path(binary_path, room, &legacy_input_path, timeout)
                .with_context(|| format!("stdin JSON ingest failed: {stdin_err}"))?;
            Ok(())
        }
    }
}

fn legacy_ingest_path(input_path: &Path) -> PathBuf {
    if input_path.is_file()
        && let Some(parent) = input_path.parent()
        && parent.file_name().is_some()
    {
        return parent.to_path_buf();
    }
    input_path.to_path_buf()
}

fn build_mempal_payload(
    room: &str,
    input_path: &Path,
    project_root: &Path,
) -> anyhow::Result<Vec<u8>> {
    let content = read_ingest_content(input_path)?;
    let source = infer_source(input_path, room);
    let project_name = infer_project_name(project_root);
    let cwd = project_root.display().to_string();
    serde_json::to_vec(&serde_json::json!({
        "content": content,
        "wing": WING,
        "room": room,
        "project": project_name,
        "cwd": cwd,
        "claude_cwd": cwd,
        "source": source,
        "source_file": SOURCE_FILE,
    }))
    .context("failed to serialize mempal ingest payload")
}

fn read_ingest_content(input_path: &Path) -> anyhow::Result<String> {
    if input_path.is_dir() {
        let result_path = input_path.join("result.toml");
        if result_path.is_file() {
            return read_result_summary(&result_path);
        }

        for relative_path in [
            "output/summary.md",
            "output/full.md",
            "output.log",
            "merge-guard.jsonl",
        ] {
            let candidate = input_path.join(relative_path);
            if candidate.is_file() {
                return read_text_file(&candidate);
            }
        }

        anyhow::bail!(
            "no summary source found in mempal input directory {}",
            input_path.display()
        );
    }

    if input_path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "result.toml")
    {
        return read_result_summary(input_path);
    }

    read_text_file(input_path)
}

fn read_result_summary(result_path: &Path) -> anyhow::Result<String> {
    let contents = read_text_file(result_path)?;
    let parsed: toml::Value = toml::from_str(&contents).with_context(|| {
        format!(
            "failed to parse result summary from {}",
            result_path.display()
        )
    })?;
    parsed
        .get("summary")
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            anyhow!(
                "result.toml has no non-empty summary: {}",
                result_path.display()
            )
        })
}

fn read_text_file(path: &Path) -> anyhow::Result<String> {
    let contents = fs::read_to_string(path).with_context(|| {
        format!(
            "failed to read mempal ingest content from {}",
            path.display()
        )
    })?;
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "merge-guard.jsonl")
    {
        return contents
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .map(ToOwned::to_owned)
            .ok_or_else(|| anyhow!("merge guard event log is empty: {}", path.display()));
    }
    Ok(contents)
}

fn infer_source(input_path: &Path, room: &str) -> String {
    if room == "csa-merge" {
        return "csa-merge".to_string();
    }

    let session_dir = if input_path.is_dir() {
        input_path
    } else {
        input_path.parent().unwrap_or(input_path)
    };

    session_dir
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(|name| format!("csa-session-{name}"))
        .unwrap_or_else(|| format!("csa-session-{room}"))
}

fn infer_project_name(project_root: &Path) -> String {
    project_root
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| project_root.display().to_string())
}

fn run_mempal_ingest_stdin(
    binary_path: &Path,
    payload: &[u8],
    timeout: Duration,
) -> anyhow::Result<()> {
    let mut command = Command::new(binary_path);
    command
        .arg("ingest")
        .arg("--stdin")
        .arg("--json")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    let mut child = command.spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        match io::Write::write_all(&mut stdin, payload) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::BrokenPipe => {}
            Err(err) => return Err(err).context("failed to write mempal ingest payload to stdin"),
        }
    }
    wait_for_mempal_child(child, timeout)
}

fn run_mempal_ingest_path(
    binary_path: &Path,
    room: &str,
    input_path: &Path,
    timeout: Duration,
) -> anyhow::Result<()> {
    let mut command = Command::new(binary_path);
    command
        .arg("ingest")
        .arg("--wing")
        .arg(WING)
        .arg("--room")
        .arg(room)
        .arg(input_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    let child = command.spawn()?;
    wait_for_mempal_child(child, timeout)
}

fn wait_for_mempal_child(mut child: std::process::Child, timeout: Duration) -> anyhow::Result<()> {
    let start = Instant::now();
    loop {
        match child.try_wait()? {
            Some(status) if status.success() => return Ok(()),
            Some(status) => anyhow::bail!(
                "mempal ingest exited with code {}",
                status.code().unwrap_or(-1)
            ),
            None if start.elapsed() >= timeout => {
                #[cfg(unix)]
                {
                    // SAFETY: negative PID targets the process group created above.
                    unsafe {
                        libc::kill(-(child.id() as i32), libc::SIGKILL);
                    }
                }
                #[cfg(not(unix))]
                {
                    let _ = child.kill();
                }
                let _ = child.wait();
                anyhow::bail!("mempal ingest timed out after {}s", timeout.as_secs());
            }
            None => thread::sleep(Duration::from_millis(100)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ENV_LOCK;
    use std::fs;
    use std::io::Write as _;
    use std::path::PathBuf;

    struct ScopedPath {
        original: Option<std::ffi::OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl ScopedPath {
        fn prepend(path: &Path) -> Self {
            let lock = ENV_LOCK.lock().expect("env lock poisoned");
            let original = std::env::var_os("PATH");
            let mut paths = vec![path.to_path_buf()];
            paths.extend(std::env::split_paths(
                original
                    .as_deref()
                    .unwrap_or_else(|| std::ffi::OsStr::new("")),
            ));
            let joined = std::env::join_paths(paths).expect("join PATH");
            // SAFETY: test-scoped env mutation protected by ENV_LOCK.
            unsafe { std::env::set_var("PATH", joined) };
            Self {
                original,
                _lock: lock,
            }
        }

        fn set_only(path: &Path) -> Self {
            let lock = ENV_LOCK.lock().expect("env lock poisoned");
            let original = std::env::var_os("PATH");
            // SAFETY: test-scoped env mutation protected by ENV_LOCK.
            unsafe { std::env::set_var("PATH", path) };
            Self {
                original,
                _lock: lock,
            }
        }
    }

    impl Drop for ScopedPath {
        fn drop(&mut self) {
            // SAFETY: test-scoped env mutation protected by ENV_LOCK.
            unsafe {
                match self.original.take() {
                    Some(path) => std::env::set_var("PATH", path),
                    None => std::env::remove_var("PATH"),
                }
            }
        }
    }

    fn executable_mempal_script(path: &Path, body: &str) {
        let mut script = fs::File::create(path).expect("create fake mempal");
        write!(script, "{body}").expect("write fake mempal");
        script.sync_all().expect("sync fake mempal");
        drop(script);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(path).expect("metadata").permissions();
            perms.set_mode(0o755);
            for attempt in 0..5 {
                match fs::set_permissions(path, perms.clone()) {
                    Ok(()) => return,
                    Err(err) if err.raw_os_error() == Some(libc::ETXTBSY) && attempt < 4 => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(err) => panic!("chmod fake mempal: {err}"),
                }
            }
        }
    }

    fn mempal_config() -> MemoryConfig {
        MemoryConfig {
            backend: MemoryBackend::Mempal,
            auto_capture: true,
            ..MemoryConfig::default()
        }
    }

    fn which_mempal(_: &MemoryConfig) -> Option<PathBuf> {
        which::which("mempal").ok()
    }

    fn temp_project_root(temp: &tempfile::TempDir) -> PathBuf {
        let project_root = temp.path().join("warifu-ce");
        fs::create_dir(&project_root).expect("create project root");
        project_root
    }

    #[test]
    fn capture_with_mempal_available_sends_json_payload_to_stdin() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let project_root = temp_project_root(&temp);
        let fake_bin = temp.path().join("bin");
        fs::create_dir(&fake_bin).expect("create fake bin");
        let log_path = temp.path().join("stdin.json");
        executable_mempal_script(
            &fake_bin.join("mempal"),
            &format!(
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf 'mempal mock 0.0.0\\n'\n  exit 0\nfi\nif [ \"$1\" = \"ingest\" ] && [ \"$2\" = \"--stdin\" ] && [ \"$3\" = \"--json\" ]; then\n  cat > '{}'\n  exit 0\nfi\nexit 64\n",
                log_path.display()
            ),
        );
        let _path = ScopedPath::prepend(&fake_bin);

        let input_dir = temp.path().join("01KSESSION");
        fs::create_dir(&input_dir).expect("create session dir");
        let result_path = input_dir.join("result.toml");
        fs::write(&result_path, "summary = \"captured via hook\"\n").expect("write result");

        let handle = spawn_mempal_ingest_with_resolver(
            &mempal_config(),
            "csa-session",
            &result_path,
            &project_root,
            Some("codex"),
            which_mempal,
        )
        .expect("capture should spawn");
        handle.join().expect("capture worker should not panic");

        let payload: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(log_path).expect("read stdin log"))
                .expect("stdin payload is json");
        assert_eq!(payload["content"], "captured via hook");
        assert_eq!(payload["wing"], "cli-sub-agent");
        assert_eq!(payload["room"], "csa-session");
        assert_eq!(payload["project"], "warifu-ce");
        assert_eq!(payload["cwd"], project_root.display().to_string());
        assert_eq!(payload["claude_cwd"], project_root.display().to_string());
        assert_eq!(payload["source"], "csa-session-01KSESSION");
        assert_eq!(payload["source_file"], "stdin://csa-hook");
    }

    #[test]
    fn capture_skips_when_mempal_unavailable() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let project_root = temp_project_root(&temp);
        let empty_bin = temp.path().join("empty-bin");
        fs::create_dir(&empty_bin).expect("create empty bin");
        let _path = ScopedPath::set_only(&empty_bin);

        let result_path = temp.path().join("result.toml");
        fs::write(&result_path, "summary = \"no mempal available\"\n").expect("write result");

        let handle = spawn_mempal_ingest_with_resolver(
            &mempal_config(),
            "csa-session",
            &result_path,
            &project_root,
            Some("codex"),
            which_mempal,
        );

        assert!(
            handle.is_none(),
            "missing mempal should silently skip capture"
        );
    }

    #[test]
    fn capture_skips_for_claude_code_sessions() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let project_root = temp_project_root(&temp);
        let result_path = temp.path().join("result.toml");
        fs::write(&result_path, "summary = \"claude-code owns mempal\"\n").expect("write result");

        let handle = spawn_mempal_ingest_with_resolver(
            &mempal_config(),
            "csa-session",
            &result_path,
            &project_root,
            Some("claude-code"),
            |_| panic!("resolver must not run for claude-code sessions"),
        );

        assert!(
            handle.is_none(),
            "claude-code capture should be skipped before binary resolution"
        );
    }

    #[test]
    fn run_mempal_ingest_passes_expected_args() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let project_root = temp_project_root(&temp);
        let log_path = temp.path().join("args.log");
        let stdin_path = temp.path().join("stdin.json");
        let script_path = temp.path().join("mempal-fake.sh");
        executable_mempal_script(
            &script_path,
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\ncat > '{}'\n",
                log_path.display(),
                stdin_path.display()
            ),
        );

        let input_dir = temp.path().join("01ABCSESSION");
        fs::create_dir(&input_dir).expect("create input dir");
        let result_path = input_dir.join("result.toml");
        fs::write(&result_path, "summary = \"captured summary\"\n").expect("write result");
        run_mempal_ingest(
            &script_path,
            "csa-session",
            &result_path,
            &project_root,
            Duration::from_secs(5),
        )
        .expect("run fake mempal");

        let args = fs::read_to_string(log_path).expect("read args");
        assert_eq!(args, "ingest\n--stdin\n--json\n");
        let payload: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(stdin_path).expect("read stdin"))
                .expect("stdin payload is json");
        assert_eq!(payload["content"], "captured summary");
        assert_eq!(payload["wing"], "cli-sub-agent");
        assert_eq!(payload["room"], "csa-session");
        assert_eq!(payload["project"], "warifu-ce");
        assert_eq!(payload["cwd"], project_root.display().to_string());
        assert_eq!(payload["claude_cwd"], project_root.display().to_string());
        assert_eq!(payload["source"], "csa-session-01ABCSESSION");
        assert_eq!(payload["source_file"], "stdin://csa-hook");
    }

    #[test]
    fn run_mempal_ingest_falls_back_to_path_args_when_stdin_is_unsupported() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let project_root = temp_project_root(&temp);
        let log_path = temp.path().join("args.log");
        let script_path = temp.path().join("mempal-fake.sh");
        executable_mempal_script(
            &script_path,
            &format!(
                r#"#!/bin/sh
if [ "$2" = "--stdin" ]; then
  cat >/dev/null
  exit 2
fi
printf '%s\n' "$@" > '{}'
"#,
                log_path.display()
            ),
        );

        let input_dir = temp.path().join("session-output");
        fs::create_dir(&input_dir).expect("create input dir");
        let result_path = input_dir.join("result.toml");
        fs::write(&result_path, "summary = \"fallback summary\"\n").expect("write result");
        run_mempal_ingest(
            &script_path,
            "csa-session",
            &result_path,
            &project_root,
            Duration::from_secs(5),
        )
        .expect("run fake mempal");

        let args = fs::read_to_string(log_path).expect("read args");
        assert_eq!(
            args,
            format!(
                "ingest\n--wing\ncli-sub-agent\n--room\ncsa-session\n{}\n",
                input_dir.display()
            )
        );
    }

    #[test]
    fn legacy_backend_disables_capture() {
        let config = MemoryConfig {
            backend: MemoryBackend::Legacy,
            auto_capture: true,
            ..MemoryConfig::default()
        };
        assert!(resolve_mempal_binary(&config).is_none());
    }

    #[test]
    fn tool_has_own_mempal_only_matches_claude_code() {
        assert!(tool_has_own_mempal("claude-code"));
        assert!(!tool_has_own_mempal("codex"));
        assert!(!tool_has_own_mempal("gemini-cli"));
        assert!(!tool_has_own_mempal("opencode"));
    }

    #[test]
    fn infer_project_name_falls_back_to_full_path_when_basename_missing() {
        let root = Path::new("/");
        assert_eq!(infer_project_name(root), root.display().to_string());
    }
}
