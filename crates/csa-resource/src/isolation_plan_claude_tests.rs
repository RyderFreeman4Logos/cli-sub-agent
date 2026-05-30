//! Integration tests for the claude-home writable-mount wiring
//! (`isolation_plan_claude.rs`).
//!
//! Mirrors the codex-home tests in `isolation_plan_tests.rs`:
//! - the owning `claude-code` tool gets the claude home writable AND a fail-fast
//!   write probe (`RequiredWritableDir`);
//! - a peer (non claude-code) tool gets the claude home writable WITHOUT a probe,
//!   gated on claude being installed — the symmetric widening that fixes the
//!   nested-EROFS failure in #1683 / #1661 / #161.
//!
//! HOME is always redirected to a temp dir so assertions never depend on the
//! host's real `~/.claude` (see the env-dependent-test lint and the #642 lesson).
use super::*;
use std::ffi::OsString;

struct ScopedEnvVar {
    key: &'static str,
    previous: Option<OsString>,
}

impl ScopedEnvVar {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: tests that mutate process environment hold ENV_LOCK.
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }

    fn unset(key: &'static str) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: tests that mutate process environment hold ENV_LOCK.
        unsafe { std::env::remove_var(key) };
        Self { key, previous }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        // SAFETY: tests that mutate process environment hold ENV_LOCK.
        unsafe {
            if let Some(value) = &self.previous {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }
}

/// (a) The owning `claude-code` tool registers a fail-fast write probe for the
/// claude home: an unwritable `CLAUDE_CONFIG_DIR` must abort the build, exactly
/// like the codex owner probe in
/// `test_tool_defaults_codex_rejects_unwritable_codex_home`.  This proves the
/// probe is registered for the owner (the writable-mount half is covered by
/// `test_claude_tool_defaults_precreate_claude_dir_for_session_env` and
/// `test_tool_defaults_claude_code`).
#[test]
fn test_tool_defaults_claude_code_rejects_unwritable_claude_home() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let _home_env = ScopedEnvVar::set("HOME", &home);

    let claude_home = temp.path().join("readonly-claude-home");
    std::fs::create_dir(&claude_home).unwrap();
    #[cfg(unix)]
    let original_mode = {
        use std::os::unix::fs::PermissionsExt;

        let metadata = std::fs::metadata(&claude_home).unwrap();
        let original_mode = metadata.permissions().mode();
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o500);
        std::fs::set_permissions(&claude_home, permissions).unwrap();
        original_mode
    };
    #[cfg(not(unix))]
    {
        let mut permissions = std::fs::metadata(&claude_home).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&claude_home, permissions).unwrap();
    }
    let _claude_home_env = ScopedEnvVar::set("CLAUDE_CONFIG_DIR", &claude_home);

    let project = PathBuf::from("/tmp/project");
    let session = PathBuf::from("/tmp/session");

    let error = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_tool_defaults("claude-code", &project, &session)
        .build()
        .expect_err("unwritable CLAUDE_CONFIG_DIR should fail preflight");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = std::fs::metadata(&claude_home).unwrap().permissions();
        permissions.set_mode(original_mode);
        std::fs::set_permissions(&claude_home, permissions).unwrap();
    }
    #[cfg(not(unix))]
    {
        let mut permissions = std::fs::metadata(&claude_home).unwrap().permissions();
        permissions.set_readonly(false);
        std::fs::set_permissions(&claude_home, permissions).unwrap();
    }

    let message = format!("{error:#}");
    assert!(
        message.contains("claude sandbox preflight failed"),
        "unexpected error: {message}"
    );
    assert!(
        message.contains("CLAUDE_CONFIG_DIR"),
        "error should name the canonical claude home source: {message}"
    );
    assert!(
        message.contains("[tools.claude-code].filesystem_sandbox.writable_paths"),
        "error should surface the claude sandbox config hint: {message}"
    );
}

