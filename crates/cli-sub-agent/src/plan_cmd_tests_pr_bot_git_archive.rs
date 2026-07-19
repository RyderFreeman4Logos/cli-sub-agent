use crate::test_bounded_command::output_with_timeout;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

pub(super) fn git_archive_entries(repo_root: &Path, pathspec: &str) -> Vec<String> {
    let git_metadata = output_with_timeout(
        {
            let mut command = Command::new("git");
            command
                .args(["rev-parse", "--absolute-git-dir", "--git-common-dir"])
                .current_dir(repo_root);
            command
        },
        Duration::from_secs(30),
    );
    assert!(
        git_metadata.status.success(),
        "git metadata query failed: {}",
        String::from_utf8_lossy(&git_metadata.stderr)
    );
    let metadata = String::from_utf8(git_metadata.stdout).expect("git metadata should be utf-8");
    let mut metadata_lines = metadata.lines();
    let git_dir = PathBuf::from(
        metadata_lines
            .next()
            .expect("absolute Git directory should be reported"),
    );
    let common_dir = PathBuf::from(
        metadata_lines
            .next()
            .expect("common Git directory should be reported"),
    );
    let common_dir = if common_dir.is_absolute() {
        common_dir
    } else {
        repo_root.join(common_dir)
    };
    let real_index = git_dir.join("index");
    let real_objects = common_dir.join("objects");
    let isolated = tempfile::tempdir().expect("isolated Git storage should be created");
    let isolated_index = isolated.path().join("index");
    let isolated_objects = isolated.path().join("objects");
    std::fs::copy(&real_index, &isolated_index).expect("real index should seed isolated index");
    std::fs::create_dir(&isolated_objects).expect("isolated object directory should be created");
    let alternates = std::env::join_paths([&real_objects])
        .expect("real object directory should be representable as a Git alternate");

    let tree = output_with_timeout(
        {
            let mut command = Command::new("git");
            command
                .args(["write-tree"])
                .current_dir(repo_root)
                .env("GIT_INDEX_FILE", &isolated_index)
                .env("GIT_OBJECT_DIRECTORY", &isolated_objects)
                .env("GIT_ALTERNATE_OBJECT_DIRECTORIES", &alternates);
            command
        },
        Duration::from_secs(30),
    );
    assert!(
        tree.status.success(),
        "git write-tree failed: {}",
        String::from_utf8_lossy(&tree.stderr)
    );
    let tree_id = String::from_utf8(tree.stdout)
        .expect("tree id should be utf-8")
        .trim()
        .to_string();

    let archive = output_with_timeout(
        {
            let mut command = Command::new("git");
            command
                .args(["archive", "--format=tar", &tree_id, pathspec])
                .current_dir(repo_root)
                .env("GIT_INDEX_FILE", &isolated_index)
                .env("GIT_OBJECT_DIRECTORY", &isolated_objects)
                .env("GIT_ALTERNATE_OBJECT_DIRECTORIES", &alternates);
            command
        },
        Duration::from_secs(30),
    );
    assert!(
        archive.status.success(),
        "git archive failed: {}",
        String::from_utf8_lossy(&archive.stderr)
    );

    let mut tar = Command::new("tar");
    use std::os::unix::process::CommandExt;
    // SAFETY: only setpgid(0, 0) in the child before exec.
    unsafe {
        tar.pre_exec(|| {
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut tar = tar
        .args(["tf", "-"])
        .current_dir(repo_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("tar should start");
    let tar_pid = tar.id() as i32;
    tar.stdin
        .as_mut()
        .expect("tar stdin")
        .write_all(&archive.stdout)
        .expect("should stream archive into tar");
    drop(tar.stdin.take());
    let listing = {
        use std::sync::mpsc;
        use std::thread;
        use std::time::Instant;
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(tar.wait_with_output());
        });
        match rx.recv_timeout(Duration::from_secs(30)) {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => panic!("tar wait failed: {error}"),
            Err(_) => {
                // SAFETY: tar_pid is the child we just spawned into its own group.
                unsafe {
                    let _ = libc::kill(-tar_pid, libc::SIGTERM);
                }
                thread::sleep(Duration::from_millis(50));
                unsafe {
                    let _ = libc::kill(-tar_pid, libc::SIGKILL);
                }
                let deadline = Instant::now() + Duration::from_secs(2);
                loop {
                    match rx.recv_timeout(Duration::from_millis(50)) {
                        Ok(Ok(_)) | Ok(Err(_)) => break,
                        Err(_) if Instant::now() < deadline => continue,
                        Err(_) => break,
                    }
                }
                panic!("tar listing exceeded 30s");
            }
        }
    };
    assert!(
        listing.status.success(),
        "tar listing failed: {}",
        String::from_utf8_lossy(&listing.stderr)
    );
    String::from_utf8(listing.stdout)
        .expect("tar output should be utf-8")
        .lines()
        .map(ToOwned::to_owned)
        .collect()
}
