//! Tests for plan_cmd_exec (bash/csa step execution, env sanitization).
//!
//! Split out of plan_cmd_exec.rs to stay under the monolith token budget.

use super::*;
use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};

fn startup_env_with_pin(depth: u32) -> crate::startup_env::StartupSubtreeEnv {
    crate::startup_env::StartupSubtreeEnv::from_values(HashMap::from([
        (csa_core::env::CSA_DEPTH_ENV_KEY, depth.to_string()),
        (
            csa_core::env::CSA_INTERNAL_INVOCATION_ENV_KEY,
            "1".to_string(),
        ),
        (
            csa_core::env::CSA_SESSION_ID_ENV_KEY,
            "01KPINNEDSESSION0000000000".to_string(),
        ),
        (
            csa_core::env::CSA_SESSION_DIR_ENV_KEY,
            "/repo/.csa/sessions/01KPINNEDSESSION0000000000".to_string(),
        ),
        (csa_core::env::CSA_PROJECT_ROOT_ENV_KEY, "/repo".to_string()),
        (csa_core::env::CSA_MODEL_SPEC_ENV_KEY, PIN_SPEC.to_string()),
        (
            csa_core::env::CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY,
            "1".to_string(),
        ),
        (csa_core::env::CSA_NO_FAILOVER_ENV_KEY, "1".to_string()),
    ]))
}

