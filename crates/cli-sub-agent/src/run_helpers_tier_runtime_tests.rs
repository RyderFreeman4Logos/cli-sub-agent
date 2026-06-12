use crate::test_env_lock::{ScopedEnvVarRestore, ScopedTestEnvVar};
use csa_config::GlobalConfig;
use csa_config::global::GlobalToolConfig;
use csa_core::types::ToolName;
use std::collections::HashMap;

fn assume_tier_tools_available() -> ScopedTestEnvVar {
    ScopedTestEnvVar::set(super::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1")
}

fn global_openai_compat_env_config() -> GlobalConfig {
    let mut global_config = GlobalConfig::default();
    global_config.tools.insert(
        "openai-compat".to_string(),
        GlobalToolConfig {
            env: HashMap::from([
                (
                    "OPENAI_COMPAT_BASE_URL".to_string(),
                    "http://localhost:8317".to_string(),
                ),
                ("OPENAI_COMPAT_API_KEY".to_string(), "test-key".to_string()),
                ("OPENAI_COMPAT_MODEL".to_string(), "local-model".to_string()),
            ]),
            ..Default::default()
        },
    );
    global_config
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

#[test]
fn resolve_tool_from_tier_keeps_globally_configured_openai_compat() {
    let _tool_availability = assume_tier_tools_available();
    let _base = ScopedEnvVarRestore::unset("OPENAI_COMPAT_BASE_URL");
    let _key = ScopedEnvVarRestore::unset("OPENAI_COMPAT_API_KEY");
    let _model = ScopedEnvVarRestore::unset("OPENAI_COMPAT_MODEL");
    let global_config = global_openai_compat_env_config();
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

    let result = super::resolve_tool_from_tier_with_global_config(
        "tier-3-complex",
        &config,
        Some(&global_config),
        None,
        &[],
        &[],
    );

    match result {
        Some(resolution) => {
            assert_eq!(resolution.tool, ToolName::OpenaiCompat);
            assert_eq!(
                resolution.model_spec.as_str(),
                "openai-compat/openai/gpt-5/high"
            );
        }
        None => {
            panic!("global openai-compat env should make the tier candidate available");
        }
    }
}
