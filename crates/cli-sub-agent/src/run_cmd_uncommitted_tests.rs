use std::io::Read;
use std::path::Path;
use std::process::Command;

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
    assert_eq!(changes.approx_diff_tokens, 0);
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
        approx_diff_tokens: 16,
        files: vec!["src/lib.rs".to_string()],
        truncated: 0,
    };

    apply_uncommitted_changes_to_result(&mut result, changes, None, false, None);

    assert_eq!(result.status, "success");
    assert_eq!(result.exit_code, 0);
    assert!(result.uncommitted_changes.is_some());
    assert!(result.large_diff_warning.is_none());
    assert!(result.warnings.is_empty());
}

#[test]
fn apply_uncommitted_changes_require_commit_flips_to_failure() {
    let mut result = session_result("success", 0);
    let changes = csa_session::UncommittedChanges {
        file_count: 1,
        insertions: 2,
        deletions: 0,
        approx_diff_tokens: 16,
        files: vec!["src/lib.rs".to_string()],
        truncated: 0,
    };

    apply_uncommitted_changes_to_result(&mut result, changes, None, true, None);

    assert_eq!(result.status, "failure");
    assert_eq!(result.exit_code, 1);
    assert_eq!(result.summary, REQUIRE_COMMIT_REASON);
    assert!(result.uncommitted_changes.is_some());
}

#[test]
fn require_commit_recovery_diagnostic_preserves_signal_exit_and_sanitized_paths() {
    let mut result = session_result("signal", 143);
    result.kill_hint = Some("memory_pressure".to_string());
    let changes = csa_session::UncommittedChanges {
        file_count: 4,
        insertions: 8,
        deletions: 1,
        approx_diff_tokens: 64,
        files: vec![
            "src/lib.rs".to_string(),
            "secret\nname.txt".to_string(),
            "../outside.txt".to_string(),
        ],
        truncated: 1,
    };
    let recovery = build_require_commit_recovery_diagnostic(&result, &changes);

    apply_uncommitted_changes_to_result(&mut result, changes, None, true, Some(recovery.clone()));

    assert_eq!(result.status, "failure");
    assert_eq!(result.exit_code, 1);
    let persisted = result
        .require_commit_recovery
        .expect("recovery diagnostic should be attached");
    assert!(persisted.require_commit);
    assert!(!persisted.commit_created);
    assert!(persisted.dirty_worktree);
    assert_eq!(persisted.termination_status, "signal");
    assert_eq!(persisted.exit_code, 143);
    assert_eq!(persisted.termination_signal, Some(15));
    assert_eq!(persisted.kill_hint.as_deref(), Some("memory_pressure"));
    assert_eq!(
        persisted.changed_paths,
        vec![
            "src/lib.rs".to_string(),
            "secret�name.txt".to_string(),
            REDACTED_PATH.to_string(),
        ]
    );
    assert_eq!(persisted.changed_paths_truncated, 1);
    assert_eq!(
        persisted.suggested_recovery_action,
        REQUIRE_COMMIT_RECOVERY_ACTION
    );
}

#[test]
fn require_commit_recovery_diagnostic_covers_generic_nonzero_exit() {
    let result = session_result("failure", 2);
    let changes = csa_session::UncommittedChanges {
        file_count: 1,
        insertions: 1,
        deletions: 0,
        approx_diff_tokens: 4,
        files: vec!["src/main.rs".to_string()],
        truncated: 0,
    };

    let recovery = build_require_commit_recovery_diagnostic(&result, &changes);

    assert_eq!(recovery.termination_status, "failure");
    assert_eq!(recovery.exit_code, 2);
    assert_eq!(recovery.termination_signal, None);
    assert_eq!(recovery.changed_paths, vec!["src/main.rs".to_string()]);
}

#[test]
fn require_commit_with_commit_created_does_not_record_recovery_or_fail_result() {
    let temp = init_repo_with_initial_commit();
    let root = temp.path();
    let session = csa_session::create_session(root, Some("run"), None, Some("codex"))
        .expect("session should be created");
    let mut session_result = session_result("success", 0);
    csa_session::save_result(root, &session.meta_session_id, &session_result)
        .expect("result should be saved");
    std::fs::write(root.join("src.rs"), "dirty\n").expect("dirty file");
    let mut execution = csa_process::ExecutionResult {
        exit_code: 0,
        summary: "done".to_string(),
        ..Default::default()
    };

    record_writer_uncommitted_changes_with_config(
        root,
        Some(&session.meta_session_id),
        &mut execution,
        WriterUncommittedRecord {
            sa_mode: false,
            require_commit: true,
            changed_paths: Some(&["src.rs".to_string()]),
            commit_created: Some(true),
            large_diff_config: &RunLargeDiffWarningConfig::default(),
        },
    );

    session_result = csa_session::load_result(root, &session.meta_session_id)
        .expect("load result")
        .expect("result should exist");
    assert_eq!(execution.exit_code, 0);
    assert_eq!(session_result.status, "success");
    assert!(session_result.require_commit_recovery.is_none());
}

