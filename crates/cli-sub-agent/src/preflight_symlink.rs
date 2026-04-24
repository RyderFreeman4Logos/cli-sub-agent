use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use csa_config::AiConfigSymlinkCheckConfig;

const DEFAULT_AI_CONFIG_SYMLINK_PATHS: &[&str] = &[
    "AGENTS.md",
    "GEMINI.md",
    "CLAUDE.md",
    ".claude/rules/AGENTS.md",
    ".agents/project-rules-ref",
    ".agents/rules-ref",
];

pub fn run_ai_config_symlink_check(
    project_root: &Path,
    config: &AiConfigSymlinkCheckConfig,
) -> Result<()> {
    if !config.enabled {
        return Ok(());
    }

    let violations: Vec<String> = if let Some(configured_paths) = config.paths.as_ref() {
        configured_paths
            .iter()
            .filter_map(|relative_path| validate_one_path(project_root, relative_path, config))
            .collect()
    } else {
        DEFAULT_AI_CONFIG_SYMLINK_PATHS
            .iter()
            .filter_map(|relative_path| validate_one_path(project_root, relative_path, config))
            .collect()
    };

    if violations.is_empty() {
        return Ok(());
    }

    let mut message = String::from("preflight: AI-config symlink integrity check failed\n");
    for violation in violations {
        message.push_str("  ");
        message.push_str(&violation);
        message.push('\n');
    }
    message.push_str(
        "\nThese paths must be symlinks into your rules/drafts repo (see AGENTS.md Rule 036).\n\
Each violation above includes the targeted fix; use `ln -sf <target> <path>` only\n\
when the symlink itself points to the wrong location.\n\n\
To disable this check, set `preflight.ai_config_symlink_check.enabled = false` in\n\
~/.config/cli-sub-agent/config.toml or the project .csa/config.toml.",
    );

    Err(anyhow!(message))
}

fn validate_one_path(
    project_root: &Path,
    relative_path: &str,
    config: &AiConfigSymlinkCheckConfig,
) -> Option<String> {
    let full_path = project_root.join(relative_path);
    let metadata = match fs::symlink_metadata(&full_path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(err) => {
            return Some(format!("{relative_path:<32} failed to inspect path: {err}"));
        }
    };

    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        if !config.treat_broken_symlink_as_error {
            return None;
        }

        let expects_directory = expects_directory_target(relative_path);
        match fs::metadata(&full_path) {
            Ok(target_metadata) => {
                if expects_directory && target_metadata.is_file() {
                    let resolved_target = describe_symlink_target(&full_path);
                    return Some(format!(
                        "{relative_path:<32} points to regular file '{resolved_target}', expected directory target; choose a directory target before recreating the symlink"
                    ));
                }

                if !expects_directory && target_metadata.is_dir() {
                    let resolved_target = describe_symlink_target(&full_path);
                    return Some(format!(
                        "{relative_path:<32} points to directory '{resolved_target}', expected file target; choose a file target before recreating the symlink"
                    ));
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let resolved_target = match read_resolved_symlink_target(&full_path) {
                    Ok(target) => target,
                    Err(target) => {
                        return Some(format!(
                            "{relative_path:<32} broken symlink: target {target} does not exist"
                        ));
                    }
                };

                if expects_directory {
                    if try_auto_heal_missing_target_dir(relative_path, &full_path, &resolved_target)
                    {
                        return None;
                    }

                    return Some(format!(
                        "{relative_path:<32} broken symlink: target directory '{}' does not exist; create it with: {}",
                        resolved_target.display(),
                        mkdir_command(&resolved_target)
                    ));
                }

                return Some(format!(
                    "{relative_path:<32} broken symlink: target file '{}' does not exist",
                    resolved_target.display()
                ));
            }
            Err(err) => {
                return Some(format!(
                    "{relative_path:<32} failed to inspect symlink target: {err}"
                ));
            }
        }
        return None;
    }

    if file_type.is_file() {
        return Some(format!(
            "{relative_path:<32} is a regular file, expected symlink"
        ));
    }

    if file_type.is_dir() {
        return Some(format!(
            "{relative_path:<32} is a regular directory, expected symlink"
        ));
    }

    Some(format!(
        "{relative_path:<32} is not a symlink, expected symlink"
    ))
}

fn describe_symlink_target(path: &Path) -> String {
    match read_resolved_symlink_target(path) {
        Ok(target) => target.display().to_string(),
        Err(target) => target,
    }
}