fn trusted_startup_env_for_pinned_plan_session(
    project_root: &std::path::Path,
    model_spec: &str,
    no_failover: bool,
) -> crate::startup_env::StartupSubtreeEnv {
    let session = csa_session::create_session(
        project_root,
        Some("plan pinned startup"),
        None,
        Some("codex"),
    )
    .expect("create pinned plan session");
    let session_dir =
        csa_session::get_session_dir(project_root, &session.meta_session_id).expect("session dir");
    let pin =
        crate::run_cmd_model_pin::resolve_subtree_model_pin(Some(model_spec), true, no_failover)
            .expect("typed pin");
    crate::run_cmd_model_pin::sync_subtree_model_pin_sidecar(
        project_root,
        &session.meta_session_id,
        &session_dir,
        Some(&pin),
    )
    .expect("write trusted pin sidecar");

    crate::startup_env::StartupSubtreeEnv::from_values(HashMap::from([
        (
            csa_core::env::CSA_DEPTH_ENV_KEY,
            session.genealogy.depth.saturating_add(1).to_string(),
        ),
        (
            csa_core::env::CSA_INTERNAL_INVOCATION_ENV_KEY,
            "1".to_string(),
        ),
        (
            csa_core::env::CSA_SESSION_ID_ENV_KEY,
            session.meta_session_id,
        ),
        (
            csa_core::env::CSA_SESSION_DIR_ENV_KEY,
            session_dir.display().to_string(),
        ),
        (
            csa_core::env::CSA_PROJECT_ROOT_ENV_KEY,
            project_root.display().to_string(),
        ),
        (
            csa_core::env::CSA_MODEL_SPEC_ENV_KEY,
            model_spec.to_string(),
        ),
        (
            csa_core::env::CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY,
            "1".to_string(),
        ),
        (
            csa_core::env::CSA_NO_FAILOVER_ENV_KEY,
            if no_failover { "1" } else { "0" }.to_string(),
        ),
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
fn current_exe_dir_for_path_prepend_ignores_relative_fallback_without_parent() {
    assert!(current_exe_dir_for_path_prepend(std::path::Path::new("csa")).is_none());
    assert_eq!(
        current_exe_dir_for_path_prepend(std::path::Path::new("/tmp/csa")),
        Some(std::path::Path::new("/tmp"))
    );
}

#[test]
fn extract_bash_code_block_ignores_markdown_fence_literals_inside_script() {
    let prompt = r#"Run this:
```bash
set -euo pipefail
EPIC_PLAN=$(printf '%s\n' "${FINAL_TODO}" | sed -n '/^```epic-plan.toml$/,/^```$/p' | sed '1d;$d')
printf '%s\n' "${EPIC_PLAN}"
```
Afterward text.
"#;

    let script = extract_bash_code_block(prompt).expect("bash block should parse");

    assert!(
        script.contains("epic-plan.toml"),
        "script should preserve the sed expression containing markdown fences"
    );
    assert!(
        script.contains(r#"printf '%s\n' "${EPIC_PLAN}""#),
        "script must not be truncated at the fence literal"
    );
}

#[test]
fn append_bash_child_diagnostics_adds_child_status_to_stderr() {
    let td = tempfile::tempdir().expect("tempdir");
    let _sandbox = crate::test_session_sandbox::ScopedSessionSandbox::new_blocking(&td);
    let project = td.path();
    let session = csa_session::create_session(project, Some("child"), None, Some("codex"))
        .expect("create child session");
    let mut stderr = format!(
        "Session {} has no live daemon process and no terminal result packet.",
        session.meta_session_id
    );

    append_bash_child_diagnostics(&mut stderr, project, "");

    assert!(stderr.contains("plan_child_died"));
    assert!(stderr.contains("status=NoLivePID"));
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

fn startup_env_from_recorded_env(
    env: &std::collections::HashMap<String, Option<String>>,
) -> crate::startup_env::StartupSubtreeEnv {
    let mut values = HashMap::new();
    for key in csa_core::env::STARTUP_SUBTREE_ENV_KEYS {
        if let Some(Some(value)) = env.get(*key) {
            values.insert(*key, value.clone());
        }
    }
    if let Some(Some(value)) = env.get(csa_core::env::CSA_PATTERN_INTERNAL_ENV_KEY) {
        values.insert(csa_core::env::CSA_PATTERN_INTERNAL_ENV_KEY, value.clone());
    }
    crate::startup_env::StartupSubtreeEnv::from_values(values)
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
        csa_core::env::CSA_INTERNAL_INVOCATION_ENV_KEY,
        csa_core::env::CSA_SESSION_ID_ENV_KEY,
        csa_core::env::CSA_SESSION_DIR_ENV_KEY,
        csa_core::env::CSA_PROJECT_ROOT_ENV_KEY,
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
        std::env::set_var(csa_core::env::CSA_INTERNAL_INVOCATION_ENV_KEY, "1");
        std::env::set_var(
            csa_core::env::CSA_SESSION_ID_ENV_KEY,
            "01KPINNEDSESSION0000000000",
        );
        std::env::set_var(
            csa_core::env::CSA_SESSION_DIR_ENV_KEY,
            "/repo/.csa/sessions/01KPINNEDSESSION0000000000",
        );
        std::env::set_var(csa_core::env::CSA_PROJECT_ROOT_ENV_KEY, "/repo");
        std::env::set_var(csa_core::env::CSA_MODEL_SPEC_ENV_KEY, PIN_SPEC);
        std::env::set_var(csa_core::env::CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY, "1");
        std::env::set_var(csa_core::env::CSA_NO_FAILOVER_ENV_KEY, "1");
    }

    let startup_env = startup_env_with_pin(0);
    let mut cmd = tokio::process::Command::new("bash");
    for key in csa_core::env::SUBTREE_PIN_ENV_KEYS {
        cmd.env(key, "spoofed");
    }
    apply_startup_child_contract_env(&mut cmd, &startup_env);
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

/// #1741 round-6/#2097: a legitimately-propagated subtree pin (this process is
/// a genuine pinned child: CSA_DEPTH > 0 + child contract backed by persisted
/// session state + matching subtree-model-pin.toml sidecar) MUST still cascade
/// to the nested bash step. The strip-then-reapply path re-writes the pin keys
/// from the typed channel, so legitimate propagation is preserved.
#[test]
fn spawn_bash_env_reapplies_legitimately_inherited_subtree_pin() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let xdg = tempfile::tempdir().expect("xdg tempdir");
    let _xdg_guard = ScopedEnvVarRestore::set("XDG_STATE_HOME", xdg.path());
    let project = tempfile::tempdir().expect("project tempdir");
    let startup_env = trusted_startup_env_for_pinned_plan_session(project.path(), PIN_SPEC, true);
    let mut cmd = tokio::process::Command::new("bash");
    cmd.env(csa_core::env::CSA_PROJECT_ROOT_ENV_KEY, project.path())
        .env(
            csa_core::env::CSA_DEPTH_ENV_KEY,
            bash_step_depth_string(&startup_env),
        )
        .env(csa_core::env::CSA_INTERNAL_INVOCATION_ENV_KEY, "1")
        .env(csa_core::env::CSA_PATTERN_INTERNAL_ENV_KEY, "1");
    apply_startup_child_contract_env(&mut cmd, &startup_env);
    let env = recorded_env(&cmd);

    assert_eq!(
        env.get(csa_core::env::CSA_DEPTH_ENV_KEY),
        Some(&Some(startup_env.current_depth().to_string())),
        "plan bash reuses the same trusted session sidecar/state contract, so \
         it must not advance CSA_DEPTH beyond that session's startup depth"
    );
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

    let nested_startup_env = startup_env_from_recorded_env(&env);
    let inherited = crate::run_cmd_model_pin::inherited_model_pin_from_startup(&nested_startup_env)
        .expect("plan bash env must still validate against the trusted sidecar");
    assert_eq!(inherited.model_spec, PIN_SPEC);
    assert!(inherited.force_ignore_tier_setting);
    assert!(inherited.no_failover);
}

/// #1750 round-4: foreground nested plan bash steps are CSA-child boundaries.
/// They must carry the startup-captured session identity and parent identity
/// into nested `csa run` / `csa review` / `csa plan run` invocations.
#[test]
fn spawn_bash_env_reapplies_session_identity_and_parent_contract() {
    let startup_env = crate::startup_env::StartupSubtreeEnv::from_values(HashMap::from([
        (
            csa_core::env::CSA_SESSION_ID_ENV_KEY,
            "01KSESSION".to_string(),
        ),
        (
            csa_core::env::CSA_SESSION_DIR_ENV_KEY,
            "/repo/session".to_string(),
        ),
        (
            csa_core::env::CSA_PARENT_SESSION_ENV_KEY,
            "01KPARENT".to_string(),
        ),
        (
            csa_core::env::CSA_PARENT_SESSION_DIR_ENV_KEY,
            "/repo/parent".to_string(),
        ),
    ]));
    let mut cmd = tokio::process::Command::new("bash");
    cmd.env(csa_core::env::CSA_SESSION_ID_ENV_KEY, "spoofed-session");
    cmd.env(csa_core::env::CSA_PARENT_SESSION_ENV_KEY, "spoofed-parent");
    cmd.env(
        csa_core::env::CSA_PARENT_SESSION_DIR_ENV_KEY,
        "spoofed-parent-dir",
    );

    apply_startup_child_contract_env(&mut cmd, &startup_env);

    let env = recorded_env(&cmd);
    assert_eq!(
        env.get(csa_core::env::CSA_SESSION_ID_ENV_KEY),
        Some(&Some("01KSESSION".to_string()))
    );
    assert_eq!(
        env.get(csa_core::env::CSA_SESSION_DIR_ENV_KEY),
        Some(&Some("/repo/session".to_string()))
    );
    assert_eq!(
        env.get(csa_core::env::CSA_PARENT_SESSION_ENV_KEY),
        Some(&Some("01KPARENT".to_string()))
    );
    assert_eq!(
        env.get(csa_core::env::CSA_PARENT_SESSION_DIR_ENV_KEY),
        Some(&Some("/repo/parent".to_string()))
    );
}

#[test]
fn spawn_bash_env_removes_absent_session_identity_contract_keys() {
    let startup_env = crate::startup_env::StartupSubtreeEnv::default();
    let mut cmd = tokio::process::Command::new("bash");
    cmd.env(csa_core::env::CSA_SESSION_ID_ENV_KEY, "spoofed-session");
    cmd.env(csa_core::env::CSA_SESSION_DIR_ENV_KEY, "spoofed-dir");
    cmd.env(csa_core::env::CSA_PARENT_SESSION_ENV_KEY, "spoofed-parent");
    cmd.env(
        csa_core::env::CSA_PARENT_SESSION_DIR_ENV_KEY,
        "spoofed-parent-dir",
    );

    apply_startup_child_contract_env(&mut cmd, &startup_env);

    let env = recorded_env(&cmd);
    for key in [
        csa_core::env::CSA_SESSION_ID_ENV_KEY,
        csa_core::env::CSA_SESSION_DIR_ENV_KEY,
        csa_core::env::CSA_PARENT_SESSION_ENV_KEY,
        csa_core::env::CSA_PARENT_SESSION_DIR_ENV_KEY,
    ] {
        assert_eq!(
            env.get(key),
            Some(&None),
            "absent startup contract key {key} must not be fabricated"
        );
    }
}

/// #1847 case (i): every weave `tool = "bash"` step spawned by `csa plan run`
/// must expose `CSA_PATTERN_INTERNAL=1` to its shell, so any nested `csa`
/// run/review/debate it invokes defaults the fatal-error-marker scan OFF and
/// cannot self-kill the pipeline on codex-fallback provider-error text. The
/// marker is set unconditionally by `spawn_bash`, independent of ambient env.
#[tokio::test]
async fn spawn_bash_step_exposes_pattern_internal_marker() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let project_root = temp.path();
    let workflow_path = project_root.join("workflow.toml");
    let env_vars: HashMap<String, String> = HashMap::new();
    let startup_env = crate::startup_env::StartupSubtreeEnv::default();

    let output = spawn_bash(
        "printf '%s' \"${CSA_PATTERN_INTERNAL:-MISSING}\"",
        &env_vars,
        project_root,
        &workflow_path,
        &startup_env,
        RunResourceOverrides::default(),
    )
    .await
    .expect("bash step should spawn");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout, "1",
        "weave bash step must see CSA_PATTERN_INTERNAL=1 (#1847)"
    );
}

/// #2375: workflow bash steps that recursively call `csa` need a stable path
/// to the exact binary that launched the current plan run. Otherwise a parent
/// using `target/release/csa` can spawn nested work through a stale installed
/// `csa` found on PATH.
#[tokio::test]
async fn spawn_bash_step_exposes_current_executable_as_csa_bin() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let project_root = temp.path();
    let workflow_path = project_root.join("workflow.toml");
    let env_vars: HashMap<String, String> = HashMap::new();
    let startup_env = crate::startup_env::StartupSubtreeEnv::default();

    let output = spawn_bash(
        "test -n \"${CSA_BIN:-}\" && test -x \"${CSA_BIN}\" && printf '%s' \"${CSA_BIN}\"",
        &env_vars,
        project_root,
        &workflow_path,
        &startup_env,
        RunResourceOverrides::default(),
    )
    .await
    .expect("bash step should spawn");

    assert!(
        output.status.success(),
        "CSA_BIN should point at an executable path: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        std::env::current_exe()
            .expect("current executable path")
            .to_string_lossy()
    );
}
