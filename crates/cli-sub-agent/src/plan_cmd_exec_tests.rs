//! Tests for plan_cmd_exec (bash/csa step execution, env sanitization).
//!
//! Split out of plan_cmd_exec.rs to stay under the monolith token budget.

use super::*;
use crate::test_env_lock::TEST_ENV_LOCK;

fn startup_env_with_pin(depth: u32) -> crate::startup_env::StartupSubtreeEnv {
    crate::startup_env::StartupSubtreeEnv::from_values(HashMap::from([
        (csa_core::env::CSA_DEPTH_ENV_KEY, depth.to_string()),
        (csa_core::env::CSA_MODEL_SPEC_ENV_KEY, PIN_SPEC.to_string()),
        (
            csa_core::env::CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY,
            "1".to_string(),
        ),
        (csa_core::env::CSA_NO_FAILOVER_ENV_KEY, "1".to_string()),
    ]))
}

#[test]
fn is_step_runtime_var_only_matches_step_output_and_session() {
    assert!(is_step_runtime_var("STEP_1_OUTPUT"));
    assert!(is_step_runtime_var("STEP_22_SESSION"));
    assert!(!is_step_runtime_var("STEP_OUTPUT"));
    assert!(!is_step_runtime_var("STEP_1_OUTPUT_JSON"));
    assert!(!is_step_runtime_var("STEP_A_OUTPUT"));
    assert!(!is_step_runtime_var("USER_LANGUAGE"));
}

#[test]
fn reduce_bash_env_for_spawn_drops_unreferenced_step_runtime_vars() {
    let env_vars = HashMap::from([
        ("STEP_1_OUTPUT".to_string(), "large".to_string()),
        ("STEP_2_SESSION".to_string(), "sid".to_string()),
        (
            "USER_LANGUAGE".to_string(),
            "Chinese (Simplified)".to_string(),
        ),
    ]);

    let reduced = reduce_bash_env_for_spawn("echo ok", &env_vars);
    assert!(!reduced.contains_key("STEP_1_OUTPUT"));
    assert!(!reduced.contains_key("STEP_2_SESSION"));
    assert_eq!(
        reduced.get("USER_LANGUAGE").map(String::as_str),
        Some("Chinese (Simplified)")
    );
}

#[test]
fn reduce_bash_env_for_spawn_keeps_referenced_step_runtime_vars() {
    let env_vars = HashMap::from([
        ("STEP_1_OUTPUT".to_string(), "payload".to_string()),
        ("STEP_2_SESSION".to_string(), "sid".to_string()),
        ("SCOPE".to_string(), "demo".to_string()),
    ]);

    let script = "printf '%s' \"${STEP_1_OUTPUT}\"; printenv STEP_2_SESSION >/dev/null";
    let reduced = reduce_bash_env_for_spawn(script, &env_vars);
    assert_eq!(
        reduced.get("STEP_1_OUTPUT").map(String::as_str),
        Some("payload")
    );
    assert_eq!(
        reduced.get("STEP_2_SESSION").map(String::as_str),
        Some("sid")
    );
    assert_eq!(reduced.get("SCOPE").map(String::as_str), Some("demo"));
}

#[test]
fn clean_step_output_extracts_codex_json_event_stream_text() {
    let output = [
            r#"{"type":"thread.started","thread_id":"thread_1"}"#,
            r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"- [ ] write test"}}"#,
            r#"{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"schema_version = 1"}}"#,
        ]
        .join("\n");

    assert_eq!(
        clean_step_output_for_env(&output, &ToolName::Codex, OutputFormat::Json),
        "- [ ] write test\nschema_version = 1"
    );
}

#[test]
fn clean_step_output_ignores_codex_tool_result_items() {
    let output = [
            r#"{"type":"thread.started","thread_id":"thread_1"}"#,
            r#"{"type":"item.completed","item":{"id":"item_1","type":"tool_result","text":"secret shell output"}}"#,
            r#"{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"agent summary"}}"#,
        ]
        .join("\n");

    assert_eq!(
        clean_step_output_for_env(&output, &ToolName::Codex, OutputFormat::Json),
        "agent summary"
    );
}

#[test]
fn clean_step_output_drops_codex_stream_without_agent_messages() {
    let output = [
            r#"{"type":"thread.started","thread_id":"thread_1"}"#,
            r#"{"type":"item.completed","item":{"id":"item_1","type":"tool_result","text":"secret shell output"}}"#,
        ]
        .join("\n");

    assert_eq!(
        clean_step_output_for_env(&output, &ToolName::Codex, OutputFormat::Json),
        ""
    );
}

#[test]
fn clean_step_output_falls_back_for_codex_json_without_text() {
    let output = "not json\n{\"type\":\"thread.started\"}";

    assert_eq!(
        clean_step_output_for_env(output, &ToolName::Codex, OutputFormat::Json),
        output
    );
}

#[test]
fn clean_step_output_leaves_clean_prose_for_non_codex_tools() {
    let output = "plain summary\n- [ ] already clean";

    assert_eq!(
        clean_step_output_for_env(output, &ToolName::GeminiCli, OutputFormat::Json),
        output
    );
    assert_eq!(
        clean_step_output_for_env(output, &ToolName::ClaudeCode, OutputFormat::Json),
        output
    );
}