fn read_resolved_symlink_target(path: &Path) -> std::result::Result<PathBuf, String> {
    match fs::read_link(path) {
        Ok(target) => Ok(resolve_symlink_target(path, &target)),
        Err(err) => Err(format!("<unreadable symlink target: {err}>")),
    }
}

fn resolve_symlink_target(link_path: &Path, target: &Path) -> PathBuf {
    if target.is_absolute() {
        return target.to_path_buf();
    }

    link_path
        .parent()
        .map(|parent| parent.join(target))
        .unwrap_or_else(|| target.to_path_buf())
}

fn try_auto_heal_missing_target_dir(
    relative_path: &str,
    link_path: &Path,
    resolved_target: &Path,
) -> bool {
    if !expects_directory_target(relative_path) {
        return false;
    }

    let Some(parent) = resolved_target.parent() else {
        return false;
    };
    if !is_writable_directory(parent) {
        return false;
    }

    match fs::create_dir_all(resolved_target) {
        Ok(()) => {
            tracing::info!(
                link_path = %link_path.display(),
                target = %resolved_target.display(),
                "auto-healed missing AI-config symlink target directory"
            );
            true
        }
        Err(err) => {
            tracing::debug!(
                link_path = %link_path.display(),
                target = %resolved_target.display(),
                error = %err,
                "failed to auto-heal missing AI-config symlink target directory"
            );
            false
        }
    }
}

fn expects_directory_target(relative_path: &str) -> bool {
    let path = Path::new(relative_path);
    path.extension().is_none()
        || path.extension().and_then(|extension| extension.to_str()) == Some("d")
}

fn is_writable_directory(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    metadata.is_dir() && directory_permissions_allow_write(&metadata)
}

#[cfg(unix)]
fn directory_permissions_allow_write(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;

    metadata.permissions().mode() & 0o222 != 0
}

#[cfg(not(unix))]
fn directory_permissions_allow_write(metadata: &fs::Metadata) -> bool {
    !metadata.permissions().readonly()
}

fn mkdir_command(target: &Path) -> String {
    format!("mkdir -p {}", shell_quote_path(target))
}