#[test]
fn require_commit_with_no_session_dirty_paths_does_not_fail_or_diagnose() {
    let temp = init_repo_with_initial_commit();
    let root = temp.path();
    let session = csa_session::create_session(root, Some("run"), None, Some("codex"))
        .expect("session should be created");
    let session_result = session_result("success", 0);
    csa_session::save_result(root, &session.meta_session_id, &session_result)
        .expect("result should be saved");
    std::fs::write(root.join("preexisting.txt"), "dirty\n").expect("dirty file");
    let mut execution = csa_process::ExecutionResult {
        exit_code: 0,
        summary: "done".to_string(),
        ..Default::default()
    };

    record_writer_uncommitted_changes_with_config(
        root,
        Some(&session.meta_session_id),
        &mut execution,
        WriterUncommittedRecord {
            sa_mode: false,
            require_commit: true,
            changed_paths: Some(&[]),
            commit_created: Some(false),
            large_diff_config: &RunLargeDiffWarningConfig::default(),
        },
    );

    let loaded = csa_session::load_result(root, &session.meta_session_id)
        .expect("load result")
        .expect("result should exist");
    assert_eq!(execution.exit_code, 0);
    assert_eq!(loaded.status, "success");
    assert!(loaded.uncommitted_changes.is_none());
    assert!(loaded.require_commit_recovery.is_none());
}

#[test]
fn diff_stream_token_estimate_stops_at_threshold_byte_limit() {
    use std::cell::Cell;
    use std::rc::Rc;

    struct CountingReader {
        remaining: usize,
        read_bytes: Rc<Cell<usize>>,
    }

    impl Read for CountingReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.remaining == 0 {
                return Ok(0);
            }
            let n = self.remaining.min(buf.len());
            buf[..n].fill(b'x');
            self.remaining -= n;
            self.read_bytes.set(self.read_bytes.get() + n);
            Ok(n)
        }
    }

    let threshold = 10;
    let read_bytes = Rc::new(Cell::new(0));
    let reader = CountingReader {
        remaining: tracked_diff_byte_limit(threshold) * 10,
        read_bytes: Rc::clone(&read_bytes),
    };

    let estimate =
        estimate_diff_stream_tokens(reader, threshold).expect("stream estimate should succeed");

    assert!(estimate.cap_reached);
    assert_eq!(estimate.tokens, threshold + 1);
    assert_eq!(read_bytes.get(), tracked_diff_byte_limit(threshold));
}

#[test]
fn collect_uncommitted_changes_caps_tracked_diff_token_estimate() {
    let temp = init_repo_with_initial_commit();
    let root = temp.path();
    let long_line = "x".repeat(tracked_diff_byte_limit(10) * 4);
    std::fs::write(root.join("seed.txt"), format!("seed\n{long_line}\n")).unwrap();

    let changes = collect_uncommitted_changes_with_token_threshold(root, 10)
        .expect("tracked mutation should be reported");

    assert_eq!(changes.file_count, 1);
    assert!(
        changes.changed_lines() < 100,
        "test fixture should exercise token threshold, not line threshold"
    );
    assert_eq!(changes.approx_diff_tokens, 11);

    let warning = large_diff_warning_report(
        &changes,
        &RunLargeDiffWarningConfig {
            enabled: true,
            changed_files: 100,
            changed_lines: 100,
            approx_diff_tokens: 10,
            mode: RunLargeDiffWarningMode::Warn,
        },
    )
    .expect("bounded tracked token estimate should still trip token threshold");

    assert_eq!(warning.approx_diff_tokens, 11);
}

#[test]
fn large_diff_warning_report_triggers_on_file_count() {
    let changes = changes(6, 10, 5, 100);
    let warning = large_diff_warning_report(&changes, &RunLargeDiffWarningConfig::default())
        .expect("file count above default threshold should warn");

    assert_eq!(warning.changed_files, 6);
    assert_eq!(warning.changed_lines, 15);
    assert_eq!(warning.approx_diff_tokens, 100);
}

#[test]
fn large_diff_warning_report_triggers_on_changed_lines() {
    let changes = changes(2, 501, 0, 100);
    let warning = large_diff_warning_report(&changes, &RunLargeDiffWarningConfig::default())
        .expect("changed lines above default threshold should warn");

    assert_eq!(warning.changed_files, 2);
    assert_eq!(warning.changed_lines, 501);
    assert_eq!(warning.approx_diff_tokens, 100);
}

