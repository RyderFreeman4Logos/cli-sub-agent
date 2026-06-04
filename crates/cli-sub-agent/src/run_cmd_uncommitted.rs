//! Writer-session dirty-worktree signal for `csa run`.

use std::path::Path;
use std::process::Command;

use tracing::warn;

const MAX_UNCOMMITTED_FILES: usize = 20;
const REQUIRE_COMMIT_REASON: &str =
    "writer session ended with uncommitted changes (--require-commit set)";

pub(crate) fn is_writer_session(sa_mode: bool, task_type: Option<&str>) -> bool {
    !sa_mode && matches!(task_type, Some("run"))
}

pub(crate) fn effective_writer_must_commit(
    cli_require_commit: bool,
    config: Option<&csa_config::ProjectConfig>,
) -> bool {
    cli_require_commit || config.is_some_and(|cfg| cfg.run.writer_must_commit)
}

pub(crate) fn summarize_uncommitted_changes(
    porcelain: &str,
    numstat: &str,
) -> Option<csa_session::UncommittedChanges> {
    let paths = changed_paths_from_porcelain(porcelain);
    if paths.is_empty() {
        return None;
    }

    let (insertions, deletions) = parse_numstat_totals(numstat);
    let file_count = paths.len();
    let files = paths
        .into_iter()
        .take(MAX_UNCOMMITTED_FILES)
        .collect::<Vec<_>>();
    let truncated = file_count.saturating_sub(files.len());

    Some(csa_session::UncommittedChanges {
        file_count,
        insertions,
        deletions,
        files,
        truncated,
    })
}

pub(crate) fn record_writer_uncommitted_changes(
    project_root: &Path,
    session_id: Option<&str>,
    result: &mut csa_process::ExecutionResult,
    sa_mode: bool,
    require_commit: bool,
) {
    if !is_writer_session(sa_mode, Some("run")) {
        return;
    }
    let Some(session_id) = session_id else {
        return;
    };
    let Some(changes) = collect_uncommitted_changes(project_root) else {
        return;
    };

    match csa_session::load_result(project_root, session_id) {
        Ok(Some(mut session_result)) => {
            apply_uncommitted_changes_to_result(
                &mut session_result,
                changes.clone(),
                require_commit,
            );
            if let Err(err) = csa_session::save_result(project_root, session_id, &session_result) {
                warn!(
                    session = %session_id,
                    error = %err,
                    "Failed to persist writer uncommitted-changes signal"
                );
            }
        }
        Ok(None) => {
            warn!(
                session = %session_id,
                "No result.toml to annotate with writer uncommitted-changes signal"
            );
        }
        Err(err) => {
            warn!(
                session = %session_id,
                error = %err,
                "Failed to load result.toml for writer uncommitted-changes signal"
            );
        }
    }

    if require_commit {
        result.mark_gate_failure("writer-uncommitted");
        result.summary = REQUIRE_COMMIT_REASON.to_string();
        if !result.stderr_output.is_empty() && !result.stderr_output.ends_with('\n') {
            result.stderr_output.push('\n');
        }
        result.stderr_output.push_str(REQUIRE_COMMIT_REASON);
        result.stderr_output.push('\n');
    }
}

pub(crate) fn record_run_dirty(
    project_root: &Path,
    session_id: Option<&str>,
    result: &mut csa_process::ExecutionResult,
    cli_require_commit: bool,
    config: Option<&csa_config::ProjectConfig>,
) {
    let sa_mode = std::env::var(crate::pipeline::prompt_guard::PROMPT_GUARD_CALLER_INJECTION_ENV)
        .ok()
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "true" | "1"))
        .unwrap_or(false);
    record_writer_uncommitted_changes(
        project_root,
        session_id,
        result,
        sa_mode,
        effective_writer_must_commit(cli_require_commit, config),
    );
}

pub(crate) fn apply_uncommitted_changes_to_result(
    result: &mut csa_session::SessionResult,
    changes: csa_session::UncommittedChanges,
    require_commit: bool,
) {
    result.uncommitted_changes = Some(changes);
    if require_commit {
        result.exit_code = 1;
        result.status = csa_session::SessionResult::status_from_exit_code(1);
        result.summary = REQUIRE_COMMIT_REASON.to_string();
    }
}

