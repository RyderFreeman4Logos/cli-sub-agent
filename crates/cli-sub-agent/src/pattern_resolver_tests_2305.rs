use std::path::Path;

use super::*;
use tempfile::TempDir;

#[test]
fn resolve_pattern_falls_back_to_bundled_mktd_without_repo_local_pattern() {
    let tmp = TempDir::new().unwrap();
    let materialized = TempDir::new().unwrap();

    let resolved =
        resolve_pattern_with_materialization_root("mktd", tmp.path(), Some(materialized.path()))
            .unwrap();

    assert!(resolved.skill_md.contains("mktd: Make TODO"));
    assert!(resolved.dir.starts_with(materialized.path()));
    assert!(resolved.dir.join("workflow.toml").is_file());
    assert!(resolved.dir.join("skills/mktd/SKILL.md").is_file());
    assert!(!tmp.path().join(".csa").exists());
    assert!(!tmp.path().join("patterns").exists());
}

#[test]
fn materialize_bundled_mktd_writes_full_files_without_temp_residue() {
    let materialized = TempDir::new().unwrap();
    let dest =
        materialize_bundled_pattern("mktd", &BUNDLED_MKTD_PATTERN, Some(materialized.path()))
            .unwrap();

    for file in BUNDLED_MKTD_PATTERN.files {
        let path = dest.join(file.path);
        assert_eq!(std::fs::read(&path).unwrap(), file.contents);
    }

    assert_no_temp_files(&dest);
}

fn assert_no_temp_files(dir: &Path) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            assert_no_temp_files(&path);
        } else {
            assert!(
                !path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains(".tmp."),
                "temporary bundled file was left behind: {}",
                path.display()
            );
        }
    }
}
