use tracing::warn;

pub(crate) struct ProcessExitStatus {
    pub(crate) code: i32,
    pub(crate) signal: Option<i32>,
    pub(crate) note: Option<String>,
}

pub(crate) fn process_exit_status(status: std::process::ExitStatus) -> ProcessExitStatus {
    let signal = exit_status_signal(status);
    let code = status.code().unwrap_or_else(|| {
        if let Some(signal) = signal {
            let code = 128 + signal;
            warn!(signal, exit_code = code, "Process terminated by signal");
            code
        } else {
            warn!("Process terminated without exit code or signal, using exit code 1");
            1
        }
    });
    ProcessExitStatus {
        code,
        signal,
        note: signal.map(format_signal_exit_note),
    }
}

pub(crate) fn append_signal_exit_note(stderr_output: &mut String, note: &str) {
    if !stderr_output.is_empty() && !stderr_output.ends_with('\n') {
        stderr_output.push('\n');
    }
    stderr_output.push_str(note);
    stderr_output.push('\n');
}

#[cfg(unix)]
fn exit_status_signal(status: std::process::ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.signal()
}

#[cfg(not(unix))]
fn exit_status_signal(_status: std::process::ExitStatus) -> Option<i32> {
    None
}

fn format_signal_exit_note(signal: i32) -> String {
    format!(
        "process killed by signal {signal} ({})",
        signal_name(signal)
    )
}

fn signal_name(signal: i32) -> &'static str {
    match signal {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        6 => "SIGABRT",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        15 => "SIGTERM",
        _ => "UNKNOWN",
    }
}