pub(crate) fn format_uncommitted_warning(changes: &csa_session::UncommittedChanges) -> String {
    format!(
        "⚠ writer session ended with {} uncommitted files (+{}/-{}) — work NOT committed",
        changes.file_count, changes.insertions, changes.deletions
    )
}

/// Total working-tree change size, in lines, used by the review-aware writer
/// guard (#1842) to size-gate prompt injection for resume sessions: tracked diff
/// lines (insertions + deletions vs `HEAD`) PLUS the line count of untracked,
/// non-ignored files.
///
/// Untracked files never appear in `git diff HEAD`, so a substantial change made
/// entirely of new (never-staged) files would otherwise measure as zero changed
/// lines and be mis-classified as trivial — handing the writer the *brief* guard
/// exactly when the full per-dimension checklist matters most. Counting untracked
/// non-ignored lines closes that gap. `.gitignore` is honored (build artifacts do
/// not inflate the count), and git state is never mutated to measure it (no
/// `git add`, no intent-to-add).
///
/// Returns `0` when `project_root` is not a git worktree or the tree is clean.
/// Any git/IO failure is non-fatal (fail-open), matching the rest of the guard.
pub(crate) fn working_tree_changed_lines(project_root: &Path) -> usize {
    let tracked = collect_uncommitted_changes(project_root)
        .map(|changes| changes.insertions.saturating_add(changes.deletions) as usize)
        .unwrap_or(0);
    tracked.saturating_add(untracked_non_ignored_lines(project_root))
}

/// Sum of line counts across untracked, non-ignored files.
///
/// `git ls-files --others --exclude-standard` enumerates files git is neither
/// tracking nor ignoring, so build artifacts and `.gitignore`d paths never
/// inflate the size measure, and the index/worktree are never mutated. `--others`
/// excludes anything already in the index (including intent-to-add entries, which
/// `git diff HEAD` already counts), so there is no double-counting with the
/// tracked diff. Fail-open: returns `0` when `project_root` is not a git worktree
/// or git fails.
fn untracked_non_ignored_lines(project_root: &Path) -> usize {
    if !super::git::is_git_worktree(project_root) {
        return 0;
    }
    let Some(listing) = run_git_capture(
        project_root,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    ) else {
        return 0;
    };
    listing
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(|rel| count_file_lines(&project_root.join(rel)))
        .sum()
}

/// Per-file byte ceiling when counting untracked-file lines. The gate only
/// distinguishes trivial (`<= TRIVIAL_DIFF_MAX_LINES`) from substantial, so
/// scanning past ~1 MiB of a single file cannot change the outcome; the ceiling
/// keeps a pathological large non-ignored file from forcing an unbounded read
/// (bounded-read discipline, mirroring the guard's other reads).
const MAX_LINE_SCAN_BYTES: usize = 1 << 20;

/// Count newline-delimited lines in `path` with bounded memory and IO.
///
/// Streams the file through a fixed buffer (never loading it whole) counting
/// `\n` bytes, stopping after [`MAX_LINE_SCAN_BYTES`]; a final line lacking a
/// trailing newline still counts. Fail-open: any IO error (including a missing
/// file) yields `0` so a transient read failure cannot abort the guard.
///
/// Only regular files are opened. `path` comes straight from an untracked
/// worktree enumeration ([`untracked_non_ignored_lines`]), which can legitimately
/// surface symlinks, FIFOs, sockets, or device nodes. `File::open` follows
/// symlinks and blocks indefinitely on a FIFO (and can hang or error on other
/// special files), so the path is first classified with `symlink_metadata` (which
/// does NOT follow symlinks); anything that is not a regular file is skipped
/// (counted as `0`) without ever being opened. Mirrors the classify-before-touch
/// pattern in [`crate::edit_restriction_guard`]'s `capture_path_state`.
fn count_file_lines(path: &Path) -> usize {
    use std::io::Read;

    // Classify WITHOUT following symlinks and WITHOUT opening: only a regular file
    // is safe to hand to `File::open` here. A stat error (missing/unreadable) or
    // any non-regular type (symlink, FIFO, socket, device) fails open to `0`.
    if !std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_file()) {
        return 0;
    }

    let Ok(file) = std::fs::File::open(path) else {
        return 0;
    };
    let mut reader = std::io::BufReader::new(file);
    let mut buf = [0u8; 16 * 1024];
    let mut newlines = 0usize;
    let mut scanned = 0usize;
    let mut last_byte = None;
    while scanned < MAX_LINE_SCAN_BYTES {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                newlines += buf[..n].iter().filter(|&&b| b == b'\n').count();
                last_byte = Some(buf[n - 1]);
                scanned = scanned.saturating_add(n);
            }
            Err(_) => return 0,
        }
    }
    // A non-empty file whose final scanned byte is not a newline has a trailing
    // partial line; an empty file (`None`) or one ending in `\n` does not.
    match last_byte {
        Some(b) if b != b'\n' => newlines.saturating_add(1),
        _ => newlines,
    }
}

