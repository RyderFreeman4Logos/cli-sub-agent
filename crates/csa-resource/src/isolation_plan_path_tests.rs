use super::*;

#[test]
fn test_resolve_writable_relative_path_against_project_root() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let drafts = project.join("drafts");
    std::fs::create_dir_all(&drafts).expect("create drafts dir");

    let resolved =
        resolve_writable_paths(&[PathBuf::from("./drafts")], &project).expect("valid path");

    assert_eq!(resolved, vec![drafts.canonicalize().unwrap()]);
}

#[cfg(unix)]
#[test]
fn test_resolve_writable_symlink_inside_project_to_external_target() {
    use std::os::unix::fs::symlink;

    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    let external = tmp.path().join("external-drafts");
    std::fs::create_dir_all(&project).expect("create project dir");
    std::fs::create_dir_all(&external).expect("create external dir");
    symlink(&external, project.join("drafts")).expect("create symlink");

    let resolved = resolve_writable_paths(&[PathBuf::from("drafts")], &project)
        .expect("project-local symlink should be accepted");

    let canonical_project = project.canonicalize().unwrap();
    let canonical_external = external.canonicalize().unwrap();
    assert_eq!(resolved, vec![canonical_external.clone()]);
    assert!(!canonical_external.starts_with(canonical_project));
}

#[test]
fn test_resolve_writable_nonexistent_path_uses_existing_parent() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).expect("create project dir");

    let resolved =
        resolve_writable_paths(&[PathBuf::from("drafts/new")], &project).expect("valid path");

    let expected = project.canonicalize().unwrap().join("drafts/new");
    assert_eq!(resolved, vec![expected.clone()]);
    assert!(!expected.exists());
}

#[test]
fn test_resolve_writable_accepts_config_path_outside_default_roots() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).expect("create project dir");

    let resolved = resolve_writable_paths(&[PathBuf::from("/opt/data")], &project)
        .expect("config extra_writable outside default roots should be accepted");

    assert_eq!(resolved, vec![PathBuf::from("/opt/data")]);
}

#[cfg(unix)]
#[test]
fn test_writable_validation_error_includes_original_and_resolved_path() {
    use std::os::unix::fs::symlink;

    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).expect("create project dir");
    symlink("/etc", project.join("etc-link")).expect("create symlink");

    let err = resolve_writable_paths(&[PathBuf::from("etc-link")], &project)
        .expect_err("sensitive symlink target should be rejected")
        .to_string();

    assert!(err.contains("etc-link"), "missing original path: {err}");
    assert!(
        err.contains("resolved path /etc is forbidden"),
        "missing resolved path: {err}"
    );
}
