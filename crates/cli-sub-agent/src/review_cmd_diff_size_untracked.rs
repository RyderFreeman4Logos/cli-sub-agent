use std::path::Path;
use std::process::Command;

use csa_session::ReviewDiffSize;
use tracing::warn;

const SCAN_FILE_LIMIT: usize = 5_000;

pub(super) fn collect(project_root: &Path) -> Option<ReviewDiffSize> {
    collect_with_limit(project_root, SCAN_FILE_LIMIT)
}

fn collect_with_limit(project_root: &Path, file_limit: usize) -> Option<ReviewDiffSize> {
    let paths = run_git(
        project_root,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?;
    let mut diff_size = ReviewDiffSize::default();

    for (scanned_files, path) in paths
        .split(|byte| *byte == b'\0')
        .filter(|path| !path.is_empty())
        .enumerate()
    {
        if scanned_files == file_limit {
            diff_size
                .notes
                .push(format!("untracked scan capped at {file_limit} files"));
            break;
        }

        let relative_path = String::from_utf8_lossy(path);
        let full_path = project_root.join(relative_path.as_ref());
        let Some(bytes) = regular_file_bytes(&full_path)? else {
            continue;
        };
        let Some(added_lines) = git_numstat_added_lines(project_root, relative_path.as_ref())?
        else {
            continue;
        };
        diff_size.files += 1;
        diff_size.changed_lines += added_lines;
        diff_size.bytes = diff_size.bytes.saturating_add(bytes);
    }

    Some(diff_size)
}

fn regular_file_bytes(path: &Path) -> Option<Option<usize>> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.file_type().is_file() {
        return Some(None);
    }
    Some(Some(match usize::try_from(metadata.len()) {
        Ok(len) => len,
        Err(_) => usize::MAX,
    }))
}

fn git_numstat_added_lines(project_root: &Path, relative_path: &str) -> Option<Option<usize>> {
    let output = Command::new("git")
        .args(["diff", "--no-index", "--numstat", "--", "/dev/null"])
        .arg(relative_path)
        .current_dir(project_root)
        .output()
        .ok()?;
    if !(output.status.success() || output.status.code() == Some(1)) {
        warn!(
            path = relative_path,
            status = ?output.status.code(),
            stderr = %String::from_utf8_lossy(&output.stderr),
            "Failed to count untracked file diff size with git numstat"
        );
        return None;
    }
    parse_numstat_added_lines(&output.stdout)
}

fn parse_numstat_added_lines(output: &[u8]) -> Option<Option<usize>> {
    let output = String::from_utf8_lossy(output);
    let Some(line) = output.lines().next() else {
        return Some(Some(0));
    };
    let added = line.split('\t').next()?;
    if added == "-" {
        return Some(None);
    }
    Some(Some(added.parse().ok()?))
}

fn run_git(project_root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("git")
        .args(args)
        .current_dir(project_root)
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use tempfile::tempdir;

    use super::*;

    fn run_git_command(project_root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(project_root)
            .output()
            .expect("git command should execute");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn setup_repo() -> tempfile::TempDir {
        let temp = tempdir().expect("tempdir");
        run_git_command(temp.path(), &["init"]);
        temp
    }

    #[test]
    fn counts_regular_file_and_skips_symlink() {
        let repo = setup_repo();
        std::fs::write(repo.path().join("new.txt"), "one\ntwo\nthree\n")
            .expect("write untracked file");
        #[cfg(unix)]
        std::os::unix::fs::symlink("new.txt", repo.path().join("new-link.txt"))
            .expect("create symlink");

        let size =
            collect_with_limit(repo.path(), SCAN_FILE_LIMIT).expect("collect untracked size");

        assert_eq!(size.files, 1);
        assert_eq!(size.changed_lines, 3);
        assert!(size.notes.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn classifies_unix_socket_as_non_regular_without_opening_it() {
        let repo = setup_repo();
        let socket_path = repo.path().join("review-size.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket_path)
            .expect("bind unix socket special file");

        assert_eq!(regular_file_bytes(&socket_path), Some(None));
    }

    #[test]
    fn caps_scanned_files_with_note() {
        let repo = setup_repo();
        std::fs::write(repo.path().join("a.txt"), "one\n").expect("write first untracked file");
        std::fs::write(repo.path().join("b.txt"), "two\n").expect("write second untracked file");

        let size = collect_with_limit(repo.path(), 1).expect("collect untracked size");

        assert_eq!(size.files, 1);
        assert_eq!(size.changed_lines, 1);
        assert_eq!(
            super::super::format_review_diff_size_line(&size),
            "Diff size: 1 files, 1 changed lines, 4 bytes; untracked scan capped at 1 files"
        );
    }
}
