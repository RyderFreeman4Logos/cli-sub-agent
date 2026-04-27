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
    assert!(result.unwrap_err().to_string().contains("--prompt-file"));
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
