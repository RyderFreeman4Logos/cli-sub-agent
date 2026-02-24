use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::path::Path;
use std::str::FromStr;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub enum ProjectProfile {
    Rust,
    Node,
    Python,
    Go,
    Mixed,
    #[default]
    Unknown,
}

impl Display for ProjectProfile {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::Rust => "rust",
            Self::Node => "node",
            Self::Python => "python",
            Self::Go => "go",
            Self::Mixed => "mixed",
            Self::Unknown => "unknown",
        };
        write!(f, "{value}")
    }
}

impl FromStr for ProjectProfile {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "rust" => Ok(Self::Rust),
            "node" => Ok(Self::Node),
            "python" => Ok(Self::Python),
            "go" => Ok(Self::Go),
            "mixed" => Ok(Self::Mixed),
            "unknown" => Ok(Self::Unknown),
            _ => Err(()),
        }
    }
}

fn has_non_symlink_file(project_root: &Path, file_name: &str) -> bool {
    let path = project_root.join(file_name);
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => !metadata.file_type().is_symlink() && metadata.is_file(),
        Err(_) => false,
    }
}

pub fn detect_project_profile(project_root: &Path) -> ProjectProfile {
    let has_rust =
        has_non_symlink_file(project_root, "Cargo.toml") || has_non_symlink_file(project_root, "Cargo.lock");
    let has_node = has_non_symlink_file(project_root, "package.json")
        || has_non_symlink_file(project_root, "package-lock.json")
        || has_non_symlink_file(project_root, "yarn.lock")
        || has_non_symlink_file(project_root, "pnpm-lock.yaml");
    let has_python = has_non_symlink_file(project_root, "pyproject.toml")
        || has_non_symlink_file(project_root, "requirements.txt")
        || has_non_symlink_file(project_root, "poetry.lock")
        || has_non_symlink_file(project_root, "Pipfile");
    let has_go =
        has_non_symlink_file(project_root, "go.mod") || has_non_symlink_file(project_root, "go.sum");

    let detected_count = [has_rust, has_node, has_python, has_go]
        .into_iter()
        .filter(|detected| *detected)
        .count();

    if detected_count > 1 {
        return ProjectProfile::Mixed;
    }
    if has_rust {
        return ProjectProfile::Rust;
    }
    if has_node {
        return ProjectProfile::Node;
    }
    if has_python {
        return ProjectProfile::Python;
    }
    if has_go {
        return ProjectProfile::Go;
    }

    ProjectProfile::Unknown
}

pub fn detect_project_profile_with_override(
    project_root: &Path,
    config_override: Option<&str>,
) -> ProjectProfile {
    if let Some(override_value) = config_override {
        if let Ok(parsed) = ProjectProfile::from_str(override_value) {
            return parsed;
        }
    }

    detect_project_profile(project_root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_detect_project_profile_rust_with_cargo_toml() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();

        assert_eq!(detect_project_profile(dir.path()), ProjectProfile::Rust);
    }

    #[test]
    fn test_detect_project_profile_node_with_package_json() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), "{ \"name\": \"demo\" }").unwrap();

        assert_eq!(detect_project_profile(dir.path()), ProjectProfile::Node);
    }

    #[test]
    fn test_detect_project_profile_go_with_go_mod() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("go.mod"), "module example.com/demo").unwrap();

        assert_eq!(detect_project_profile(dir.path()), ProjectProfile::Go);
    }

    #[test]
    fn test_detect_project_profile_python_with_pyproject_toml() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        assert_eq!(detect_project_profile(dir.path()), ProjectProfile::Python);
    }

    #[test]
    fn test_detect_project_profile_mixed_with_rust_and_node() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
        std::fs::write(dir.path().join("package.json"), "{ \"name\": \"demo\" }").unwrap();

        assert_eq!(detect_project_profile(dir.path()), ProjectProfile::Mixed);
    }

    #[test]
    fn test_detect_project_profile_unknown_with_empty_dir() {
        let dir = tempdir().unwrap();

        assert_eq!(detect_project_profile(dir.path()), ProjectProfile::Unknown);
    }

    #[test]
    fn test_detect_project_profile_with_override_rust_case_insensitive() {
        let dir = tempdir().unwrap();

        assert_eq!(
            detect_project_profile_with_override(dir.path(), Some("RuSt")),
            ProjectProfile::Rust
        );
    }

    #[test]
    fn test_detect_project_profile_with_override_invalid_falls_back_to_detection() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), "{ \"name\": \"demo\" }").unwrap();

        assert_eq!(
            detect_project_profile_with_override(dir.path(), Some("invalid")),
            ProjectProfile::Node
        );
    }

    #[test]
    fn test_project_profile_display_outputs_lowercase() {
        assert_eq!(ProjectProfile::Rust.to_string(), "rust");
        assert_eq!(ProjectProfile::Node.to_string(), "node");
        assert_eq!(ProjectProfile::Python.to_string(), "python");
        assert_eq!(ProjectProfile::Go.to_string(), "go");
        assert_eq!(ProjectProfile::Mixed.to_string(), "mixed");
        assert_eq!(ProjectProfile::Unknown.to_string(), "unknown");
    }
}
