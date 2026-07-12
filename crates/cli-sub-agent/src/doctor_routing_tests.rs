use super::*;
use crate::test_env_lock::TEST_ENV_LOCK;
use std::{ffi::OsString, path::Path};

fn shipped_catalog() -> EffectiveModelCatalog {
    EffectiveModelCatalog::shipped().expect("shipped catalog")
}

fn write_project_config(project_root: &Path, contents: &str) {
    let config_dir = project_root.join(".csa");
    std::fs::create_dir_all(&config_dir).expect("create config dir");
    std::fs::write(config_dir.join("config.toml"), contents).expect("write config");
}

struct EnvVarGuard {
    key: &'static str,
    original: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let original = std::env::var_os(key);
        // SAFETY: test-scoped env mutation guarded by a process-wide mutex.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: restoration of test-scoped env mutation guarded by a process-wide mutex.
        unsafe {
            match self.original.take() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

#[test]
fn test_parse_model_spec_full() {
    let (tool, provider, model, thinking) =
        parse_model_spec("gemini-cli/google/gemini-2.5-pro/xhigh");
    assert_eq!(tool, "gemini-cli");
    assert_eq!(provider, "google");
    assert_eq!(model, "gemini-2.5-pro");
    assert_eq!(thinking, "xhigh");
}

#[test]
fn test_parse_model_spec_partial() {
    let (tool, provider, model, thinking) = parse_model_spec("codex/openai");
    assert_eq!(tool, "codex");
    assert_eq!(provider, "openai");
    assert_eq!(model, "default");
    assert_eq!(thinking, "none");
}

#[test]
fn test_tool_exe_name_mapping() {
    let config: ProjectConfig = toml::from_str("").unwrap();

    assert_eq!(tool_exe_name("gemini-cli", &config), "gemini");
    // codex now defaults to CLI transport (#760 / #1128 transport flip);
    // the CLI binary is `codex`, not `codex-acp`.
    assert_eq!(tool_exe_name("codex", &config), "codex");
    // claude-code now defaults to CLI transport (#1115/#1117 workaround);
    // the CLI binary is `claude`, not `claude-code-acp`.
    assert_eq!(tool_exe_name("claude-code", &config), "claude");
    assert_eq!(tool_exe_name("opencode", &config), "opencode");
    assert_eq!(tool_exe_name("hermes", &config), "hermes");
    assert_eq!(tool_exe_name("antigravity-cli", &config), "antigravity");
    assert_eq!(tool_exe_name("unknown-tool", &config), "unknown-tool");
}

#[test]
fn test_build_operation_routing_no_tier() {
    let config: ProjectConfig = toml::from_str(
        r#"
            [tiers.tier-1]
            description = "Test"
            models = ["gemini-cli/google/default/xhigh"]
            "#,
    )
    .unwrap();

    let routing = build_operation_routing(&config, &shipped_catalog(), "run", None, "none");
    assert!(routing.tier_name.is_none());
    assert!(routing.entries.is_empty());
    assert_eq!(routing.source, "not configured");
}

#[test]
fn test_build_operation_routing_with_tier() {
    let config: ProjectConfig = toml::from_str(
        r#"
            [tiers.tier-2-standard]
            description = "Standard tasks"
            models = [
                "gemini-cli/google/default/xhigh",
                "codex/openai/gpt-5.4/high",
            ]
            "#,
    )
    .unwrap();

    let routing = build_operation_routing(
        &config,
        &shipped_catalog(),
        "run",
        Some("tier-2-standard"),
        "tier_mapping.default",
    );
    assert_eq!(routing.tier_name.as_deref(), Some("tier-2-standard"));
    assert_eq!(routing.tier_description.as_deref(), Some("Standard tasks"));
    assert_eq!(routing.entries.len(), 2);
    assert_eq!(routing.entries[0].tool, "gemini-cli");
    assert_eq!(routing.entries[0].rank, 1);
    assert_eq!(routing.entries[1].tool, "codex");
    assert_eq!(routing.entries[1].rank, 2);
}

#[test]
fn collect_routing_tables_prefers_project_review_and_debate_tiers() {
    let config: ProjectConfig = toml::from_str(
        r#"
[review]
tier = "project-review"

[debate]
tier = "project-debate"

[tiers.project-review]
description = "Project review"
models = ["codex/openai/gpt-5.5/xhigh"]

[tiers.project-debate]
description = "Project debate"
models = ["codex/openai/gpt-5.5/xhigh"]
"#,
    )
    .expect("project config");
    let mut global = GlobalConfig::default();
    global.review.tier = Some("global-review".to_string());
    global.debate.tier = Some("global-debate".to_string());

    let tables = collect_routing_tables(&config, &global, &shipped_catalog());
    let review = tables
        .iter()
        .find(|table| table.operation == "review")
        .expect("review table");
    let debate = tables
        .iter()
        .find(|table| table.operation == "debate")
        .expect("debate table");

    assert_eq!(review.tier_name.as_deref(), Some("project-review"));
    assert_eq!(debate.tier_name.as_deref(), Some("project-debate"));
}

#[test]
fn test_build_operation_routing_missing_tier() {
    let config: ProjectConfig = toml::from_str(
        r#"
            [tiers.tier-1]
            description = "Test"
            models = ["gemini-cli/google/default/xhigh"]
            "#,
    )
    .unwrap();

    let routing = build_operation_routing(
        &config,
        &shipped_catalog(),
        "review",
        Some("nonexistent"),
        "review.tier",
    );
    assert_eq!(routing.tier_name.as_deref(), Some("nonexistent"));
    assert!(routing.tier_description.is_none());
    assert!(routing.entries.is_empty());
}

#[test]
fn test_build_operation_routing_disabled_tool() {
    let config: ProjectConfig = toml::from_str(
        r#"
            [tools.codex]
            enabled = false

            [tiers.tier-1]
            description = "Test"
            models = [
                "gemini-cli/google/default/xhigh",
                "codex/openai/gpt-5.4/high",
            ]
            "#,
    )
    .unwrap();

    let routing = build_operation_routing(
        &config,
        &shipped_catalog(),
        "run",
        Some("tier-1"),
        "tier_mapping.default",
    );
    assert_eq!(routing.entries.len(), 2);
    assert!(routing.entries[0].enabled); // gemini-cli enabled by default
    assert!(!routing.entries[1].enabled); // codex explicitly disabled
}

#[test]
fn test_routing_entry_status_symbol() {
    let ready = RoutingEntry {
        rank: 1,
        tool: "test".into(),
        provider: "p".into(),
        model: "m".into(),
        thinking: "t".into(),
        enabled: true,
        binary_available: true,
        catalog_valid: true,
        catalog_source: "test catalog".into(),
        admission_status: "admitted".into(),
    };
    assert_eq!(ready.status_symbol(), "✓ ready");

    let disabled = RoutingEntry {
        enabled: false,
        ..ready
    };
    assert_eq!(disabled.status_symbol(), "✗ disabled");

    let missing = RoutingEntry {
        rank: 1,
        tool: "test".into(),
        provider: "p".into(),
        model: "m".into(),
        thinking: "t".into(),
        enabled: true,
        binary_available: false,
        catalog_valid: true,
        catalog_source: "test catalog".into(),
        admission_status: "admitted".into(),
    };
    assert_eq!(missing.status_symbol(), "✗ missing");
}

#[test]
fn test_routing_strategy_display() {
    let config: ProjectConfig = toml::from_str(
        r#"
            [tiers.tier-rr]
            description = "Round-robin tier"
            strategy = "round-robin"
            models = ["gemini-cli/google/default/xhigh"]
            "#,
    )
    .unwrap();

    let routing =
        build_operation_routing(&config, &shipped_catalog(), "run", Some("tier-rr"), "test");
    assert_eq!(routing.strategy, TierStrategy::RoundRobin);
}

#[test]
fn doctor_routing_reports_invalid_effective_config_and_renders_partial_context() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let td = tempfile::tempdir().expect("tempdir");
    let config_root = td.path().join("xdg-config");
    std::fs::create_dir_all(&config_root).expect("create config root");
    std::fs::create_dir(td.path().join(".git")).expect("create .git");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);

