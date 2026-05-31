use std::process::ExitStatus;

use crate::error::AcpError;

pub(crate) fn process_exited_error(status: ExitStatus, stderr: String) -> AcpError {
    AcpError::ProcessExited {
        code: status.code().unwrap_or(-1),
        signal: exit_signal(status),
        stderr,
    }
}

#[cfg(unix)]
fn exit_signal(status: ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;

    status.signal()
}

#[cfg(not(unix))]
fn exit_signal(_status: ExitStatus) -> Option<i32> {
    None
}

/// Format captured stderr for inclusion in error messages.
///
/// Returns an empty string when no stderr was captured, or
/// `"; stderr: <content>"` otherwise.
pub(crate) fn format_stderr(stderr: &str) -> String {
    let trimmed = stderr.trim();
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("; stderr: {trimmed}")
    }
}
