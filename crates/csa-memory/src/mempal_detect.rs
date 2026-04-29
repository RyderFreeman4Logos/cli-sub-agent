use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const DETECTION_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MempalInfo {
    pub binary_path: String,
    pub version: Option<String>,
}

static DETECTION_RESULT: OnceLock<Option<MempalInfo>> = OnceLock::new();

pub fn detect_mempal() -> Option<&'static MempalInfo> {
    DETECTION_RESULT.get_or_init(detect_mempal_inner).as_ref()
}

fn detect_mempal_inner() -> Option<MempalInfo> {
    detect_mempal_binary("mempal")
}

fn detect_mempal_binary(binary_name: &str) -> Option<MempalInfo> {
    let which_output = run_command_with_timeout(
        {
            let mut command = Command::new("which");
            command.arg(binary_name);
            command
        },
        DETECTION_TIMEOUT,
    )?;

    if !which_output.status.success() {
        return None;
    }

    let binary_path = String::from_utf8_lossy(&which_output.stdout)
        .lines()
        .next()
        .map(str::trim)
        .filter(|path| !path.is_empty())?
        .to_string();

    let version = run_command_with_timeout(
        {
            let mut command = Command::new(&binary_path);
            command.arg("--version");
            command
        },
        DETECTION_TIMEOUT,
    )
    .filter(|output| output.status.success())
    .and_then(|output| {
        first_nonempty_line(&output.stdout).or_else(|| first_nonempty_line(&output.stderr))
    });

    Some(MempalInfo {
        binary_path,
        version,
    })
}

fn first_nonempty_line(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

fn run_command_with_timeout(mut command: Command, timeout: Duration) -> Option<Output> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => return child.wait_with_output().ok(),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::detect_mempal_binary;

    #[test]
    fn detect_mempal_returns_none_when_binary_is_missing() {
        let missing_binary = format!("csa-missing-mempal-{}", ulid::Ulid::new());
        assert_eq!(detect_mempal_binary(&missing_binary), None);
    }
}