#[test]
fn clean_step_output_extracts_mixed_json_stream_and_ignores_trailing_prose() {
    let output = [
            r#"{"type":"thread.started","thread_id":"thread_1"}"#,
            r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"natural text"}}"#,
            "trailing progress note",
        ]
        .join("\n");

    assert_eq!(
        clean_step_output_for_env(&output, &ToolName::GeminiCli, OutputFormat::Text),
        "natural text"
    );
}

#[test]
fn next_csa_depth_increments_or_defaults() {
    assert_eq!(
        crate::startup_env::StartupSubtreeEnv::default().next_depth_string(),
        "1"
    );
    assert_eq!(startup_env_with_pin(2).next_depth_string(), "3");
}

const PIN_SPEC: &str = "codex/openai/gpt-5.5/xhigh";

/// Inspect the explicit env overrides recorded on a `tokio::process::Command`.
/// `env_remove(k)` is recorded as `(k, None)`; `env(k, v)` as `(k, Some(v))`.
fn recorded_env(
    cmd: &tokio::process::Command,
) -> std::collections::HashMap<String, Option<String>> {
    cmd.as_std()
        .get_envs()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.map(|v| v.to_string_lossy().into_owned()),
            )
        })
        .collect()
}

/// #1741 round-6: a bash step is marked nested (CSA_DEPTH set) and inherits
/// the parent env. When the parent is ROOT (depth 0) but ambient
/// SUBTREE_PIN_ENV_KEYS are present (a user-controlled spoof attempt), the
/// spawned child env MUST NOT carry the pin keys — they are env_removed
/// (reserved) and NOT re-applied, because the root process has no legitimate
/// inherited pin.
#[test]
fn spawn_bash_env_strips_ambient_subtree_pin_when_not_legitimately_inherited() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let original: Vec<(&str, Option<String>)> = [
        "CSA_DEPTH",
        csa_core::env::CSA_MODEL_SPEC_ENV_KEY,
        csa_core::env::CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY,
        csa_core::env::CSA_NO_FAILOVER_ENV_KEY,
    ]
    .iter()
    .map(|k| (*k, std::env::var(k).ok()))
    .collect();

    // SAFETY: test-scoped env mutation, serialized by TEST_ENV_LOCK.
    unsafe {
        // Root depth: any ambient pin is NOT a CSA-injected inherited pin.
        std::env::set_var("CSA_DEPTH", "0");
        std::env::set_var(csa_core::env::CSA_MODEL_SPEC_ENV_KEY, PIN_SPEC);
        std::env::set_var(csa_core::env::CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY, "1");
        std::env::set_var(csa_core::env::CSA_NO_FAILOVER_ENV_KEY, "1");
    }

    let startup_env = startup_env_with_pin(0);
    let mut cmd = tokio::process::Command::new("bash");
    apply_sanitized_subtree_pin(&mut cmd, &startup_env);
    let env = recorded_env(&cmd);

    for key in csa_core::env::SUBTREE_PIN_ENV_KEYS {
        assert_eq!(
            env.get(*key),
            Some(&None),
            "ambient subtree-pin key {key} must be env_removed (reserved), \
                 never propagated to the nested bash step at root depth"
        );
    }

    // SAFETY: restore original env values.
    unsafe {
        for (key, value) in original {
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }
}

/// #1741 round-6: a legitimately-propagated subtree pin (this process is a
/// genuine pinned child: CSA_DEPTH > 0 + well-formed pin in env, as written
/// by the parent's trusted typed channel) MUST still cascade to the nested
/// bash step. The strip-then-reapply path re-writes the pin keys from the
/// typed channel, so legitimate propagation is preserved.
#[test]
fn spawn_bash_env_reapplies_legitimately_inherited_subtree_pin() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let original: Vec<(&str, Option<String>)> = [
        "CSA_DEPTH",
        csa_core::env::CSA_MODEL_SPEC_ENV_KEY,
        csa_core::env::CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY,
        csa_core::env::CSA_NO_FAILOVER_ENV_KEY,
    ]
    .iter()
    .map(|k| (*k, std::env::var(k).ok()))
    .collect();

    // SAFETY: test-scoped env mutation, serialized by TEST_ENV_LOCK.
    unsafe {
        // Child depth + well-formed pin + paired force-ignore marker =
        // a genuine CSA-injected inherited pin.
        std::env::set_var("CSA_DEPTH", "2");
        std::env::set_var(csa_core::env::CSA_MODEL_SPEC_ENV_KEY, PIN_SPEC);
        std::env::set_var(csa_core::env::CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY, "1");
        std::env::set_var(csa_core::env::CSA_NO_FAILOVER_ENV_KEY, "1");
    }

    let startup_env = startup_env_with_pin(2);
    let mut cmd = tokio::process::Command::new("bash");
    apply_sanitized_subtree_pin(&mut cmd, &startup_env);
    let env = recorded_env(&cmd);

    assert_eq!(
        env.get(csa_core::env::CSA_MODEL_SPEC_ENV_KEY),
        Some(&Some(PIN_SPEC.to_string())),
        "legitimately-inherited pin spec must cascade to the nested bash step"
    );
    assert_eq!(
        env.get(csa_core::env::CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY),
        Some(&Some("1".to_string())),
        "the paired force-ignore marker must cascade"
    );
    assert_eq!(
        env.get(csa_core::env::CSA_NO_FAILOVER_ENV_KEY),
        Some(&Some("1".to_string())),
        "no-failover must cascade when the inherited pin carries it"
    );

    // SAFETY: restore original env values.
    unsafe {
        for (key, value) in original {
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }
}
