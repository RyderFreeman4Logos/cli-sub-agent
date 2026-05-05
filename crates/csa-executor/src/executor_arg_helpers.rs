use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use tokio::process::Command;

const GEMINI_INCLUDE_DIRS_ENV_KEYS: &[&str] = &[
    "CSA_GEMINI_INCLUDE_DIRECTORIES",
    "GEMINI_INCLUDE_DIRECTORIES",
];
const GEMINI_CONTEXT_SYMLINK_FILES: &[&str] = &["GEMINI.md", "AGENTS.md", "CLAUDE.md"];

/// `"default"` means "delegate model routing to gemini-cli", so omit `-m`.
pub(crate) fn effective_gemini_model_override(model_override: &Option<String>) -> Option<&str> {
    model_override
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.eq_ignore_ascii_case("default"))
        .filter(|model| !model.is_empty())
}

pub(crate) fn codex_notify_suppression_args(env: &HashMap<String, String>) -> Vec<String> {
    match env.get("CSA_SUPPRESS_NOTIFY").map(String::as_str) {
        Some("1") => vec!["-c".to_string(), "notify=[]".to_string()],
        _ => Vec::new(),
    }
}

pub(crate) fn gemini_include_directories(
    extra_env: Option<&HashMap<String, String>>,
    prompt: &str,
    execution_dir: Option<&Path>,
) -> Vec<String> {
    let mut directories = Vec::new();

    if let Some(dir) = execution_dir {
        push_unique_directory_string(&mut directories, dir.to_string_lossy().as_ref());
        append_gemini_context_symlink_target_dirs(&mut directories, dir);
    }

    if let Some(env) = extra_env {
        let raw = GEMINI_INCLUDE_DIRS_ENV_KEYS
            .iter()
            .find_map(|key| env.get(*key))
            .map(String::as_str)
            .unwrap_or_default();

        for entry in raw.split([',', '\n']) {
            let directory = entry.trim();
            if directory.is_empty() {
                continue;
            }
            if Path::new(directory).is_relative() {
                if let Some(base) = execution_dir {
                    let combined = base.join(directory);
                    push_unique_directory_string(
                        &mut directories,
                        combined.to_string_lossy().as_ref(),
                    );
                } else {
                    push_unique_directory_string(&mut directories, directory);
                }
                continue;
            }
            push_unique_directory_string(&mut directories, directory);
        }
    }

    for directory in gemini_prompt_directories(prompt) {
        push_unique_directory_string(&mut directories, &directory);
    }

    directories
}

fn append_gemini_context_symlink_target_dirs(directories: &mut Vec<String>, execution_dir: &Path) {
    let canonical_execution_dir =
        fs::canonicalize(execution_dir).unwrap_or_else(|_| execution_dir.to_path_buf());

    for file_name in GEMINI_CONTEXT_SYMLINK_FILES {
        let candidate = execution_dir.join(file_name);
        let Ok(metadata) = fs::symlink_metadata(&candidate) else {
            continue;
        };
        if !metadata.file_type().is_symlink() {
            continue;
        }

        let Ok(resolved) = fs::canonicalize(&candidate) else {
            continue;
        };
        let Some(parent) = resolved.parent() else {
            continue;
        };
        if parent.starts_with(&canonical_execution_dir) {
            continue;
        }

        push_unique_directory_string(directories, parent.to_string_lossy().as_ref());
    }
}

pub(crate) fn append_gemini_include_directories_args(cmd: &mut Command, directories: &[String]) {
    for directory in directories {
        cmd.arg("--include-directories").arg(directory);
    }
}

fn gemini_prompt_directories(prompt: &str) -> Vec<String> {
    let mut directories = Vec::new();
    let tokens: Vec<String> = prompt
        .split_whitespace()
        .map(trim_prompt_path_token)
        .filter(|token| !token.is_empty())
        .collect();

    let mut index = 0;
    while index < tokens.len() {
        if !tokens[index].starts_with('/') || tokens[index].contains("://") {
            index += 1;
            continue;
        }

        let mut candidate = String::new();
        let mut best_match: Option<(usize, PathBuf)> = None;
        for (end, token) in tokens.iter().enumerate().skip(index) {
            if end > index {
                if token.starts_with('/') {
                    break;
                }
                candidate.push(' ');
            }
            candidate.push_str(token);

            let path = Path::new(&candidate);
            if path.is_absolute() && path.exists() {
                best_match = Some((end, path.to_path_buf()));
            }
        }

        if let Some((end, path)) = best_match {
            let dir = if path.is_dir() {
                path
            } else if let Some(parent) = path.parent() {
                parent.to_path_buf()
            } else {
                index += 1;
                continue;
            };

            let normalized = fs::canonicalize(&dir).unwrap_or(dir);
            push_unique_directory_string(&mut directories, normalized.to_string_lossy().as_ref());
            index = end + 1;
        } else {
            index += 1;
        }
    }
    directories
}

