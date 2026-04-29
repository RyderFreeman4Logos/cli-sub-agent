//! Best-effort mempal capture for lifecycle hook artifacts.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use csa_config::{GlobalConfig, MemoryBackend, MemoryConfig, ProjectConfig};

const INGEST_TIMEOUT: Duration = Duration::from_secs(30);
const WING: &str = "cli-sub-agent";

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
pub fn spawn_mempal_ingest(config: &MemoryConfig, room: &'static str, input_path: &Path) {
    if !config.auto_capture {
        return;
    }

    let Some(binary_path) = resolve_mempal_binary(config) else {
        return;
    };

    let input_path = input_path.to_path_buf();
    thread::spawn(move || {
        if let Err(err) = run_mempal_ingest(&binary_path, room, &input_path, INGEST_TIMEOUT) {
            tracing::warn!(
                room,
                input = %input_path.display(),
                error = %err,
                "mempal ingest failed; continuing"
            );
        }
    });
}

/// Convenience wrapper for merge-guard capture, where only the current working
/// directory is available.
pub fn spawn_mempal_ingest_for_project(project_root: &Path, room: &'static str, input_path: &Path) {
    if let Some(config) = load_effective_memory_config(project_root) {
        spawn_mempal_ingest(&config, room, input_path);
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

    let mut child = command.spawn()?;
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
    use std::fs;
    use std::io::Write as _;

    #[test]
    fn run_mempal_ingest_passes_expected_args() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let log_path = temp.path().join("args.log");
        let script_path = temp.path().join("mempal-fake.sh");
        let mut script = fs::File::create(&script_path).expect("create fake mempal");
        writeln!(
            script,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n",
            log_path.display()
        )
        .expect("write fake mempal");
        drop(script);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&script_path).expect("metadata").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&script_path, perms).expect("chmod");
        }

        let input_dir = temp.path().join("session-output");
        fs::create_dir(&input_dir).expect("create input dir");
        run_mempal_ingest(
            &script_path,
            "csa-session",
            &input_dir,
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
}