#[test]
fn large_diff_warning_report_triggers_on_approx_tokens() {
    let changes = changes(2, 10, 5, 8_001);
    let warning = large_diff_warning_report(&changes, &RunLargeDiffWarningConfig::default())
        .expect("approx tokens above default threshold should warn");

    assert_eq!(warning.changed_files, 2);
    assert_eq!(warning.changed_lines, 15);
    assert_eq!(warning.approx_diff_tokens, 8_001);
}

#[test]
fn large_diff_warning_report_suppresses_small_and_disabled_changes() {
    let changes = changes(5, 250, 250, 8_000);

    assert!(large_diff_warning_report(&changes, &RunLargeDiffWarningConfig::default()).is_none());

    let disabled = RunLargeDiffWarningConfig {
        enabled: false,
        changed_files: 1,
        changed_lines: 1,
        approx_diff_tokens: 1,
        mode: RunLargeDiffWarningMode::Warn,
    };
    assert!(large_diff_warning_report(&changes, &disabled).is_none());
}

#[test]
fn format_large_diff_warning_block_is_parseable() {
    let warning = csa_session::LargeDiffWarningReport {
        changed_files: 9,
        changed_lines: 1_420,
        approx_diff_tokens: 18_000,
    };
    let block = format_large_diff_warning_block(&warning);

    assert!(block.starts_with(
        "<!-- CSA:LARGE_DIFF_WARNING changed_files=9 changed_lines=1420 approx_diff_tokens=18000 -->"
    ));
    assert!(block.contains("This CSA session left a large changed surface."));
    assert!(block.ends_with("<!-- CSA:LARGE_DIFF_WARNING:END -->"));
}

#[test]
fn large_diff_warning_changed_paths_empty_ignores_preexisting_dirty_surface() {
    let temp = init_repo_with_initial_commit();
    let root = temp.path();
    std::fs::write(root.join("preexisting.txt"), "x\n".repeat(600)).unwrap();

    let full_dirty = collect_uncommitted_changes(root).expect("dirty worktree should count");
    assert!(
        large_diff_warning_report(&full_dirty, &RunLargeDiffWarningConfig::default()).is_some()
    );

    let session_delta = collect_uncommitted_changes_for_changed_paths(root, &[]);

    assert!(session_delta.is_none());
}

#[test]
fn large_diff_warning_changed_paths_counts_filtered_untracked_file() {
    let temp = init_repo_with_initial_commit();
    let root = temp.path();
    std::fs::write(root.join("preexisting.txt"), "x\n".repeat(600)).unwrap();
    std::fs::write(root.join("new.txt"), "one\ntwo\nthree\n").unwrap();

    let changes = collect_uncommitted_changes_for_changed_paths(root, &["new.txt".to_string()])
        .expect("changed untracked file should count");

    assert_eq!(changes.file_count, 1);
    assert_eq!(changes.insertions, 3);
    assert_eq!(changes.deletions, 0);
    assert!(changes.approx_diff_tokens > 0);
    assert_eq!(changes.files, vec!["new.txt".to_string()]);
}

#[test]
fn large_diff_warning_changed_paths_counts_large_file_after_untracked_cap() {
    let temp = init_repo_with_initial_commit();
    let root = temp.path();
    for i in 0..=crate::untracked_size::MAX_UNTRACKED_FILES {
        std::fs::write(root.join(format!("aa-preexisting-{i:04}.txt")), "x\n").unwrap();
    }
    let large_path = "zz-large.txt";
    let large_bytes = (default_tracked_diff_token_threshold() + 1) * DIFF_BYTES_PER_TOKEN;
    std::fs::write(root.join(large_path), vec![b'x'; large_bytes]).unwrap();

    let changes = collect_uncommitted_changes_for_changed_paths(root, &[large_path.to_string()])
        .expect("changed untracked file should count despite noisy worktree");

    assert_eq!(changes.file_count, 1);
    assert_eq!(changes.files, vec![large_path.to_string()]);
    assert!(
        changes.approx_diff_tokens > default_tracked_diff_token_threshold(),
        "large changed file should exceed token warning threshold: {:?}",
        changes
    );
    assert!(
        large_diff_warning_report(&changes, &RunLargeDiffWarningConfig::default()).is_some(),
        "filtered large changed path should trigger the warning"
    );
}