fn collect_uncommitted_changes(project_root: &Path) -> Option<csa_session::UncommittedChanges> {
    if !super::git::is_git_worktree(project_root) {
        return None;
    }

    let porcelain = run_git_capture(
        project_root,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--no-renames",
            "-z",
        ],
    )?;
    if porcelain.is_empty() {
        return None;
    }
    let numstat =
        run_git_capture(project_root, &["diff", "--numstat", "HEAD", "--"]).unwrap_or_default();
    summarize_uncommitted_changes(&porcelain, &numstat)
}

fn run_git_capture(project_root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

fn changed_paths_from_porcelain(porcelain: &str) -> Vec<String> {
    porcelain_entries(porcelain)
        .into_iter()
        .filter_map(parse_porcelain_path)
        .collect()
}

fn porcelain_entries(porcelain: &str) -> Vec<&str> {
    if porcelain.contains('\0') {
        porcelain
            .split('\0')
            .filter(|entry| !entry.is_empty())
            .collect()
    } else {
        porcelain
            .lines()
            .filter(|entry| !entry.is_empty())
            .collect()
    }
}

fn parse_porcelain_path(entry: &str) -> Option<String> {
    let mut chars = entry.chars();
    chars.next()?;
    chars.next()?;
    if chars.next()? != ' ' {
        return None;
    }
    let path = entry.get(3..)?.trim();
    (!path.is_empty()).then(|| path.to_string())
}

fn parse_numstat_totals(numstat: &str) -> (u64, u64) {
    let mut insertions = 0u64;
    let mut deletions = 0u64;

    for line in numstat.lines() {
        let mut columns = line.split('\t');
        let added = columns.next().and_then(|raw| raw.parse::<u64>().ok());
        let removed = columns.next().and_then(|raw| raw.parse::<u64>().ok());
        if let Some(value) = added {
            insertions = insertions.saturating_add(value);
        }
        if let Some(value) = removed {
            deletions = deletions.saturating_add(value);
        }
    }

    (insertions, deletions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_predicate_excludes_sa_mode_and_read_only_kinds() {
        assert!(is_writer_session(false, Some("run")));
        assert!(!is_writer_session(true, Some("run")));
        assert!(!is_writer_session(false, Some("review")));
        assert!(!is_writer_session(false, Some("debate")));
        assert!(!is_writer_session(false, Some("recon")));
        assert!(!is_writer_session(false, None));
    }

    #[test]
    fn summarize_uncommitted_changes_returns_none_for_clean_status() {
        assert!(summarize_uncommitted_changes("", "").is_none());
    }

    #[test]
    fn summarize_uncommitted_changes_counts_files_and_numstat() {
        let porcelain = " M crates/a.rs\0A  crates/b.rs\0?? notes/todo.md\0";
        let numstat = "10\t2\tcrates/a.rs\n5\t0\tcrates/b.rs\n-\t-\tassets/blob.bin\n";

        let changes = summarize_uncommitted_changes(porcelain, numstat)
            .expect("dirty porcelain should produce changes");

        assert_eq!(changes.file_count, 3);
        assert_eq!(changes.insertions, 15);
        assert_eq!(changes.deletions, 2);
        assert_eq!(
            changes.files,
            vec![
                "crates/a.rs".to_string(),
                "crates/b.rs".to_string(),
                "notes/todo.md".to_string()
            ]
        );
        assert_eq!(changes.truncated, 0);
    }

    #[test]
    fn summarize_uncommitted_changes_caps_file_list() {
        let porcelain = (0..25)
            .map(|idx| format!("?? file-{idx}.txt\0"))
            .collect::<String>();

        let changes = summarize_uncommitted_changes(&porcelain, "")
            .expect("dirty porcelain should produce changes");

        assert_eq!(changes.file_count, 25);
        assert_eq!(changes.files.len(), MAX_UNCOMMITTED_FILES);
        assert_eq!(changes.truncated, 5);
    }

    #[test]
    fn apply_uncommitted_changes_warn_only_preserves_success() {
        let mut result = session_result("success", 0);
        let changes = csa_session::UncommittedChanges {
            file_count: 1,
            insertions: 2,
            deletions: 0,
            files: vec!["src/lib.rs".to_string()],
            truncated: 0,
        };

        apply_uncommitted_changes_to_result(&mut result, changes, false);

        assert_eq!(result.status, "success");
        assert_eq!(result.exit_code, 0);
        assert!(result.uncommitted_changes.is_some());
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn apply_uncommitted_changes_require_commit_flips_to_failure() {
        let mut result = session_result("success", 0);
        let changes = csa_session::UncommittedChanges {
            file_count: 1,
            insertions: 2,
            deletions: 0,
            files: vec!["src/lib.rs".to_string()],
            truncated: 0,
        };

        apply_uncommitted_changes_to_result(&mut result, changes, true);

        assert_eq!(result.status, "failure");
        assert_eq!(result.exit_code, 1);
        assert_eq!(result.summary, REQUIRE_COMMIT_REASON);
        assert!(result.uncommitted_changes.is_some());
    }

    #[test]
    fn effective_writer_must_commit_respects_cli_and_config_precedence() {
        assert!(!effective_writer_must_commit(false, None));

        let config_true: csa_config::ProjectConfig =
            toml::from_str("[run]\nwriter_must_commit = true\n").unwrap();
        assert!(effective_writer_must_commit(false, Some(&config_true)));

        let config_false: csa_config::ProjectConfig =
            toml::from_str("[run]\nwriter_must_commit = false\n").unwrap();
        assert!(effective_writer_must_commit(true, Some(&config_false)));
    }

    fn session_result(status: &str, exit_code: i32) -> csa_session::SessionResult {
        let now = chrono::Utc::now();
        csa_session::SessionResult {
            status: status.to_string(),
            exit_code,
            summary: "done".to_string(),
            tool: "codex".to_string(),
            original_tool: None,
            fallback_tool: None,
            fallback_reason: None,
            started_at: now,
            completed_at: now,
            events_count: 0,
            artifacts: Vec::new(),
            peak_memory_mb: None,
            fallback_chain: None,
            gate_timeout: false,
            warnings: Vec::new(),
            raw_process_exit_code: None,
            uncommitted_changes: None,
            manager_fields: Default::default(),
        }
    }

    fn run_git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .expect("git command should execute");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// A throwaway git repo with one commit so `HEAD` exists. Hooks and GPG
    /// signing are disabled so the test stays hermetic regardless of the host's
    /// global git config.
    fn init_repo_with_initial_commit() -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        run_git(root, &["init", "-q"]);
        run_git(root, &["config", "user.email", "test@example.com"]);
        run_git(root, &["config", "user.name", "Test User"]);
        run_git(root, &["config", "commit.gpgsign", "false"]);
        run_git(
            root,
            &["config", "core.hooksPath", "/nonexistent-csa-hooks"],
        );
        std::fs::write(root.join("seed.txt"), "seed\n").expect("write seed");
        run_git(root, &["add", "seed.txt"]);
        run_git(root, &["commit", "-q", "-m", "initial"]);
        temp
    }

    #[test]
    fn count_file_lines_handles_trailing_newline_partial_line_and_missing() {
        let temp = tempfile::tempdir().unwrap();
        let with_nl = temp.path().join("with_nl.txt");
        std::fs::write(&with_nl, "x\ny\nz\n").unwrap();
        assert_eq!(count_file_lines(&with_nl), 3);

        let no_nl = temp.path().join("no_nl.txt");
        std::fs::write(&no_nl, "x\ny\nz").unwrap();
        assert_eq!(count_file_lines(&no_nl), 3);

        let empty = temp.path().join("empty.txt");
        std::fs::write(&empty, "").unwrap();
        assert_eq!(count_file_lines(&empty), 0);

        assert_eq!(count_file_lines(&temp.path().join("missing.txt")), 0);
    }

    /// Create a FIFO (named pipe) at `path`. Unix-only; used to prove the
    /// regular-file gate skips special files instead of blocking on `File::open`.
    #[cfg(unix)]
    fn make_fifo(path: &Path) {
        use std::os::unix::ffi::OsStrExt;
        let c_path =
            std::ffi::CString::new(path.as_os_str().as_bytes()).expect("fifo path has no NUL byte");
        // SAFETY: `c_path` is a valid NUL-terminated path that outlives the call;
        // mode 0o600 is a standard FIFO permission. The return code is checked.
        let rc = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
        assert_eq!(
            rc,
            0,
            "mkfifo({}) failed: {}",
            path.display(),
            std::io::Error::last_os_error()
        );
    }

    #[test]
    fn count_file_lines_skips_non_regular_paths_without_blocking() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();

        // (a) A regular file is still counted normally.
        let regular = root.join("regular.rs");
        std::fs::write(&regular, "a\nb\nc\n").unwrap();
        assert_eq!(count_file_lines(&regular), 3);

        // (b) A symlink is non-regular: it MUST be skipped (0), not FOLLOWED to its
        // regular target (which would count 3). Proves classification uses
        // symlink_metadata rather than following the link.
        #[cfg(unix)]
        {
            let link = root.join("link.rs");
            std::os::unix::fs::symlink(&regular, &link).unwrap();
            assert_eq!(
                count_file_lines(&link),
                0,
                "symlink must be skipped, not followed"
            );
        }

        // (c) A FIFO makes a blocking `File::open` hang forever pre-fix. Probe on a
        // worker thread so a regression surfaces as a bounded timeout failure
        // instead of an indefinite CI hang.
        #[cfg(unix)]
        {
            use std::sync::mpsc;
            use std::time::Duration;

            let fifo = root.join("pipe");
            make_fifo(&fifo);
            let probe = fifo.clone();
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(count_file_lines(&probe));
            });
            let counted = rx.recv_timeout(Duration::from_secs(5)).expect(
                "count_file_lines blocked on a FIFO — regression: regular-file gate missing",
            );
            assert_eq!(counted, 0, "FIFO must be skipped, not opened");
        }
    }

    #[test]
    fn working_tree_changed_lines_counts_untracked_non_ignored_files() {
        let temp = init_repo_with_initial_commit();
        let root = temp.path();
        // Substantial NEW work composed ENTIRELY of untracked files: `git diff
        // HEAD` sees nothing, so the pre-fix measure would have returned 0.
        let body: String = (0..50).map(|i| format!("line {i}\n")).collect();
        std::fs::write(root.join("new_module.rs"), &body).unwrap();

        let measured = working_tree_changed_lines(root);
        assert!(
            measured >= 50,
            "untracked-file lines must count toward the size measure, got {measured}"
        );
    }

    #[test]
    fn working_tree_changed_lines_combines_tracked_and_untracked() {
        let temp = init_repo_with_initial_commit();
        let root = temp.path();
        // Modify a tracked file (appears in `git diff HEAD`)...
        std::fs::write(root.join("seed.txt"), "seed\nedit-a\nedit-b\n").unwrap();
        // ...and add an untracked file (does not).
        std::fs::write(root.join("extra.txt"), "u1\nu2\nu3\nu4\n").unwrap();

        // 2 tracked insertions + 4 untracked lines = 6, all counted, none double.
        assert_eq!(working_tree_changed_lines(root), 6);
    }

    #[test]
    fn working_tree_changed_lines_excludes_gitignored_files() {
        let temp = init_repo_with_initial_commit();
        let root = temp.path();
        // Commit `.gitignore` so it is tracked-and-clean, not itself an untracked
        // file that would count.
        std::fs::write(root.join(".gitignore"), "build/\n*.log\n").unwrap();
        run_git(root, &["add", ".gitignore"]);
        run_git(root, &["commit", "-q", "-m", "add gitignore"]);

        // Large ignored content must not inflate the measure.
        std::fs::create_dir_all(root.join("build")).unwrap();
        let big: String = (0..500).map(|i| format!("artifact {i}\n")).collect();
        std::fs::write(root.join("build/out.txt"), &big).unwrap();
        std::fs::write(root.join("debug.log"), &big).unwrap();

        assert_eq!(
            working_tree_changed_lines(root),
            0,
            "ignored files must not inflate the writer-guard size measure"
        );
    }

    #[test]
    fn working_tree_changed_lines_zero_for_non_git_dir() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(working_tree_changed_lines(temp.path()), 0);
    }
}