/// (b) A peer (non claude-code) tool exposes the claude home WITHOUT a probe,
/// gated on claude being on PATH.  Because peers register no probe, an
/// unwritable claude home must NOT abort the build — the asymmetry mirrored from
/// `isolation_plan_codex.rs` (peer `~/.codex` widening without a probe).
#[test]
fn test_peer_tool_exposes_claude_home_without_probe() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let _home_env = ScopedEnvVar::set("HOME", &home);
    // Use the default ~/.claude layout (no override) for the peer path.
    let _claude_home_env = ScopedEnvVar::unset("CLAUDE_CONFIG_DIR");

    // Pre-create the default claude home and make it READ-ONLY.  A peer tool
    // adds it to writable_paths (so a nested claude-code child gets a writable
    // bind) but registers NO write probe, so the read-only mode must not abort.
    let claude_home = home.join(".claude");
    std::fs::create_dir(&claude_home).unwrap();
    #[cfg(unix)]
    let original_mode = {
        use std::os::unix::fs::PermissionsExt;

        let metadata = std::fs::metadata(&claude_home).unwrap();
        let original_mode = metadata.permissions().mode();
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o500);
        std::fs::set_permissions(&claude_home, permissions).unwrap();
        original_mode
    };

    let project = PathBuf::from("/tmp/project");
    let session = PathBuf::from("/tmp/session");

    // gemini-cli is a peer to BOTH codex and claude, so neither owner probe is
    // registered — this isolates the claude peer (no-probe) behavior.
    let result = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_tool_defaults("gemini-cli", &project, &session)
        .build();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = std::fs::metadata(&claude_home).unwrap().permissions();
        permissions.set_mode(original_mode);
        std::fs::set_permissions(&claude_home, permissions).unwrap();
    }

    let plan = result.expect("a peer tool must NOT probe an unwritable claude home");

    if claude_paths::has_claude_on_path() {
        assert!(
            plan.writable_paths.contains(&claude_home),
            "a peer sandbox should expose the claude home when claude is installed"
        );
    } else {
        assert!(
            !plan.writable_paths.contains(&claude_home),
            "without claude on PATH, a peer sandbox must NOT expose the claude home"
        );
    }
}

/// (c) Regression for the nested-EROFS bug (#1683 / #1661): a claude-code child
/// spawned NESTED inside a codex parent bwrap used to inherit a read-only
/// `~/.claude` (the depth-0 claude arm only ran for the claude-code tool), so
/// the child's SessionStart hook `mkdir ~/.claude/session-env/<id>` failed with
/// EROFS.  A codex parent must now expose the claude home writable.
#[test]
fn test_codex_parent_exposes_writable_claude_home_nested_erofs_regression() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let _home_env = ScopedEnvVar::set("HOME", &home);
    let _claude_home_env = ScopedEnvVar::unset("CLAUDE_CONFIG_DIR");
    // Keep the codex OWNER probe hermetic (writable temp); it is not under test.
    let codex_home = temp.path().join("codex-home");
    std::fs::create_dir(&codex_home).unwrap();
    let _codex_home_env = ScopedEnvVar::set("CODEX_HOME", &codex_home);

    let project = PathBuf::from("/tmp/project");
    let session = PathBuf::from("/tmp/session");

    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_tool_defaults("codex", &project, &session)
        .build()
        .expect("codex parent defaults should build");

    let claude_home = home.join(".claude");
    if claude_paths::has_claude_on_path() {
        assert!(
            plan.writable_paths.contains(&claude_home),
            "a codex parent must expose ~/.claude writable so a nested claude-code child avoids EROFS"
        );
        assert!(
            claude_home.is_dir(),
            "the claude home should be pre-created as a bwrap bind source"
        );
    } else {
        assert!(
            !plan.writable_paths.contains(&claude_home),
            "without claude on PATH, a codex parent need not expose ~/.claude"
        );
    }
}
