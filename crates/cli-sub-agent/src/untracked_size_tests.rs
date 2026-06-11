//! Tests for [`crate::untracked_size`], in a sibling file so the implementation
//! module stays well within the per-module token budget (#1818).

use super::*;

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

/// A throwaway git repo. Hooks and GPG signing are disabled so the test stays
/// hermetic regardless of the host's global git config. No commit is made —
/// `git ls-files --others` works without `HEAD`.
fn init_repo() -> tempfile::TempDir {
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
    temp
}

#[cfg(unix)]
fn make_pathspec_echo_git(root: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let fake_git = root.join("git");
    std::fs::write(
        &fake_git,
        r#"#!/bin/sh
set -eu
dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
count_file="$dir/git-count"
if [ -f "$count_file" ]; then
    count=$(cat "$count_file")
else
    count=0
fi
count=$((count + 1))
printf '%s' "$count" > "$count_file"
after_separator=0
for arg in "$@"; do
    if [ "$after_separator" -eq 1 ]; then
        printf '%s\000' "$arg"
    fi
    if [ "$arg" = "--" ]; then
        after_separator=1
    fi
done
"#,
    )
    .unwrap();
    let mut perms = std::fs::metadata(&fake_git).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&fake_git, perms).unwrap();
    fake_git
}

#[test]
fn classify_counts_exact_small_text_file() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("a.rs");
    std::fs::write(&file, "one\ntwo\nthree\n").unwrap();

    match classify_untracked_file(&file) {
        FileClass::Text {
            lines,
            capped,
            bytes,
        } => {
            assert_eq!(lines, 3);
            assert!(!capped, "small file must not be flagged capped");
            assert_eq!(bytes, 14);
        }
        _ => panic!("expected a counted text file"),
    }
}

#[test]
fn classify_counts_trailing_partial_line() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("no_nl.txt");
    std::fs::write(&file, "x\ny\nz").unwrap(); // no trailing newline

    match classify_untracked_file(&file) {
        FileClass::Text { lines, .. } => assert_eq!(lines, 3),
        _ => panic!("expected a counted text file"),
    }
}

#[test]
fn classify_marks_large_file_without_reading_it() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("big.bin");
    // One byte over the ceiling, all newlines: a naive line count would report
    // > 1M lines. `Large` proves it was sized, not read.
    let size = (MAX_LINE_SCAN_BYTES + 1) as usize;
    std::fs::write(&file, vec![b'\n'; size]).unwrap();

    match classify_untracked_file(&file) {
        FileClass::Large { bytes } => assert_eq!(bytes, MAX_LINE_SCAN_BYTES + 1),
        _ => panic!("a file above the byte ceiling must be classified Large, not line-counted"),
    }
}

#[test]
fn classify_detects_binary_via_nul_byte() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("blob.bin");
    std::fs::write(&file, b"text\n\0\x01\x02\nmore\n").unwrap();

    match classify_untracked_file(&file) {
        FileClass::Binary { bytes } => assert_eq!(bytes, 14),
        _ => panic!("a file with a NUL byte must be classified Binary, no bogus line count"),
    }
}

#[test]
fn classify_caps_pathological_line_count() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("many_lines.txt");
    // Under the byte ceiling but over the line ceiling: MAX_LINES_PER_FILE+10
    // newlines occupy ~50 KiB, well below 1 MiB.
    std::fs::write(&file, vec![b'\n'; MAX_LINES_PER_FILE + 10]).unwrap();

    match classify_untracked_file(&file) {
        FileClass::Text { lines, capped, .. } => {
            assert_eq!(lines, MAX_LINES_PER_FILE);
            assert!(capped, "line count at the ceiling must be flagged capped");
        }
        _ => panic!("expected a capped text file"),
    }
}

#[test]
fn classify_tolerates_missing_path() {
    let temp = tempfile::tempdir().unwrap();
    assert!(matches!(
        classify_untracked_file(&temp.path().join("missing.txt")),
        FileClass::Skipped
    ));
}