fn trim_prompt_path_token(raw: &str) -> String {
    raw.trim_matches(|c: char| {
        matches!(
            c,
            '"' | '\''
                | '`'
                | ','
                | ';'
                | ':'
                | '.'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '<'
                | '>'
        )
    })
    .to_string()
}

fn push_unique_directory_string(directories: &mut Vec<String>, directory: &str) {
    push_unique_directory_string_with_exists(directories, directory, |path| path.try_exists());
}

fn push_unique_directory_string_with_exists(
    directories: &mut Vec<String>,
    directory: &str,
    try_exists: impl FnOnce(&Path) -> io::Result<bool>,
) {
    let trimmed = directory.trim();
    if trimmed.is_empty() {
        return;
    }

    let path = Path::new(trimmed);
    // Never inject filesystem root as an include directory. In practice this
    // can explode workspace scan scope and trigger avoidable memory pressure.
    if is_filesystem_root(path) {
        return;
    }
    if directories.iter().any(|existing| existing == trimmed) {
        return;
    }

    match try_exists(path) {
        Ok(true) => {}
        Ok(false) => {
            tracing::warn!(
                directory = %trimmed,
                "Skipping Gemini include directory because it does not exist"
            );
            return;
        }
        Err(error) => {
            tracing::warn!(
                directory = %trimmed,
                error = %error,
                "Skipping Gemini include directory because existence could not be checked"
            );
            return;
        }
    }

    directories.push(trimmed.to_string());
}

fn is_filesystem_root(path: &Path) -> bool {
    path.is_absolute() && path.parent().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

    #[derive(Clone)]
    struct SharedBufferWriter {
        buf: Arc<Mutex<Vec<u8>>>,
    }

    impl io::Write for SharedBufferWriter {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            let mut guard = self.buf.lock().expect("buffer lock poisoned");
            guard.extend_from_slice(data);
            Ok(data.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[derive(Clone)]
    struct SharedMakeWriter {
        buf: Arc<Mutex<Vec<u8>>>,
    }

    impl<'a> MakeWriter<'a> for SharedMakeWriter {
        type Writer = SharedBufferWriter;

        fn make_writer(&'a self) -> Self::Writer {
            SharedBufferWriter {
                buf: Arc::clone(&self.buf),
            }
        }
    }

    fn capture_warn_logs(action: impl FnOnce()) -> String {
        let log_buf = Arc::new(Mutex::new(Vec::new()));
        let make_writer = SharedMakeWriter {
            buf: Arc::clone(&log_buf),
        };
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .without_time()
            .with_target(false)
            .with_writer(make_writer)
            .finish();

        tracing::subscriber::with_default(subscriber, action);

        String::from_utf8(log_buf.lock().expect("buffer lock poisoned").clone())
            .expect("logs should be valid UTF-8")
    }

    #[test]
    fn push_unique_directory_string_skips_nonexistent_path_with_warning() {
        let temp = tempfile::tempdir().expect("tempdir");
        let missing = temp.path().join("missing");
        let missing = missing.to_string_lossy().to_string();
        let mut directories = Vec::new();

        let logs = capture_warn_logs(|| {
            push_unique_directory_string(&mut directories, &missing);
        });

        assert!(directories.is_empty());
        assert!(logs.contains("Skipping Gemini include directory because it does not exist"));
        assert!(logs.contains(&missing));
    }

    #[test]
    fn push_unique_directory_string_skips_permission_error_with_warning() {
        let mut directories = Vec::new();

        let logs = capture_warn_logs(|| {
            push_unique_directory_string_with_exists(&mut directories, "/inaccessible", |_| {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "permission denied",
                ))
            });
        });

        assert!(directories.is_empty());
        assert!(
            logs.contains(
                "Skipping Gemini include directory because existence could not be checked"
            )
        );
        assert!(logs.contains("permission denied"));
    }

    #[test]
    fn push_unique_directory_string_adds_existing_path_normally() {
        let temp = tempfile::tempdir().expect("tempdir");
        let existing = temp.path().to_string_lossy().to_string();
        let mut directories = Vec::new();

        let logs = capture_warn_logs(|| {
            push_unique_directory_string(&mut directories, &existing);
        });

        assert_eq!(directories, vec![existing]);
        assert!(!logs.contains("Skipping Gemini include directory"));
    }
}
