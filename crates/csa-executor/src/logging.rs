//! Session-isolated logging setup.

use anyhow::Result;
use chrono::Utc;
use std::path::Path;

/// Create a session-specific log writer.
///
/// Returns a non-blocking writer and a worker guard that must be kept alive
/// for the duration of logging. The caller (main.rs) should configure the
/// tracing subscriber with the returned writer.
///
/// Log files are created in `{session_dir}/logs/run-{timestamp}.log`.
pub fn create_session_log_writer(
    session_dir: &Path,
) -> Result<(
    tracing_appender::non_blocking::NonBlocking,
    tracing_appender::non_blocking::WorkerGuard,
)> {
    let log_dir = session_dir.join("logs");
    std::fs::create_dir_all(&log_dir)?;

    let file_name = format!("run-{}.log", Utc::now().format("%Y%m%d-%H%M%S"));
    let file_appender = tracing_appender::rolling::never(&log_dir, file_name);
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    Ok((non_blocking, guard))
}
