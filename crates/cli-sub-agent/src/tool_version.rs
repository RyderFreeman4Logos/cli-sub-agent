use std::process::Stdio;
use std::time::Duration;

use csa_executor::Executor;
use tokio::io::AsyncReadExt;

const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(4);

pub(crate) async fn detect_tool_version(executor: &Executor) -> Option<String> {
    if matches!(executor, Executor::OpenaiCompat { .. }) {
        tracing::debug!(tool = %executor.tool_name(), "Skipping version probe for HTTP-only tool");
        return None;
    }

    let binary = executor.runtime_binary_name();
    let version = probe_binary_version(binary).await;
    if version.is_none() {
        tracing::debug!(
            tool = %executor.tool_name(),
            binary,
            "Failed to detect tool version"
        );
    }
    version
}

async fn probe_binary_version(binary: &str) -> Option<String> {
    probe_binary_version_with_timeout(binary, VERSION_PROBE_TIMEOUT).await
}

async fn probe_binary_version_with_timeout(binary: &str, timeout: Duration) -> Option<String> {
    let mut cmd = tokio::process::Command::new(binary);
    cmd.arg("--version");
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    cmd.process_group(0);

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            tracing::debug!(binary, error = %err, "Version probe command failed");
            return None;
        }
    };

    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take()?;
    let output = match tokio::time::timeout(timeout, async {
        let mut stdout_buf = Vec::new();
        let mut stderr_buf = Vec::new();
        let (stdout_result, stderr_result, wait_result) = tokio::join!(
            stdout.read_to_end(&mut stdout_buf),
            stderr.read_to_end(&mut stderr_buf),
            child.wait()
        );
        let status = wait_result?;
        stdout_result?;
        stderr_result?;
        Ok::<_, std::io::Error>((status, stdout_buf, stderr_buf))
    })
    .await
    {
        Ok(Ok(output)) => output,
        Ok(Err(err)) => {
            tracing::debug!(binary, error = %err, "Version probe command failed");
            return None;
        }
        Err(_) => {
            let kill_result = child.start_kill();
            let wait_result = child.wait().await;
            tracing::debug!(
                binary,
                timeout_secs = timeout.as_secs_f64(),
                kill_error = kill_result.as_ref().err().map(ToString::to_string),
                wait_error = wait_result.as_ref().err().map(ToString::to_string),
                "Version probe timed out; killed child process"
            );
            return None;
        }
    };

    let (_status, stdout_buf, stderr_buf) = output;
    let stdout = String::from_utf8_lossy(&stdout_buf);
    let stderr = String::from_utf8_lossy(&stderr_buf);
    parse_first_numeric_version_token(&format!("{stdout}\n{stderr}"))
}

fn parse_first_numeric_version_token(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut idx = 0;

    while idx < bytes.len() {
        let ch = text[idx..].chars().next()?;
        let ch_len = ch.len_utf8();
        let next_is_digit = text[idx + ch_len..]
            .chars()
            .next()
            .is_some_and(|next| next.is_ascii_digit());
        if ch.is_ascii_digit() || ((ch == 'v' || ch == 'V') && next_is_digit) {
            let start = if ch.is_ascii_digit() {
                idx
            } else {
                idx + ch_len
            };
            let mut end = start;
            while end < bytes.len() {
                let current = text[end..].chars().next()?;
                if current.is_ascii_alphanumeric() || matches!(current, '.' | '-' | '_' | '+') {
                    end += current.len_utf8();
                } else {
                    break;
                }
            }
            if end > start {
                let candidate = &text[start..end];
                if candidate.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                    return Some(candidate.to_string());
                }
            }
        }
        idx += ch_len;
    }

    None
}

#[cfg(test)]
mod tests {
    use super::{parse_first_numeric_version_token, probe_binary_version_with_timeout};
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::time::Duration;

    #[test]
    fn tool_version_probe_parses_known_format() {
        assert_eq!(
            parse_first_numeric_version_token("claude-code 1.2.3\n"),
            Some("1.2.3".to_string())
        );
        assert_eq!(
            parse_first_numeric_version_token("codex version 0.18.2 (abc123)\n"),
            Some("0.18.2".to_string())
        );
        assert_eq!(
            parse_first_numeric_version_token("gemini-cli v1.0.0-beta\n"),
            Some("1.0.0-beta".to_string())
        );
    }

    #[tokio::test]
    async fn tool_version_probe_returns_none_on_missing_binary() {
        assert!(
            probe_binary_version_with_timeout(
                "/definitely/missing/csa-tool-version-probe",
                Duration::from_millis(50),
            )
            .await
            .is_none()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tool_version_probe_kills_child_on_timeout() {
        let temp = tempfile::tempdir().expect("tempdir");
        let script_path = temp.path().join("fake-tool");
        let pid_path = temp.path().join("fake-tool.pid");
        std::fs::write(
            &script_path,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  echo $$ > \"{}\"\n  sleep 30\nfi\n",
                pid_path.display()
            ),
        )
        .expect("write script");
        let mut perms = std::fs::metadata(&script_path)
            .expect("metadata")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms).expect("chmod");

        let version = probe_binary_version_with_timeout(
            script_path.to_str().expect("utf-8 path"),
            Duration::from_millis(50),
        )
        .await;
        assert!(version.is_none());

        let pid = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let Ok(pid) = std::fs::read_to_string(&pid_path) {
                    break pid;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("pid file should be written")
        .trim()
        .parse::<u32>()
        .expect("pid should be numeric");

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if !process_exists(pid) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("timed out child should be reaped");
    }

    #[cfg(unix)]
    fn process_exists(pid: u32) -> bool {
        Path::new("/proc").join(pid.to_string()).exists()
    }
}
