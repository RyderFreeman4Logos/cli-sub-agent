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

            // #1679: never auto-promote a broad system directory found in the
            // prompt. gemini-cli scans every include directory recursively, and
            // two path classes break that scan into a zero-turn, idle-killed
            // session: broad temp roots (world-unreadable subtrees like
            // /var/tmp/systemd-private-* -> repeated EACCES stalls) and virtual
            // filesystems (/proc, /sys, /dev, /run -> blocking devices, FIFOs,
            // infinite recursion). is_broad_system_dir encodes the per-class
            // match (bare temp root vs whole virtual-FS subtree).
            //
            // Screen BOTH the raw prompt-derived path and its canonicalized
            // form, skipping promotion if EITHER matches:
            //   - the raw check catches a literal `/tmp` / `/var/tmp` token even
            //     when that root is itself a symlink whose target is outside the
            //     denylist (e.g. macOS, where /tmp -> /private/tmp);
            //   - the canonical check catches `..` variants (e.g. `/tmp/../tmp`)
            //     and symlinks pointing *into* a denylist root, including -- via
            //     the /private/* aliases -- the macOS canonical form of
            //     `/tmp/../tmp` (which is /private/tmp).
            // The union is strictly more conservative than either alone: a real
            // subdirectory matches neither form and is still included. It screens
            // against a denylist, not by resolving every symlink chain. The
            // canonicalized form is what gets pushed; paths that fail to
            // canonicalize (TOCTOU removal after the existence check above) fall
            // back to the raw token.
            let normalized = fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
            if !is_broad_system_dir(&dir) && !is_broad_system_dir(&normalized) {
                push_unique_directory_string(
                    &mut directories,
                    normalized.to_string_lossy().as_ref(),
                );
            }
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

/// Broad system *temp* roots, matched at the **bare root only** (`==`) so that
/// scratch subdirectories such as `/tmp/work/file` stay includable. The
/// recursive-scan EACCES stall that bars promoting the bare roots is documented
/// at the call site (`gemini_prompt_directories`, #1679).
///
/// The macOS canonical aliases (`/private/tmp`, `/private/var/tmp`): there
/// `/tmp` and `/var/tmp` symlink into `/private`, so `/tmp/../tmp` canonicalizes
/// to `/private/tmp` and `/var/tmp/../tmp` to `/private/var/tmp`; listing them
/// lets the canonical-form check catch those variants. ONLY the two temp-root
/// aliases are added -- never `/private/var` or any broader parent.
const GEMINI_BROAD_SYSTEM_DIRS: &[&str] = &["/tmp", "/var/tmp", "/private/tmp", "/private/var/tmp"];

/// Virtual / pseudo filesystems whose **entire subtree** is excluded. Unlike the
/// temp roots above, any descendant may be a blocking character device, a named
/// pipe (FIFO), or an unbounded self-referential tree (e.g. `/proc/self/fd`,
/// `/dev/stdin`, `/sys/kernel`), so promoting one would let gemini-cli's
/// recursive scan block or loop forever -- the #1679 freeze class, reachable via
/// untrusted issue-body paths under `csa plan run --issue N`. Matched with
/// `Path::starts_with`, which is **component-wise**, not a string prefix: `/dev`
/// denies `/dev/stdin` but NOT `/devfoo`, `/run` denies `/run/foo` but NOT
/// `/runtime` -- only true sub-paths, never prefix-sharing siblings.
const GEMINI_BROAD_SYSTEM_SUBTREES: &[&str] = &["/run", "/proc", "/sys", "/dev"];