fn shell_quote_path(path: &Path) -> String {
    let value = path.display().to_string();
    if value.is_empty() {
        return "''".to_string();
    }

    if value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-' | b'+')
    }) {
        return value;
    }

    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_never_fails() {
        let d = tempfile::tempdir().unwrap();
        let cfg = AiConfigSymlinkCheckConfig {
            enabled: false,
            ..Default::default()
        };
        run_ai_config_symlink_check(d.path(), &cfg).expect("disabled should return Ok");
    }

    #[test]
    fn enabled_with_no_paths_present_returns_ok() {
        let d = tempfile::tempdir().unwrap();
        let cfg = AiConfigSymlinkCheckConfig {
            enabled: true,
            ..Default::default()
        };
        run_ai_config_symlink_check(d.path(), &cfg).expect("missing files are ok");
    }

    #[test]
    fn regular_file_at_ai_config_path_is_violation() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("AGENTS.md"), "content").unwrap();
        let cfg = AiConfigSymlinkCheckConfig {
            enabled: true,
            ..Default::default()
        };
        let err = run_ai_config_symlink_check(d.path(), &cfg).expect_err("should fail");
        assert!(err.to_string().contains("AGENTS.md"), "got: {err}");
        assert!(err.to_string().contains("regular file"), "got: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn valid_symlink_passes() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("real.md");
        std::fs::write(&target, "content").unwrap();
        std::os::unix::fs::symlink(&target, d.path().join("AGENTS.md")).unwrap();
        let cfg = AiConfigSymlinkCheckConfig {
            enabled: true,
            ..Default::default()
        };
        run_ai_config_symlink_check(d.path(), &cfg).expect("valid symlink ok");
    }

    #[cfg(unix)]
    #[test]
    fn broken_symlink_fails_when_treated_as_error() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("missing-parent").join("nowhere");
        std::os::unix::fs::symlink(&target, d.path().join("AGENTS.md")).unwrap();
        let cfg = AiConfigSymlinkCheckConfig {
            enabled: true,
            ..Default::default()
        };
        let err = run_ai_config_symlink_check(d.path(), &cfg).expect_err("broken -> err");
        assert!(err.to_string().contains("broken symlink"), "got: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn auto_heals_missing_target_dir_when_parent_exists() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("links")).unwrap();
        std::fs::create_dir_all(d.path().join("subdir")).unwrap();
        std::os::unix::fs::symlink("../subdir/target_dir", d.path().join("links/foo.d")).unwrap();
        let cfg = AiConfigSymlinkCheckConfig {
            enabled: true,
            paths: Some(vec!["links/foo.d".to_string()]),
            ..Default::default()
        };

        run_ai_config_symlink_check(d.path(), &cfg).expect("missing target dir should heal");

        assert!(d.path().join("subdir/target_dir").is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn reports_mkdir_suggestion_when_parent_missing() {
        let d = tempfile::tempdir().unwrap();
        let target = d
            .path()
            .join("nonexistent")
            .join("deeply")
            .join("nested")
            .join("target");
        std::os::unix::fs::symlink(&target, d.path().join("foo.d")).unwrap();
        let cfg = AiConfigSymlinkCheckConfig {
            enabled: true,
            paths: Some(vec!["foo.d".to_string()]),
            ..Default::default()
        };

        let err = run_ai_config_symlink_check(d.path(), &cfg).expect_err("broken -> err");

        assert!(err.to_string().contains("mkdir -p"), "got: {err}");
        assert!(
            err.to_string().contains(&target.display().to_string()),
            "got: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn file_target_broken_symlink_does_not_suggest_mkdir() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("missing.md");
        std::os::unix::fs::symlink(&target, d.path().join("AGENTS.md")).unwrap();
        let cfg = AiConfigSymlinkCheckConfig {
            enabled: true,
            ..Default::default()
        };

        let err = run_ai_config_symlink_check(d.path(), &cfg).expect_err("broken -> err");

        assert!(err.to_string().contains("target file"), "got: {err}");
        assert!(!err.to_string().contains("mkdir -p"), "got: {err}");
        assert!(!err.to_string().contains("target directory"), "got: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn file_target_broken_symlink_is_not_auto_healed() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("missing.md");
        std::os::unix::fs::symlink(&target, d.path().join("AGENTS.md")).unwrap();
        let cfg = AiConfigSymlinkCheckConfig {
            enabled: true,
            ..Default::default()
        };

        let err = run_ai_config_symlink_check(d.path(), &cfg).expect_err("broken -> err");

        assert!(err.to_string().contains("target file"), "got: {err}");
        assert!(
            !target.exists(),
            "file-target symlink must not create a directory at the missing file path"
        );
    }

    #[cfg(unix)]
    #[test]
    fn directory_target_pointing_to_file_is_violation() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("rules-file");
        std::fs::write(&target, "not a directory").unwrap();
        std::fs::create_dir_all(d.path().join(".agents")).unwrap();
        std::os::unix::fs::symlink(&target, d.path().join(".agents/rules-ref")).unwrap();
        let cfg = AiConfigSymlinkCheckConfig {
            enabled: true,
            ..Default::default()
        };

        let err = run_ai_config_symlink_check(d.path(), &cfg).expect_err("file target -> err");

        assert!(
            err.to_string().contains("expected directory target"),
            "got: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn file_target_pointing_to_directory_is_violation() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("rules-dir");
        std::fs::create_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(&target, d.path().join("AGENTS.md")).unwrap();
        let cfg = AiConfigSymlinkCheckConfig {
            enabled: true,
            ..Default::default()
        };

        let err = run_ai_config_symlink_check(d.path(), &cfg).expect_err("dir target -> err");

        assert!(
            err.to_string().contains("expected file target"),
            "got: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn broken_symlink_passes_when_configured_to_ignore() {
        let d = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/nonexistent", d.path().join("AGENTS.md")).unwrap();
        let cfg = AiConfigSymlinkCheckConfig {
            enabled: true,
            treat_broken_symlink_as_error: false,
            ..Default::default()
        };
        run_ai_config_symlink_check(d.path(), &cfg).expect("ignored broken -> ok");
    }

    #[test]
    fn custom_path_list_overrides_defaults() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("CUSTOM.md"), "content").unwrap();
        let cfg = AiConfigSymlinkCheckConfig {
            enabled: true,
            paths: Some(vec!["CUSTOM.md".to_string()]),
            ..Default::default()
        };
        let err = run_ai_config_symlink_check(d.path(), &cfg).expect_err("custom path flagged");
        assert!(err.to_string().contains("CUSTOM.md"), "got: {err}");
        assert!(
            !err.to_string()
                .contains("AGENTS.md                        is "),
            "got: {err}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn disabled_returns_ok_on_windows() {
        let d = tempfile::tempdir().unwrap();
        let cfg = AiConfigSymlinkCheckConfig {
            enabled: false,
            ..Default::default()
        };
        run_ai_config_symlink_check(d.path(), &cfg).expect("disabled should return Ok");
    }
}
