pub(in crate::review_cmd::tests) use super::*;
pub(crate) use review_core::{
    ScopedEnvVarRestore, project_config_with_enabled_tools, setup_git_repo,
};

#[path = "review_cmd_tests.rs"]
mod review_core;
#[path = "review_cmd_tests_safety_preamble.rs"]
mod safety_preamble_tests;
#[path = "review_cmd_tests_full_consistency.rs"]
mod tests_full_consistency;
#[path = "review_cmd_timeout_tests.rs"]
mod timeout_tests;

#[test]
fn review_readonly_project_root_defaults_to_readonly_without_fix() {
    assert!(resolve_review_readonly_project_root(false, None));
    assert!(resolve_review_readonly_project_root(false, Some(true)));
    assert!(!resolve_review_readonly_project_root(false, Some(false)));
}

#[test]
fn review_fix_forces_writable_project_root() {
    assert!(!resolve_review_readonly_project_root(true, None));
    assert!(!resolve_review_readonly_project_root(true, Some(true)));
    assert!(!resolve_review_readonly_project_root(true, Some(false)));
}
