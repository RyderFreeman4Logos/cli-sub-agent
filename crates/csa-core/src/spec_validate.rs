use std::path::{Path, PathBuf};

#[derive(thiserror::Error, Debug)]
pub enum SpecValidationError {
    #[error("Spec path contains null byte: rejected for security")]
    NullByte,

    #[error("Spec path must use .toml or .spec extension: {path}")]
    InvalidExtension { path: PathBuf },

    #[error("Spec path is not a readable file: {path}")]
    NotAFile { path: PathBuf },

    #[error("Failed to read spec file {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub type Result<T> = std::result::Result<T, SpecValidationError>;

pub fn validate_spec(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().to_string_lossy().contains('\0') {
        return Err(SpecValidationError::NullByte);
    }

    let normalized = path.to_path_buf();
    if !has_spec_extension(path) {
        return Err(SpecValidationError::InvalidExtension { path: normalized });
    }

    if !path.is_file() {
        return Err(SpecValidationError::NotAFile { path: normalized });
    }

    std::fs::File::open(path).map_err(|source| SpecValidationError::Read {
        path: normalized.clone(),
        source,
    })?;

    Ok(normalized)
}

fn has_spec_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("toml") || ext.eq_ignore_ascii_case("spec"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_toml_and_spec_files() {
        let temp = tempfile::tempdir().unwrap();
        let toml_path = temp.path().join("contract.toml");
        let spec_path = temp.path().join("contract.spec");
        std::fs::write(&toml_path, "summary = \"ok\"\n").unwrap();
        std::fs::write(&spec_path, "summary = \"ok\"\n").unwrap();

        assert_eq!(validate_spec(&toml_path).unwrap(), toml_path);
        assert_eq!(validate_spec(&spec_path).unwrap(), spec_path);
    }

    #[test]
    fn rejects_unknown_extension() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("contract.md");
        std::fs::write(&path, "# Contract\n").unwrap();

        let err = validate_spec(&path).unwrap_err();

        assert!(matches!(err, SpecValidationError::InvalidExtension { .. }));
    }

    #[test]
    fn rejects_missing_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("missing.toml");

        let err = validate_spec(&path).unwrap_err();

        assert!(matches!(err, SpecValidationError::NotAFile { .. }));
    }

    #[test]
    fn rejects_directory() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("contract.toml");
        std::fs::create_dir(&path).unwrap();

        let err = validate_spec(&path).unwrap_err();

        assert!(matches!(err, SpecValidationError::NotAFile { .. }));
    }
}