/// True if `path` must be excluded from Gemini auto-include promotion: either it
/// is exactly a broad temp root (`==`, so subdirectories remain includable) or
/// it lies within a virtual/system-FS subtree (component-wise `Path::starts_with`).
fn is_broad_system_dir(path: &Path) -> bool {
    GEMINI_BROAD_SYSTEM_DIRS
        .iter()
        .any(|broad| path == Path::new(broad))
        || GEMINI_BROAD_SYSTEM_SUBTREES
            .iter()
            .any(|subtree| path.starts_with(Path::new(subtree)))
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

    #[test]
    fn is_broad_system_dir_matches_bare_roots_only() {
        for broad in [
            "/tmp",
            "/var/tmp",
            "/run",
            "/proc",
            "/sys",
            "/dev",
            // macOS canonical aliases of the temp roots (#1679 round-2 follow-up).
            "/private/tmp",
            "/private/var/tmp",
        ] {
            assert!(
                is_broad_system_dir(Path::new(broad)),
                "{broad} must be treated as a broad system dir"
            );
        }
        // A trailing slash is the same directory (component-wise comparison).
        assert!(is_broad_system_dir(Path::new("/var/tmp/")));
        // Specific subdirectories are NOT broad and remain includable.
        assert!(!is_broad_system_dir(Path::new("/tmp/work")));
        assert!(!is_broad_system_dir(Path::new("/var/tmp/foo/bar")));
        assert!(!is_broad_system_dir(Path::new("/home/user/project")));
    }

    #[test]
    fn is_broad_system_dir_excludes_virtual_fs_subtrees() {
        // Virtual/system filesystems are excluded as WHOLE subtrees (#1679
        // security follow-up). Asserted at the is_broad_system_dir level so it
        // needs neither the paths to exist nor canonicalize to succeed.
        for excluded in [
            "/proc",
            "/sys",
            "/dev",
            "/run",
            "/proc/self/fd",
            "/dev/stdin",
            "/sys/kernel",
            "/run/foo",
        ] {
            assert!(
                is_broad_system_dir(Path::new(excluded)),
                "{excluded} must be excluded as a virtual/system FS subtree"
            );
        }
    }

    #[test]
    fn is_broad_system_dir_subtree_match_is_component_wise() {
        // Path::starts_with is component-wise, NOT a string prefix: a sibling
        // whose name merely begins with a denied root must stay includable.
        assert!(!is_broad_system_dir(Path::new("/devfoo")));
        assert!(!is_broad_system_dir(Path::new("/runtime")));
        assert!(!is_broad_system_dir(Path::new("/sysadmin")));
        assert!(!is_broad_system_dir(Path::new("/processes")));
    }

    #[cfg(unix)]
    #[test]
    fn gemini_prompt_directories_excludes_existing_virtual_fs_path() {
        // End-to-end: a real virtual-FS sub-path in the prompt must not be
        // promoted. Gated on existence for portability (minimal containers).
        if !Path::new("/proc/self").exists() {
            return;
        }
        let prompt = "Inspect /proc/self for the running process.";
        let dirs = gemini_prompt_directories(prompt);
        assert!(
            !dirs.iter().any(|d| Path::new(d).starts_with("/proc")),
            "no /proc subtree path may be promoted, got: {dirs:?}"
        );
    }

    #[test]
    fn gemini_prompt_directories_excludes_broad_temp_roots_keeps_subdirs() {
        // A real nested file under the system temp root: its parent is a
        // specific subdirectory and therefore a legitimate include candidate.
        let temp = tempfile::tempdir().expect("tempdir");
        let work_dir = temp.path().join("work");
        fs::create_dir_all(&work_dir).expect("create work dir");
        let session_file = work_dir.join("session-file.txt");
        fs::write(&session_file, b"x").expect("write session file");
        let canonical_work = fs::canonicalize(&work_dir).expect("canonicalize work dir");

        // Mirrors the CSA atomic-commit-discipline preamble that injects bare
        // `/tmp` and `/var/tmp` tokens into the prompt (#1679).
        let prompt = format!(
            "Persist scratch under /tmp and /var/tmp; read {} for context.",
            session_file.display()
        );

        let dirs = gemini_prompt_directories(&prompt);

        assert!(
            !dirs.iter().any(|d| d == "/tmp"),
            "bare /tmp must be excluded, got: {dirs:?}"
        );
        assert!(
            !dirs.iter().any(|d| d == "/var/tmp"),
            "bare /var/tmp must be excluded, got: {dirs:?}"
        );
        assert!(
            dirs.iter().any(|d| Path::new(d) == canonical_work),
            "specific subdir {} must still be included, got: {dirs:?}",
            canonical_work.display()
        );
    }

    #[test]
    fn gemini_prompt_directories_rejects_dotdot_broad_dir_variants() {
        // #1679 hardening: a path token whose raw form is not literally a
        // broad root but canonicalizes to one (e.g. `/tmp/../tmp`,
        // `/var/tmp/../tmp`) MUST NOT slip into the include set, because the
        // canonicalized form is what gets pushed. A real subdirectory under
        // the temp root MUST still be included.
        let temp = tempfile::tempdir().expect("tempdir");
        let work_dir = temp.path().join("work");
        fs::create_dir_all(&work_dir).expect("create work dir");
        let canonical_work = fs::canonicalize(&work_dir).expect("canonicalize work dir");

        let prompt = format!(
            "Scan /tmp/../tmp and /var/tmp/../tmp but read {} for context.",
            work_dir.display()
        );

        let dirs = gemini_prompt_directories(&prompt);

        assert!(
            !dirs.iter().any(|d| d == "/tmp"),
            "`/tmp/../tmp` must not canonicalize into an included bare /tmp, got: {dirs:?}"
        );
        assert!(
            !dirs.iter().any(|d| d == "/var/tmp"),
            "`/var/tmp/../tmp` must not canonicalize into an included bare /var/tmp, got: {dirs:?}"
        );
        assert!(
            dirs.iter().any(|d| Path::new(d) == canonical_work),
            "real subdir {} must still be included, got: {dirs:?}",
            canonical_work.display()
        );
    }

    #[cfg(unix)]
    #[test]
    fn gemini_prompt_directories_rejects_symlink_into_broad_root() {
        // #1679 hardening (symlinked-path direction): a prompt token whose raw
        // form is NOT literally a broad root but whose canonical form resolves
        // INTO one (here a symlink pointing at /tmp) MUST NOT be promoted. This
        // exercises the canonical-form arm of the union screen.
        //
        // The mirror case -- a denylist root that is *itself* a symlink to a
        // target outside the denylist (macOS: `/tmp` -> `/private/tmp`) -- is
        // not portably constructible on the Linux CI host (we cannot repoint
        // `/tmp`). It is instead covered by the raw-form arm of the screen plus
        // the `/private/*` aliases in GEMINI_BROAD_SYSTEM_DIRS, not by this test.
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let sneaky = temp.path().join("sneaky");
        symlink("/tmp", &sneaky).expect("create symlink to /tmp");

        let prompt = format!("Scan {} for context.", sneaky.display());
        let dirs = gemini_prompt_directories(&prompt);

        assert!(
            !dirs.iter().any(|d| d == "/tmp"),
            "a symlink resolving to /tmp must not be promoted, got: {dirs:?}"
        );
    }

    #[test]
    fn gemini_include_directories_keeps_project_root_and_drops_prompt_broad_dirs() {
        let project = tempfile::tempdir().expect("project tempdir");
        let project_path = fs::canonicalize(project.path()).expect("canonicalize project");

        let prompt = "Work in /tmp and /var/tmp but commit the project.";
        let dirs = gemini_include_directories(None, prompt, Some(project.path()));

        assert!(
            dirs.iter().any(|d| Path::new(d) == project_path),
            "project root must still be included, got: {dirs:?}"
        );
        assert!(
            !dirs.iter().any(|d| d == "/tmp" || d == "/var/tmp"),
            "broad prompt dirs must be excluded, got: {dirs:?}"
        );
    }
}