#[test]
fn large_diff_warning_changed_paths_preserves_boundary_whitespace_in_untracked_paths() {
    let temp = init_repo_with_initial_commit();
    let root = temp.path();
    let leading_path = " leading-large.txt";
    let trailing_path = "trailing-large.txt ";
    let large_bytes = (default_tracked_diff_token_threshold() + 1) * DIFF_BYTES_PER_TOKEN;
    for path in [leading_path, trailing_path] {
        std::fs::write(root.join(path), vec![b'x'; large_bytes]).unwrap();
    }

    let changed_paths = vec![leading_path.to_string(), trailing_path.to_string()];
    let changes = collect_uncommitted_changes_for_changed_paths(root, &changed_paths)
        .expect("filtered untracked files with boundary whitespace should count");

    assert_eq!(changes.file_count, 2);
    assert_eq!(changes.files, changed_paths);
    assert!(
        large_diff_warning_report(&changes, &RunLargeDiffWarningConfig::default()).is_some(),
        "filtered large paths with boundary whitespace should trigger the warning"
    );
}

#[test]
fn large_diff_warning_changed_paths_treats_tracked_pathspec_magic_as_literal() {
    let temp = init_repo_with_initial_commit();
    let root = temp.path();
    let magic_path = ":(glob)large.rs";
    std::fs::write(root.join(magic_path), "seed\n").unwrap();
    run_git_literal_pathspecs(root, &["add", "--", magic_path]);
    run_git(root, &["commit", "-q", "-m", "add magic path"]);

    let token_threshold = 10;
    let long_line = "x".repeat(tracked_diff_byte_limit(token_threshold) * 4);
    std::fs::write(root.join(magic_path), format!("seed\n{long_line}\n")).unwrap();

    let changes = collect_uncommitted_changes_for_changed_paths_with_token_threshold(
        root,
        &[magic_path.to_string()],
        token_threshold,
    )
    .expect("filtered tracked path with pathspec magic should count");

    assert_eq!(changes.file_count, 1);
    assert_eq!(changes.files, vec![magic_path.to_string()]);
    assert_eq!(changes.insertions, 1);
    assert_eq!(changes.deletions, 0);
    assert_eq!(changes.approx_diff_tokens, token_threshold + 1);

    let warning = large_diff_warning_report(
        &changes,
        &RunLargeDiffWarningConfig {
            enabled: true,
            changed_files: 100,
            changed_lines: 100,
            approx_diff_tokens: token_threshold,
            mode: RunLargeDiffWarningMode::Warn,
        },
    )
    .expect("large literal tracked path should trip token threshold");

    assert_eq!(warning.approx_diff_tokens, token_threshold + 1);
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
        post_exec_gate: None,
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
        ..Default::default()
    }
}

fn changes(
    file_count: usize,
    insertions: u64,
    deletions: u64,
    approx_diff_tokens: usize,
) -> csa_session::UncommittedChanges {
    csa_session::UncommittedChanges {
        file_count,
        insertions,
        deletions,
        approx_diff_tokens,
        files: Vec::new(),
        truncated: 0,
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

fn run_git_literal_pathspecs(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_LITERAL_PATHSPECS", "1")
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
fn working_tree_changed_lines_counts_untracked_non_ignored_files() {
    let temp = init_repo_with_initial_commit();
    let root = temp.path();
    let body: String = (0..50).map(|i| format!("line {i}\n")).collect();
    std::fs::write(root.join("new_module.rs"), &body).unwrap();

    let measured = working_tree_changed_lines(root);
    assert!(
        measured >= 50,
        "untracked-file lines must count toward the size measure, got {measured}"
    );
}

#[test]
fn working_tree_changed_lines_treats_large_uncounted_untracked_file_as_substantial() {
    let temp = init_repo_with_initial_commit();
    let root = temp.path();
    let long_line = format!("{}\n", "x".repeat(64 * 1024));
    std::fs::write(root.join("large.txt"), long_line.repeat(17)).unwrap();

    let measured = working_tree_changed_lines(root);
    assert!(
        measured > 10,
        "lower-bound untracked size must not look trivial to the writer guard, got {measured}"
    );
}

#[test]
fn working_tree_changed_lines_combines_tracked_and_untracked() {
    let temp = init_repo_with_initial_commit();
    let root = temp.path();
    std::fs::write(root.join("seed.txt"), "seed\nedit-a\nedit-b\n").unwrap();
    std::fs::write(root.join("extra.txt"), "u1\nu2\nu3\nu4\n").unwrap();

    assert_eq!(working_tree_changed_lines(root), 6);
}

#[test]
fn working_tree_changed_lines_excludes_gitignored_files() {
    let temp = init_repo_with_initial_commit();
    let root = temp.path();
    std::fs::write(root.join(".gitignore"), "build/\n*.log\n").unwrap();
    run_git(root, &["add", ".gitignore"]);
    run_git(root, &["commit", "-q", "-m", "add gitignore"]);

    let big: String = (0..500).map(|i| format!("artifact {i}\n")).collect();
    std::fs::create_dir_all(root.join("build")).unwrap();
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
