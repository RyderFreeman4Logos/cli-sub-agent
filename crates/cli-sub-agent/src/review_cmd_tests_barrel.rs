pub(in crate::review_cmd::tests) use super::*;
use csa_core::types::ToolName;
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

#[test]
fn explicit_tool_failover_context_requires_explicit_tool_active_tier_and_failover() {
    let context = |direct_tool_requested: bool, tier_active: bool, no_failover: bool| {
        (direct_tool_requested && tier_active && !no_failover).then_some(ToolName::Codex)
    };
    assert_eq!(context(true, true, false), Some(ToolName::Codex));
    assert_eq!(context(false, true, false), None);
    assert_eq!(context(true, false, false), None);
    assert_eq!(context(true, true, true), None);
}

fn project_config_with_review_readonly(readonly_sandbox: Option<bool>) -> ProjectConfig {
    let mut config = project_config_with_enabled_tools(&["codex"]);
    config.review = Some(csa_config::global::ReviewConfig {
        readonly_sandbox,
        ..Default::default()
    });
    config
}

#[test]
fn review_readonly_configured_prefers_project_over_global() {
    let mut global = GlobalConfig::default();
    global.review.readonly_sandbox = Some(false);
    let project = project_config_with_review_readonly(Some(true));
    assert_eq!(
        resolve_review_readonly_configured(Some(&project), &global),
        Some(true)
    );
    assert!(resolve_review_readonly_project_root(
        false,
        resolve_review_readonly_configured(Some(&project), &global)
    ));

    let mut global = GlobalConfig::default();
    global.review.readonly_sandbox = Some(true);
    let project = project_config_with_review_readonly(Some(false));
    assert_eq!(
        resolve_review_readonly_configured(Some(&project), &global),
        Some(false)
    );
    assert!(!resolve_review_readonly_project_root(
        false,
        resolve_review_readonly_configured(Some(&project), &global)
    ));

    let global = GlobalConfig::default();
    let project = project_config_with_review_readonly(Some(false));
    assert_eq!(
        resolve_review_readonly_configured(Some(&project), &global),
        Some(false)
    );
    assert!(!resolve_review_readonly_project_root(
        false,
        resolve_review_readonly_configured(Some(&project), &global)
    ));
}

#[test]
fn review_readonly_configured_falls_back_to_global_when_project_unset() {
    let project = project_config_with_review_readonly(None);

    let mut global = GlobalConfig::default();
    global.review.readonly_sandbox = Some(true);
    assert_eq!(
        resolve_review_readonly_configured(Some(&project), &global),
        Some(true)
    );
    assert!(resolve_review_readonly_project_root(
        false,
        resolve_review_readonly_configured(Some(&project), &global)
    ));

    global.review.readonly_sandbox = Some(false);
    assert_eq!(
        resolve_review_readonly_configured(Some(&project), &global),
        Some(false)
    );
    assert!(!resolve_review_readonly_project_root(
        false,
        resolve_review_readonly_configured(Some(&project), &global)
    ));
}