#[cfg(unix)]
#[test]
fn classify_skips_symlink_without_following_it() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target.rs");
    std::fs::write(&target, "a\nb\nc\n").unwrap();
    let link = temp.path().join("link.rs");
    std::os::unix::fs::symlink(&target, &link).unwrap();

    assert!(
        matches!(classify_untracked_file(&link), FileClass::Skipped),
        "a symlink must be skipped, not followed to its regular target"
    );
}

/// Create a FIFO at `path`. Used to prove the regular-file gate skips special
/// files instead of blocking forever on `File::open`.
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

#[cfg(unix)]
#[test]
fn classify_skips_fifo_without_blocking() {
    use std::sync::mpsc;
    use std::time::Duration;

    let temp = tempfile::tempdir().unwrap();
    let fifo = temp.path().join("pipe");
    make_fifo(&fifo);

    let probe = fifo.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(matches!(
            classify_untracked_file(&probe),
            FileClass::Skipped
        ));
    });
    let skipped = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("classify blocked on a FIFO — regression: regular-file gate missing");
    assert!(skipped, "a FIFO must be skipped, not opened");
}

#[test]
fn untracked_diff_size_counts_exact_small_files() {
    let temp = init_repo();
    let root = temp.path();
    std::fs::write(root.join("a.txt"), "1\n2\n3\n").unwrap();
    std::fs::write(root.join("b.txt"), "4\n5\n").unwrap();

    let size = untracked_diff_size(root);

    assert_eq!(size.files, 2);
    assert_eq!(size.lines, 5);
    assert_eq!(size.bytes, 6 + 4);
    assert!(!size.lower_bound);
    assert!(
        size.notes.is_empty(),
        "exact small files need no estimated/capped note, got {:?}",
        size.notes
    );
}

#[test]
fn untracked_diff_size_for_paths_counts_only_matching_files() {
    let temp = init_repo();
    let root = temp.path();
    std::fs::write(root.join("a.txt"), "1\n2\n3\n").unwrap();
    std::fs::write(root.join("b.txt"), "4\n5\n").unwrap();
    let filter = std::collections::BTreeSet::from(["b.txt".to_string()]);

    let size = untracked_diff_size_for_paths(root, &filter);

    assert_eq!(size.files, 1);
    assert_eq!(size.lines, 2);
    assert_eq!(size.bytes, 4);
    assert!(size.notes.is_empty());
}

#[test]
fn untracked_diff_size_for_paths_checks_filtered_paths_after_global_cap() {
    let temp = init_repo();
    let root = temp.path();
    for i in 0..=MAX_UNTRACKED_FILES {
        std::fs::write(root.join(format!("aa-preexisting-{i:04}.txt")), "x\n").unwrap();
    }
    std::fs::write(
        root.join("zz-large.txt"),
        vec![b'x'; (MAX_LINE_SCAN_BYTES + 1) as usize],
    )
    .unwrap();
    let filter = std::collections::BTreeSet::from(["zz-large.txt".to_string()]);

    let size = untracked_diff_size_for_paths(root, &filter);

    assert_eq!(size.files, 1);
    assert_eq!(size.lines, 0, "large files are sized but not line-counted");
    assert_eq!(size.bytes, MAX_LINE_SCAN_BYTES + 1);
    assert!(size.lower_bound);
    assert!(
        size.notes
            .iter()
            .any(|note| note.contains("not line-counted")),
        "large file should add a lower-bound note, got {:?}",
        size.notes
    );
    assert!(
        !size.notes.iter().any(|note| note.contains("truncated")),
        "filtered path sizing must not inherit the global untracked cap note, got {:?}",
        size.notes
    );
}

#[cfg(unix)]
#[test]
fn filtered_untracked_listing_batches_many_pathspecs_once() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    let fake_git = make_pathspec_echo_git(temp.path());
    let filter = (0..64)
        .map(|i| format!("new-{i:04}.rs"))
        .collect::<BTreeSet<_>>();

    let listing = list_filtered_untracked_with_git(&root, &filter, fake_git.as_os_str());

    assert_eq!(listing.paths.len(), filter.len());
    assert!(!listing.truncated);
    let count = std::fs::read_to_string(temp.path().join("git-count")).unwrap();
    assert_eq!(
        count, "1",
        "filtered sizing must batch pathspecs into one git call"
    );
}

