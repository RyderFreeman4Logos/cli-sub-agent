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
Fix by re-creating the symlinks:\n  ln -sf <target> <path>\n\n\
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

        if let Err(err) = fs::metadata(&full_path) {
            if err.kind() == std::io::ErrorKind::NotFound {
                let resolved_target = describe_symlink_target(&full_path);
                return Some(format!(
                    "{relative_path:<32} broken symlink: target '{resolved_target}' does not exist"
                ));
            }
            return Some(format!(
                "{relative_path:<32} failed to inspect symlink target: {err}"
            ));
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
    match fs::read_link(path) {
        Ok(target) => resolve_symlink_target(path, &target).display().to_string(),
        Err(err) => format!("<unreadable symlink target: {err}>"),
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
        std::os::unix::fs::symlink("/nonexistent/nowhere", d.path().join("AGENTS.md")).unwrap();
        let cfg = AiConfigSymlinkCheckConfig {
            enabled: true,
            ..Default::default()
        };
        let err = run_ai_config_symlink_check(d.path(), &cfg).expect_err("broken -> err");
        assert!(err.to_string().contains("broken symlink"), "got: {err}");
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
