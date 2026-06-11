use crate::test_env_lock::{ScopedEnvVarRestore, ScopedTestEnvVar};
use csa_core::types::ToolName;

fn assume_tier_tools_available() -> ScopedTestEnvVar {
    ScopedTestEnvVar::set(super::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1")
}

#[test]
fn resolve_tool_and_model_default_tier_skips_unconfigured_openai_compat_fallback() {
    let _tool_availability = assume_tier_tools_available();
    let _base = ScopedEnvVarRestore::unset("OPENAI_COMPAT_BASE_URL");
    let _key = ScopedEnvVarRestore::unset("OPENAI_COMPAT_API_KEY");
    let _model = ScopedEnvVarRestore::unset("OPENAI_COMPAT_MODEL");
    let mut config = super::tier_tests::config_with_tier(
        "tier-3-complex",
        vec![
            "openai-compat/openai/gpt-5/high",
            "codex/openai/gpt-5.4/xhigh",
        ],
        &["openai-compat", "codex"],
    );
    config
        .tier_mapping
        .insert("default".to_string(), "tier-3-complex".to_string());
    let project_root = tempfile::tempdir().expect("temp project root");

    let result = super::resolve_tool_and_model(super::RoutingRequest {
        config: Some(&config),
        ..super::RoutingRequest::new(project_root.path())
    });

    match result {
        Ok((tool, model_spec, _)) => {
            assert_eq!(tool, ToolName::Codex);
            assert_eq!(model_spec.as_deref(), Some("codex/openai/gpt-5.4/xhigh"));
        }
        Err(err) => {
            panic!("default tier should fall through to runtime-available codex: {err}");
        }
    }
}
