//! ULID validation and prefix matching

use anyhow::{bail, Context, Result};
use std::path::Path;

/// Generate a new ULID session ID
pub fn new_session_id() -> String {
    ulid::Ulid::new().to_string()
}

/// Validate that a string is a valid ULID
pub fn validate_session_id(id: &str) -> Result<()> {
    if id.len() != 26 {
        bail!(
            "Invalid session ID '{}': expected 26 characters, got {}",
            id,
            id.len()
        );
    }

    // Try to parse as ULID
    ulid::Ulid::from_string(id)
        .with_context(|| format!("Invalid session ID '{}': not a valid ULID", id))?;

    Ok(())
}

/// Resolve a session ID prefix to a full session ID
///
/// Scans the sessions directory and finds sessions matching the prefix.
/// Returns an error if 0 or more than 1 match is found.
pub fn resolve_session_prefix(sessions_dir: &Path, prefix: &str) -> Result<String> {
    if !sessions_dir.exists() {
        bail!("No session matching prefix '{}'", prefix);
    }

    let entries = std::fs::read_dir(sessions_dir).with_context(|| {
        format!(
            "Failed to read sessions directory: {}",
            sessions_dir.display()
        )
    })?;

    let mut matches = Vec::new();

    for entry in entries {
        let entry = entry.context("Failed to read directory entry")?;
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();

        if name.to_uppercase().starts_with(&prefix.to_uppercase()) {
            matches.push(name.to_string());
        }
    }

    match matches.len() {
        0 => bail!("No session matching prefix '{}'", prefix),
        1 => Ok(matches.into_iter().next().unwrap()),
        _ => bail!(
            "Ambiguous session prefix '{}': matches multiple sessions: {}",
            prefix,
            matches.join(", ")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_new_session_id_format() {
        let id = new_session_id();
        assert_eq!(id.len(), 26, "ULID should be 26 characters");
        assert!(
            validate_session_id(&id).is_ok(),
            "Generated ID should be valid"
        );
    }

    #[test]
    fn test_validate_valid_ulid() {
        let id = new_session_id();
        assert!(validate_session_id(&id).is_ok());
    }

    #[test]
    fn test_validate_invalid_length() {
        let result = validate_session_id("too-short");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("expected 26 characters"));
    }

    #[test]
    fn test_validate_invalid_format() {
        let result = validate_session_id("!!!!!!!!!!!!!!!!!!!!!!!!!!"); // 26 chars but invalid
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not a valid ULID"));
    }

    #[test]
    fn test_resolve_prefix_unique_match() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let sessions_dir = temp_dir.path();

        // Create session directories
        std::fs::create_dir_all(sessions_dir.join("01HY7ABCDEFGHIJKLMNOPQRSTU")).unwrap();
        std::fs::create_dir_all(sessions_dir.join("01HY8XYZABCDEFGHIJKLMNOPQR")).unwrap();

        let result = resolve_session_prefix(sessions_dir, "01HY7");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "01HY7ABCDEFGHIJKLMNOPQRSTU");
    }

    #[test]
    fn test_resolve_prefix_no_match() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let sessions_dir = temp_dir.path();

        std::fs::create_dir_all(sessions_dir.join("01HY7ABCDEFGHIJKLMNOPQRSTU")).unwrap();

        let result = resolve_session_prefix(sessions_dir, "01HZ");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("No session matching prefix"));
    }

    #[test]
    fn test_resolve_prefix_ambiguous() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let sessions_dir = temp_dir.path();

        std::fs::create_dir_all(sessions_dir.join("01HY7AAAAAAAAAAAAAAAAAAAAA")).unwrap();
        std::fs::create_dir_all(sessions_dir.join("01HY7BBBBBBBBBBBBBBBBBBBBBB")).unwrap();

        let result = resolve_session_prefix(sessions_dir, "01HY7");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Ambiguous session prefix"));
    }

    #[test]
    fn test_resolve_prefix_nonexistent_dir() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let sessions_dir = temp_dir.path().join("nonexistent");

        let result = resolve_session_prefix(&sessions_dir, "01HY7");
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_prefix_case_insensitive() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let sessions_dir = temp_dir.path();

        std::fs::create_dir_all(sessions_dir.join("01HY7ABCDEFGHIJKLMNOPQRSTU")).unwrap();

        // Lowercase prefix should match uppercase directory
        let result = resolve_session_prefix(sessions_dir, "01hy7");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "01HY7ABCDEFGHIJKLMNOPQRSTU");
    }

    #[test]
    fn test_resolve_prefix_mixed_case() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let sessions_dir = temp_dir.path();

        std::fs::create_dir_all(sessions_dir.join("01HY7ABCDEFGHIJKLMNOPQRSTU")).unwrap();

        // Mixed case should also work
        let result = resolve_session_prefix(sessions_dir, "01Hy7a");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "01HY7ABCDEFGHIJKLMNOPQRSTU");
    }
}