#[cfg(unix)]
#[test]
fn filtered_untracked_listing_caps_many_pathspecs_before_git() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    let fake_git = make_pathspec_echo_git(temp.path());
    let filter = (0..(MAX_UNTRACKED_FILES + 25))
        .map(|i| format!("new-{i:04}.rs"))
        .collect::<BTreeSet<_>>();

    let listing = list_filtered_untracked_with_git(&root, &filter, fake_git.as_os_str());

    assert_eq!(listing.paths.len(), MAX_UNTRACKED_FILES);
    assert!(
        listing.truncated,
        "oversized filtered path sets must be marked lower-bound/truncated"
    );
    let count = std::fs::read_to_string(temp.path().join("git-count")).unwrap();
    assert_eq!(
        count, "1",
        "filtered sizing must not spawn once per changed path"
    );
}

#[test]
fn untracked_diff_size_excludes_gitignored_files() {
    let temp = init_repo();
    let root = temp.path();
    std::fs::write(root.join(".gitignore"), "build/\n*.log\n").unwrap();
    run_git(root, &["add", ".gitignore"]);
    run_git(root, &["commit", "-q", "-m", "ignore"]);

    std::fs::create_dir_all(root.join("build")).unwrap();
    let big: String = (0..500).map(|i| format!("artifact {i}\n")).collect();
    std::fs::write(root.join("build/out.txt"), &big).unwrap();
    std::fs::write(root.join("debug.log"), &big).unwrap();

    let size = untracked_diff_size(root);
    assert_eq!(size.files, 0, "ignored files must not be scanned");
    assert_eq!(size.lines, 0);
    assert!(size.notes.is_empty());
}

#[test]
fn untracked_diff_size_marks_large_and_binary_without_inflating_lines() {
    let temp = init_repo();
    let root = temp.path();
    std::fs::write(root.join("ok.txt"), "a\nb\n").unwrap();
    std::fs::write(
        root.join("huge.bin"),
        vec![b'\n'; (MAX_LINE_SCAN_BYTES + 1) as usize],
    )
    .unwrap();
    std::fs::write(root.join("blob.bin"), b"x\n\0\0\0\n").unwrap();

    let size = untracked_diff_size(root);

    assert_eq!(size.files, 3, "all three regular files are sized");
    assert_eq!(size.lines, 2, "only the exact text file contributes lines");
    assert!(size.lower_bound);
    assert_eq!(
        size.notes.len(),
        1,
        "large+binary collapse into one not-line-counted note, got {:?}",
        size.notes
    );
    assert!(size.notes[0].contains("not line-counted"));
    assert!(size.notes[0].contains("lower bound"));
}

#[test]
fn untracked_diff_size_truncates_beyond_file_cap() {
    let temp = init_repo();
    let root = temp.path();
    let extra = 5;
    for i in 0..(MAX_UNTRACKED_FILES + extra) {
        std::fs::write(root.join(format!("f{i}.txt")), "x\n").unwrap();
    }

    let size = untracked_diff_size(root);

    assert_eq!(
        size.files, MAX_UNTRACKED_FILES,
        "the scan must stop at the file cap"
    );
    assert_eq!(size.lines, MAX_UNTRACKED_FILES, "one line per scanned file");
    assert!(
        size.notes.iter().any(|note| note.contains("truncated")),
        "exceeding the file cap must set a truncation note, got {:?}",
        size.notes
    );
}

