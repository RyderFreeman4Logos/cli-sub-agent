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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_create_session_log_writer_creates_file_in_tempdir() {
        let tmp = tempfile::tempdir().expect("Failed to create tempdir");
        let session_dir = tmp.path();

        let (writer, _guard) =
            create_session_log_writer(session_dir).expect("Should create log writer successfully");

        // Verify the logs directory was created
        let log_dir = session_dir.join("logs");
        assert!(log_dir.exists(), "logs/ directory should exist");
        assert!(log_dir.is_dir(), "logs/ should be a directory");

        // Verify at least one log file was created
        let entries: Vec<_> = std::fs::read_dir(&log_dir)
            .expect("Should read log dir")
            .collect();
        assert_eq!(entries.len(), 1, "Exactly one log file should be created");

        let log_file = entries[0].as_ref().expect("Should read entry");
        let file_name = log_file.file_name();
        let name_str = file_name.to_string_lossy();
        assert!(
            name_str.starts_with("run-"),
            "Log file should start with 'run-': got {name_str}"
        );
        assert!(
            name_str.ends_with(".log"),
            "Log file should end with '.log': got {name_str}"
        );

        // Writer should be usable (non-blocking wrapper around the file)
        drop(writer);
    }

    #[test]
    fn test_create_session_log_writer_creates_nested_logs_dir() {
        let tmp = tempfile::tempdir().expect("Failed to create tempdir");
        let session_dir = tmp.path().join("deep").join("nested").join("session");
        // The session_dir itself doesn't exist yet, but create_session_log_writer
        // calls create_dir_all on session_dir/logs, which should create all parents.

        let result = create_session_log_writer(&session_dir);
        assert!(
            result.is_ok(),
            "Should create nested dirs: {:?}",
            result.err()
        );

        let log_dir = session_dir.join("logs");
        assert!(log_dir.exists(), "Nested logs/ directory should exist");
    }

    #[test]
    fn test_create_session_log_writer_error_on_invalid_path() {
        // Use a path that cannot be created (null bytes in path on Unix)
        let bad_path = PathBuf::from("/dev/null/impossible/path");
        let result = create_session_log_writer(&bad_path);
        assert!(result.is_err(), "Should fail with invalid directory path");
    }

    #[test]
    fn test_log_file_name_contains_timestamp() {
        let tmp = tempfile::tempdir().expect("Failed to create tempdir");
        let (_writer, _guard) =
            create_session_log_writer(tmp.path()).expect("Should create log writer");

        let log_dir = tmp.path().join("logs");
        let entry = std::fs::read_dir(&log_dir)
            .expect("Should read log dir")
            .next()
            .expect("Should have one entry")
            .expect("Should read entry");

        let name = entry.file_name().to_string_lossy().to_string();
        // Format: run-YYYYMMDD-HHMMSS.log
        // Check that the timestamp portion has correct format (8 digits, dash, 6 digits)
        let stem = name.strip_prefix("run-").expect("Should start with run-");
        let stem = stem.strip_suffix(".log").expect("Should end with .log");
        assert_eq!(
            stem.len(),
            15,
            "Timestamp should be 15 chars (YYYYMMDD-HHMMSS): got '{stem}'"
        );
        assert_eq!(
            stem.chars().nth(8),
            Some('-'),
            "Should have dash at position 8: got '{stem}'"
        );
    }
}