    let user_config_path = ProjectConfig::user_config_path().expect("resolve user config path");
    std::fs::create_dir_all(user_config_path.parent().expect("user config dir"))
        .expect("create user config dir");
    // Use opencode + acp as the still-invalid combination after #1128 flipped
    // codex CLI to a legal value. opencode has no ACP transport, so the merge
    // still produces a validation error tagged on the offending key. The invalid
    // value lives in USER config; project config stays valid so the project view
    // remains Valid in isolation.
    std::fs::write(
        &user_config_path,
        r#"
[tools.opencode]
transport = "acp"
"#,
    )
    .expect("write invalid user config");

    write_project_config(
        td.path(),
        r#"
[tools.opencode]
transport = "auto"

[tiers.tier-1]
description = "Test"
models = ["codex/openai/gpt-5.4/high"]

[tier_mapping]
default = "tier-1"
"#,
    );

    let report = build_routing_report(td.path(), None, None)
        .expect("doctor routing should keep running when effective config is invalid");
    let rendered = render_routing_text(&report);
    let json: serde_json::Value = serde_json::from_str(
        render_routing_json(&report)
            .expect("render routing json")
            .trim(),
    )
    .expect("parse routing json");

    assert_eq!(report.tables.len(), 0);
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(report.diagnostics[0].key, "effective-config");
    assert!(
        report.diagnostics[0]
            .message
            .contains("tools.opencode.transport"),
        "diagnostic should surface the merged-config key: {:?}",
        report.diagnostics[0]
    );
    assert!(
        rendered.contains("[error] effective-config: failed to load project config"),
        "text output should surface the effective-config diagnostic: {rendered}"
    );
    assert!(
        rendered.contains("Built-in tools: opencode, codex, claude-code"),
        "text output should keep best-effort routing context: {rendered}"
    );
    assert!(
        rendered.contains("VCS backend: git"),
        "text output should render VCS detection without valid effective config: {rendered}"
    );
    assert!(
        rendered.contains("unable to render project-specific routing"),
        "text output should explain why project routing is partial: {rendered}"
    );

    assert_eq!(json["routing"], serde_json::json!([]));
    assert_eq!(
        json["diagnostics"][0]["key"],
        serde_json::json!("effective-config")
    );
    assert!(
        json["diagnostics"][0]["message"]
            .as_str()
            .expect("routing json diagnostic message")
            .contains("tools.opencode.transport"),
        "json output should surface the merged-config key: {json}"
    );
    assert_eq!(json["context"]["vcs_backend"], serde_json::json!("git"));
    assert_eq!(
        json["project_hint"],
        serde_json::json!(
            "unable to render project-specific routing; see effective-config diagnostic"
        )
    );
}