#[cfg(unix)]
#[test]
fn untracked_diff_size_tolerates_non_regular_entry() {
    let temp = init_repo();
    let root = temp.path();
    std::fs::write(root.join("good.txt"), "a\nb\nc\n").unwrap();
    // An untracked symlink (git lists it under --others) must be skipped, and the
    // good file still counted — one bad entry never aborts the scan.
    std::os::unix::fs::symlink(root.join("good.txt"), root.join("link.txt")).unwrap();

    let size = untracked_diff_size(root);
    assert_eq!(size.files, 1, "only the regular file is sized");
    assert_eq!(size.lines, 3);
}

#[test]
fn untracked_diff_size_empty_for_non_git_dir() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("loose.txt"), "ignored without git\n").unwrap();

    let size = untracked_diff_size(temp.path());
    assert_eq!(size.files, 0);
    assert_eq!(size.lines, 0);
    assert_eq!(size.bytes, 0);
    assert!(size.notes.is_empty());
}

// --- Enumeration-boundary tests -------------------------------------------
//
// The `untracked_diff_size_truncates_beyond_file_cap` test above proves the
// *report* is bounded, but it cannot prove the enumeration STOPS EARLY rather
// than collecting every path and truncating afterward. These exercise
// `read_nul_delimited_capped` directly — the streaming core — so a regression
// back to "buffer everything, then `.take(cap)`" fails a test.

#[test]
fn nul_parser_stops_early_without_draining_the_stream() {
    // Far more NUL-delimited entries than the cap. Fixed-width names (10 bytes +
    // a NUL = 11 bytes each) keep the consumed-byte arithmetic exact.
    let cap = MAX_UNTRACKED_FILES;
    let entries = cap * 50;
    let mut input = Vec::new();
    for i in 0..entries {
        input.extend_from_slice(format!("f{i:09}").as_bytes());
        input.push(0u8);
    }
    let total_len = input.len();
    let mut cursor = std::io::Cursor::new(input);

    let (paths, truncated) = read_nul_delimited_capped(&mut cursor, cap);

    assert_eq!(paths.len(), cap, "enumeration must yield exactly the cap");
    assert!(
        truncated,
        "more entries than the cap must set the truncated flag"
    );
    // The parser must NOT have read the whole stream: it consumes only the `cap`
    // entries plus the single probe entry that proves truncation. A `Cursor`'s
    // position is the exact byte offset consumed (no `BufReader` read-ahead), so
    // this asserts early-stop, not merely post-collection truncation.
    let consumed = cursor.position() as usize;
    assert!(
        consumed < total_len / 10,
        "early stop expected: consumed {consumed} of {total_len} bytes (no full drain)"
    );
}

#[test]
fn nul_parser_exactly_cap_entries_is_not_truncated() {
    // Exactly `cap` entries must NOT report truncation: the +1 probe hits EOF.
    let mut input = Vec::new();
    for i in 0..3 {
        input.extend_from_slice(format!("p{i}").as_bytes());
        input.push(0u8);
    }
    let mut cursor = std::io::Cursor::new(input);

    let (paths, truncated) = read_nul_delimited_capped(&mut cursor, 3);

    assert_eq!(paths, vec!["p0", "p1", "p2"]);
    assert!(!truncated, "exactly cap entries is not truncation");
}

#[test]
fn nul_parser_below_cap_returns_all_entries() {
    let mut input = Vec::new();
    for i in 0..4 {
        input.extend_from_slice(format!("p{i}").as_bytes());
        input.push(0u8);
    }
    let mut cursor = std::io::Cursor::new(input);

    let (paths, truncated) = read_nul_delimited_capped(&mut cursor, 100);

    assert_eq!(paths, vec!["p0", "p1", "p2", "p3"]);
    assert!(!truncated);
}

#[test]
fn nul_parser_skips_empty_and_tolerates_unterminated_tail() {
    // "a\0\0b": the empty middle entry is skipped; the final "b" has no trailing
    // NUL (git -z always terminates, but the parser must not drop the tail).
    let mut cursor = std::io::Cursor::new(b"a\0\0b".to_vec());

    let (paths, truncated) = read_nul_delimited_capped(&mut cursor, 100);

    assert_eq!(paths, vec!["a", "b"]);
    assert!(!truncated);
}
