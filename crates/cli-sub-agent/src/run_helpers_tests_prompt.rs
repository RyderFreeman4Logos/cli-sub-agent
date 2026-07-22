// --- resolve_prompt_with_file tests ---

#[test]
fn resolve_prompt_with_file_reads_file_content() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), "prompt from file").unwrap();
    let result = super::resolve_prompt_with_file(None, Some(tmp.path())).unwrap();
    assert_eq!(result, "prompt from file");
}

#[test]
fn resolve_prompt_with_file_routes_dash_to_stdin() {
    let mut stdin = std::io::Cursor::new("prompt from stdin");
    let result = super::resolve_prompt_with_file_from_reader(
        Some("positional".to_string()),
        Some(std::path::Path::new("-")),
        false,
        &mut stdin,
    )
    .unwrap();
    assert_eq!(result, "prompt from stdin");
}

#[test]
fn resolve_prompt_with_file_routes_dev_stdin_to_stdin() {
    let mut stdin = std::io::Cursor::new("prompt from dev stdin");
    let result = super::resolve_prompt_with_file_from_reader(
        Some("positional".to_string()),
        Some(std::path::Path::new("/dev/stdin")),
        false,
        &mut stdin,
    )
    .unwrap();
    assert_eq!(result, "prompt from dev stdin");
}

#[test]
fn resolve_prompt_with_file_real_path_unchanged() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), "real file content").unwrap();
    let mut stdin = std::io::Cursor::new("stdin should not win");
    let result = super::resolve_prompt_with_file_from_reader(
        Some("positional".to_string()),
        Some(tmp.path()),
        false,
        &mut stdin,
    )
    .unwrap();
    assert_eq!(result, "real file content");
}

#[test]
fn resolve_prompt_with_file_empty_stdin_sentinel_bails() {
    let mut stdin = std::io::Cursor::new("   ");
    let result = super::resolve_prompt_with_file_from_reader(
        None,
        Some(std::path::Path::new("-")),
        false,
        &mut stdin,
    );
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Empty prompt from stdin")
    );
}

#[test]
fn resolve_prompt_with_file_overrides_positional() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), "file wins").unwrap();
    let result =
        super::resolve_prompt_with_file(Some("positional".to_string()), Some(tmp.path())).unwrap();
    assert_eq!(result, "file wins");
}

#[test]
fn resolve_prompt_with_file_rejects_empty_file() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), "   ").unwrap();
    let result = super::resolve_prompt_with_file(None, Some(tmp.path()));
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("empty"));
}

#[test]
fn resolve_prompt_with_file_rejects_missing_file() {
    let path = std::path::Path::new("/tmp/csa-nonexistent-prompt-file-test.md");
    let result = super::resolve_prompt_with_file(None, Some(path));
    assert!(result.is_err());
    let message = result.unwrap_err().to_string();
    assert!(
        message.contains("--prompt-file")
            && (message.contains("not found") || message.contains("unreadable")),
        "{message}"
    );
}

#[cfg(unix)]
#[test]
fn validate_prompt_file_rejects_nonexistent_path_beyond_repo_symlink() {
    use std::os::unix::fs::symlink;
    use std::process::Command;

    let root = tempfile::tempdir().expect("tempdir");
    let outside = root.path().join("outside");
    let repo = root.path().join("repo");
    std::fs::create_dir_all(outside.join("verbatim")).expect("outside");
    std::fs::create_dir_all(&repo).expect("repo");
    std::fs::write(outside.join("verbatim/issue.md"), "prompt body\n").expect("write prompt");
    symlink(outside.join("verbatim"), repo.join("drafts")).expect("symlink drafts");

    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(&repo)
            .status()
            .expect("git init")
            .success()
    );

    let beyond = repo.join("drafts/verbatim/issue.md");
    // Git pathspec would fatal with "beyond a symbolic link"; filesystem
    // validation must fail first with a targeted prompt-file error.
    let err = super::validate_prompt_file_path(Some(&beyond))
        .expect_err("path beyond symlink component must fail");
    let message = format!("{err:#}");
    assert!(
        message.contains("--prompt-file")
            && message.contains("not found or unreadable")
            && !message.contains("beyond a symbolic link")
            && !message.contains("pathspec"),
        "{message}"
    );
}

#[cfg(unix)]
#[test]
fn validate_prompt_file_accepts_canonical_external_and_symlink_ok_paths() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().expect("tempdir");
    let outside = root.path().join("outside");
    let repo = root.path().join("repo");
    std::fs::create_dir_all(outside.join("verbatim")).expect("outside");
    std::fs::create_dir_all(&repo).expect("repo");
    let external = outside.join("verbatim/issue.md");
    std::fs::write(&external, "prompt body\n").expect("write prompt");
    symlink(outside.join("verbatim"), repo.join("drafts")).expect("symlink drafts");

    super::validate_prompt_file_path(Some(&external))
        .expect("canonical external prompt file must be accepted");
    let through_symlink = repo.join("drafts/issue.md");
    super::validate_prompt_file_path(Some(&through_symlink))
        .expect("readable file through allowed symlink must be accepted");

    let content = super::resolve_prompt_with_file(None, Some(&through_symlink))
        .expect("symlink-ok prompt must be readable");
    assert_eq!(content, "prompt body\n");
}

#[test]
fn resolve_prompt_with_file_falls_through_to_positional() {
    let result = super::resolve_prompt_with_file(Some("hello".to_string()), None).unwrap();
    assert_eq!(result, "hello");
}

#[test]
fn resolve_positional_stdin_sentinel_preserves_non_sentinel_prompt() {
    let result =
        super::resolve_positional_stdin_sentinel(Some("literal prompt".to_string())).unwrap();
    assert_eq!(result, Some("literal prompt".to_string()));
}

#[test]
fn resolve_positional_stdin_sentinel_reads_from_stdin_for_dash() {
    let mut stdin = std::io::Cursor::new("prompt from stdin");
    let result = super::prompt::resolve_positional_stdin_sentinel_from_reader(
        Some("-".to_string()),
        false,
        &mut stdin,
    )
    .unwrap();
    assert_eq!(result, Some("prompt from stdin".to_string()));
}
