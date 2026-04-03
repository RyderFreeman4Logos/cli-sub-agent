//! Integration tests for VcsIdentity across Git-only, colocated jj+git, and session round-trip.

use crate::state::MetaSessionState;
use csa_core::vcs::VcsKind;

// ── Git-only tests ──────────────────────────────────────────────

#[test]
fn test_git_backend_identity_fills_commit_id_and_ref_name() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path();

    // Initialize a real git repo with a commit
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(project)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(project)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(project)
        .output()
        .unwrap();
    std::fs::write(project.join("README.md"), "test").unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(project)
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(project)
        .output()
        .unwrap();

    let backend = crate::vcs_backends::create_vcs_backend(project);
    assert_eq!(backend.kind(), VcsKind::Git);

    let identity = backend.identity(project).unwrap();
    assert_eq!(identity.vcs_kind, VcsKind::Git);
    assert!(identity.commit_id.is_some(), "Git should have commit_id");
    assert!(
        identity.change_id.is_none(),
        "Git should not have change_id"
    );
    assert!(identity.short_id.is_some(), "Git should have short_id");
    assert!(identity.op_id.is_none(), "Git should not have op_id");
    // ref_name should be the default branch
    assert!(identity.ref_name.is_some(), "Git should have ref_name");
}

// ── Session round-trip tests ────────────────────────────────────

#[test]
fn test_session_state_backward_compat_without_vcs_identity() {
    // Simulate a v1 session TOML (no vcs_identity field)
    let toml_str = r#"
meta_session_id = "01AAAAAAAAAAAAAAAAAAAAAAAAA"
project_path = "/tmp/test"
created_at = "2026-01-01T00:00:00Z"
last_accessed = "2026-01-01T00:00:00Z"
branch = "feat/test"
git_head_at_creation = "abc123def456"
change_id = "abc123def456"

[genealogy]
depth = 0

[context_status]
is_compacted = false

[task_context]
"#;

    let state: MetaSessionState = toml::from_str(toml_str).unwrap();

    // Legacy fields should be populated
    assert_eq!(state.branch.as_deref(), Some("feat/test"));
    assert_eq!(state.git_head_at_creation.as_deref(), Some("abc123def456"));
    assert_eq!(state.change_id.as_deref(), Some("abc123def456"));

    // vcs_identity should be None (not in TOML)
    assert!(state.vcs_identity.is_none());

    // identity_version should default to 1
    assert_eq!(state.identity_version, 1);

    // resolved_identity() should construct from legacy fields
    let resolved = state.resolved_identity();
    assert_eq!(resolved.vcs_kind, VcsKind::Git);
    assert_eq!(resolved.commit_id.as_deref(), Some("abc123def456"));
    assert_eq!(resolved.ref_name.as_deref(), Some("feat/test"));
    // change_id == git_head → not jj, so change_id should be None
    assert!(resolved.change_id.is_none());
}

#[test]
fn test_session_state_with_vcs_identity_roundtrip() {
    // Create a v2 session TOML with vcs_identity
    let toml_str = r#"
meta_session_id = "01BBBBBBBBBBBBBBBBBBBBBBBBB"
project_path = "/tmp/test"
created_at = "2026-01-01T00:00:00Z"
last_accessed = "2026-01-01T00:00:00Z"
branch = "feat/test"
git_head_at_creation = "abc123"
change_id = "abc123"
identity_version = 2

[vcs_identity]
vcs_kind = "git"
commit_id = "abc123"
short_id = "abc1"
ref_name = "feat/test"

[genealogy]
depth = 0

[context_status]
is_compacted = false

[task_context]
"#;

    let state: MetaSessionState = toml::from_str(&toml_str).unwrap();

    // v2 session should have vcs_identity
    assert!(state.vcs_identity.is_some());
    assert_eq!(state.identity_version, 2);

    // resolved_identity() should return the v2 identity
    let resolved = state.resolved_identity();
    assert_eq!(resolved.vcs_kind, VcsKind::Git);
    assert_eq!(resolved.commit_id.as_deref(), Some("abc123"));
    assert_eq!(resolved.ref_name.as_deref(), Some("feat/test"));
}

#[test]
fn test_resolved_identity_detects_jj_from_legacy_fields() {
    // Simulate a legacy session where change_id differs from git_head (jj session)
    let toml_str = r#"
meta_session_id = "01CCCCCCCCCCCCCCCCCCCCCCCCC"
project_path = "/tmp/test"
created_at = "2026-01-01T00:00:00Z"
last_accessed = "2026-01-01T00:00:00Z"
branch = "my-bookmark"
git_head_at_creation = "deadbeef123456"
change_id = "kxmlopqrstuvwxyz"

[genealogy]
depth = 0

[context_status]
is_compacted = false

[task_context]
"#;

    let state: MetaSessionState = toml::from_str(toml_str).unwrap();
    let resolved = state.resolved_identity();

    // Different change_id vs git_head → detected as jj
    assert_eq!(resolved.vcs_kind, VcsKind::Jj);
    assert_eq!(resolved.commit_id.as_deref(), Some("deadbeef123456"));
    assert_eq!(resolved.change_id.as_deref(), Some("kxmlopqrstuvwxyz"));
    assert_eq!(resolved.ref_name.as_deref(), Some("my-bookmark"));
}
