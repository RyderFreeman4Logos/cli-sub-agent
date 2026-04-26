//! Routing diagnostic for `csa doctor routing`.
//!
//! Shows the complete routing table for all operation types (run, review,
//! debate), including effective tier, tool availability, and failover order.

use anyhow::Result;
use csa_config::{GlobalConfig, ProjectConfig, TierStrategy};
use csa_core::types::OutputFormat;
use std::{env, fmt::Write as _, path::Path};

const BUILTIN_TOOLS: &[&str] = &["gemini-cli", "opencode", "codex", "claude-code"];

/// Map tool name (from model spec) to its executable binary name.
fn tool_exe_name(tool: &str, config: &ProjectConfig) -> String {
    crate::run_helpers::resolved_tool_binary_name(tool, Some(config))
        .unwrap_or(tool)
        .to_string()
}

/// Check if a tool binary is available in PATH.
fn is_tool_binary_available(tool: &str, config: &ProjectConfig) -> bool {
    let _binary_name = tool_exe_name(tool, config);
    crate::run_helpers::is_tool_binary_available_for_config(tool, Some(config))
}

/// A single entry in the routing table for display.
#[derive(Debug, Clone)]
struct RoutingEntry {
    rank: usize,
    tool: String,
    provider: String,
    model: String,
    thinking: String,
    enabled: bool,
    binary_available: bool,
}

impl RoutingEntry {
    fn status_symbol(&self) -> &str {
        if !self.enabled {
            "✗ disabled"
        } else if !self.binary_available {
            "✗ missing"
        } else {
            "✓ ready"
        }
    }
}

/// Parse a model spec string (tool/provider/model/thinking) into components.
fn parse_model_spec(spec: &str) -> (String, String, String, String) {
    let parts: Vec<&str> = spec.splitn(4, '/').collect();
    (
        parts.first().copied().unwrap_or("?").to_string(),
        parts.get(1).copied().unwrap_or("?").to_string(),
        parts.get(2).copied().unwrap_or("default").to_string(),
        parts.get(3).copied().unwrap_or("none").to_string(),
    )
}

/// Resolved routing for a single operation type.
#[derive(Debug)]
struct OperationRouting {
    operation: String,
    tier_name: Option<String>,
    tier_description: Option<String>,
    strategy: TierStrategy,
    entries: Vec<RoutingEntry>,
    source: String,
}

#[derive(Debug)]
struct RoutingDiagnostic {
    key: &'static str,
    message: String,
}

#[derive(Debug)]
struct RoutingContext {
    built_in_tools: Vec<String>,
    global_config_source: &'static str,
    vcs_backend: Option<String>,
}

#[derive(Debug)]
struct RoutingReport {
    diagnostics: Vec<RoutingDiagnostic>,
    context: RoutingContext,
    tables: Vec<OperationRouting>,
    project_hint: Option<String>,
}

/// Build routing info for an operation from its tier configuration.
fn build_operation_routing(
    config: &ProjectConfig,
    operation: &str,
    tier_name: Option<&str>,
    source: &str,
) -> OperationRouting {
    let Some(tier_name) = tier_name else {
        return OperationRouting {
            operation: operation.to_string(),
            tier_name: None,
            tier_description: None,
            strategy: TierStrategy::default(),
            entries: Vec::new(),
            source: "not configured".to_string(),
        };
    };

    let resolved = config.resolve_tier_selector(tier_name);
    let tier_key = resolved.as_deref().unwrap_or(tier_name);
    let tier = config.tiers.get(tier_key);

    match tier {
        Some(tier_cfg) => {
            let entries: Vec<RoutingEntry> = tier_cfg
                .models
                .iter()
                .enumerate()
                .map(|(i, spec)| {
                    let (tool, provider, model, thinking) = parse_model_spec(spec);
                    let enabled = config.is_tool_enabled(&tool);
                    let binary_available = is_tool_binary_available(&tool, config);
                    RoutingEntry {
                        rank: i + 1,
                        tool,
                        provider,
                        model,
                        thinking,
                        enabled,
                        binary_available,
                    }
                })
                .collect();
            OperationRouting {
                operation: operation.to_string(),
                tier_name: Some(tier_key.to_string()),
                tier_description: Some(tier_cfg.description.clone()),
                strategy: tier_cfg.strategy,
                entries,
                source: source.to_string(),
            }
        }
        None => OperationRouting {
            operation: operation.to_string(),
            tier_name: Some(tier_name.to_string()),
            tier_description: None,
            strategy: TierStrategy::default(),
            entries: Vec::new(),
            source: format!("tier '{}' not found", tier_name),
        },
    }
}

/// Collect routing tables for all operation types.
fn collect_routing_tables(config: &ProjectConfig, global: &GlobalConfig) -> Vec<OperationRouting> {
    let mut tables = Vec::new();

    // run: uses tier_mapping "default" as the default tier
    let run_tier = config.tier_mapping.get("default").map(|s| s.as_str());
    tables.push(build_operation_routing(
        config,
        "run",
        run_tier,
        "tier_mapping.default",
    ));

    // review: uses review.tier from config
    let review_tier = global.review.tier.as_deref();
    tables.push(build_operation_routing(
        config,
        "review",
        review_tier,
        "review.tier",
    ));

    // debate: uses debate.tier from config
    let debate_tier = global.debate.tier.as_deref();
    tables.push(build_operation_routing(
        config,
        "debate",
        debate_tier,
        "debate.tier",
    ));

    tables
}

fn detect_vcs_backend(project_root: &Path) -> Option<String> {
    csa_core::vcs::detect_vcs_kind(project_root).map(|kind| kind.to_string())
}

fn build_routing_report(
    project_root: &Path,
    operation_filter: Option<String>,
    tier_filter: Option<String>,
) -> Result<RoutingReport> {
    if let Some(ref op) = operation_filter
        && !matches!(op.as_str(), "run" | "review" | "debate")
    {
        anyhow::bail!(
            "Unknown operation '{}'. Valid operations: run, review, debate",
            op
        );
    }

    let (global, global_config_source) = match GlobalConfig::load() {
        Ok(cfg) => (cfg, "loaded"),
        Err(_) => (
            GlobalConfig::default(),
            "defaults (global config unavailable)",
        ),
    };
    let context = RoutingContext {
        built_in_tools: BUILTIN_TOOLS
            .iter()
            .map(|tool| (*tool).to_string())
            .collect(),
        global_config_source,
        vcs_backend: detect_vcs_backend(project_root),
    };

    let config = match ProjectConfig::load(project_root) {
        Ok(Some(config)) => Some(config),
        Ok(None) => None,
        Err(error) => {
            return Ok(RoutingReport {
                diagnostics: vec![RoutingDiagnostic {
                    key: "effective-config",
                    message: format!("failed to load project config: {error:#}"),
                }],
                context,
                tables: Vec::new(),
                project_hint: Some(
                    "unable to render project-specific routing; see effective-config diagnostic"
                        .to_string(),
                ),
            });
        }
    };

    let Some(config) = config else {
        anyhow::bail!("No configuration found. Run 'csa init' first.");
    };

    let mut tables = collect_routing_tables(&config, &global);

    if let Some(ref op) = operation_filter {
        tables.retain(|table| table.operation == *op);
    }

    if let Some(ref tier) = tier_filter {
        let resolved = config
            .resolve_tier_selector(tier)
            .unwrap_or_else(|| tier.clone());
        tables.retain(|table| table.tier_name.as_deref() == Some(resolved.as_str()));
    }

    let project_hint = if config.tiers.is_empty() {
        Some("no tiers configured; routing diagnostic requires tier configuration".to_string())
    } else {
        None
    };

    Ok(RoutingReport {
        diagnostics: Vec::new(),
        context,
        tables,
        project_hint,
    })
}

/// Run routing diagnostic.
pub async fn run_doctor_routing(
    format: OutputFormat,
    operation_filter: Option<String>,
    tier_filter: Option<String>,
) -> Result<()> {
    let cwd = env::current_dir()?;
    let report = build_routing_report(&cwd, operation_filter, tier_filter)?;

    match format {
        OutputFormat::Text => print!("{}", render_routing_text(&report)),
        OutputFormat::Json => print!("{}", render_routing_json(&report)?),
    }

    Ok(())
}

/// Render routing tables as human-readable text.
fn render_routing_text(report: &RoutingReport) -> String {
    let mut output = String::new();

    if !report.diagnostics.is_empty() {
        writeln!(&mut output, "=== Diagnostics ===").expect("write diagnostics header");
        for diagnostic in &report.diagnostics {
            writeln!(
                &mut output,
                "[error] {}: {}",
                diagnostic.key, diagnostic.message
            )
            .expect("write diagnostic");
        }
        writeln!(&mut output).expect("write diagnostics spacer");
    }

    writeln!(&mut output, "=== Routing Context ===").expect("write routing context header");
    writeln!(
        &mut output,
        "Built-in tools: {}",
        report.context.built_in_tools.join(", ")
    )
    .expect("write built-in tools");
    writeln!(
        &mut output,
        "Global config: {}",
        report.context.global_config_source
    )
    .expect("write global config source");
    writeln!(
        &mut output,
        "VCS backend: {}",
        report
            .context
            .vcs_backend
            .as_deref()
            .unwrap_or("not detected")
    )
    .expect("write vcs backend");
    writeln!(&mut output).expect("write routing context spacer");

    if let Some(hint) = &report.project_hint {
        writeln!(&mut output, "=== Project Routing ===").expect("write project routing header");
        writeln!(&mut output, "{hint}").expect("write project routing hint");
        if report.tables.is_empty() {
            return output;
        }
        writeln!(&mut output).expect("write project routing spacer");
    }

    for table in &report.tables {
        writeln!(&mut output, "=== Routing: {} ===", table.operation)
            .expect("write routing table header");

        match (&table.tier_name, &table.tier_description) {
            (Some(name), Some(desc)) => {
                let strategy_label = match table.strategy {
                    TierStrategy::Priority => "priority",
                    TierStrategy::RoundRobin => "round-robin",
                };
                writeln!(&mut output, "Tier: {name} ({desc})").expect("write tier");
                writeln!(&mut output, "Strategy: {strategy_label}").expect("write strategy");
                writeln!(&mut output, "Source: {}", table.source).expect("write source");
            }
            (Some(name), None) => {
                writeln!(&mut output, "Tier: {name} (NOT FOUND)").expect("write missing tier");
                writeln!(&mut output, "Source: {}", table.source).expect("write source");
            }
            (None, _) => {
                writeln!(&mut output, "Tier: (none) — {}", table.source).expect("write no tier");
            }
        }

        if table.entries.is_empty() {
            writeln!(&mut output).expect("write empty entries spacer");
            continue;
        }

        writeln!(&mut output).expect("write table spacer");
        writeln!(
            &mut output,
            "  {:<3} {:<13} {:<11} {:<25} {:<10} Status",
            "#", "Tool", "Provider", "Model", "Thinking"
        )
        .expect("write table heading");

        for entry in &table.entries {
            writeln!(
                &mut output,
                "  {:<3} {:<13} {:<11} {:<25} {:<10} {}",
                entry.rank,
                entry.tool,
                entry.provider,
                entry.model,
                entry.thinking,
                entry.status_symbol(),
            )
            .expect("write routing entry");
        }

        // Failover order (only enabled + available tools)
        let failover: Vec<&str> = table
            .entries
            .iter()
            .filter(|e| e.enabled && e.binary_available)
            .map(|e| e.tool.as_str())
            .collect();
        if !failover.is_empty() {
            writeln!(&mut output).expect("write failover spacer");
            writeln!(&mut output, "Failover order: {}", failover.join(" → "))
                .expect("write failover order");
        }

        writeln!(&mut output).expect("write table trailer");
    }

    output
}

/// Render routing tables as JSON.
fn render_routing_json(report: &RoutingReport) -> Result<String> {
    let routing: Vec<serde_json::Value> = report
        .tables
        .iter()
        .map(|table| {
            let entries: Vec<serde_json::Value> = table
                .entries
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "rank": e.rank,
                        "tool": e.tool,
                        "provider": e.provider,
                        "model": e.model,
                        "thinking": e.thinking,
                        "enabled": e.enabled,
                        "binary_available": e.binary_available,
                        "ready": e.enabled && e.binary_available,
                    })
                })
                .collect();

            let failover: Vec<&str> = table
                .entries
                .iter()
                .filter(|e| e.enabled && e.binary_available)
                .map(|e| e.tool.as_str())
                .collect();

            let strategy = match table.strategy {
                TierStrategy::Priority => "priority",
                TierStrategy::RoundRobin => "round-robin",
            };

            serde_json::json!({
                "operation": table.operation,
                "tier": table.tier_name,
                "description": table.tier_description,
                "strategy": strategy,
                "source": table.source,
                "models": entries,
                "failover_order": failover,
            })
        })
        .collect();

    let diagnostics: Vec<serde_json::Value> = report
        .diagnostics
        .iter()
        .map(|diagnostic| {
            serde_json::json!({
                "severity": "error",
                "key": diagnostic.key,
                "message": diagnostic.message,
            })
        })
        .collect();

    let output = serde_json::json!({
        "diagnostics": diagnostics,
        "context": {
            "built_in_tools": report.context.built_in_tools,
            "global_config": report.context.global_config_source,
            "vcs_backend": report.context.vcs_backend,
        },
        "project_hint": report.project_hint,
        "routing": routing,
    });
    Ok(format!("{}\n", serde_json::to_string_pretty(&output)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env_lock::TEST_ENV_LOCK;
    use std::{ffi::OsString, path::Path};

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

        let routing = build_operation_routing(&config, "run", None, "none");
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
    fn test_build_operation_routing_missing_tier() {
        let config: ProjectConfig = toml::from_str(
            r#"
            [tiers.tier-1]
            description = "Test"
            models = ["gemini-cli/google/default/xhigh"]
            "#,
        )
        .unwrap();

        let routing =
            build_operation_routing(&config, "review", Some("nonexistent"), "review.tier");
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

        let routing =
            build_operation_routing(&config, "run", Some("tier-1"), "tier_mapping.default");
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

        let routing = build_operation_routing(&config, "run", Some("tier-rr"), "test");
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
        // Use gemini-cli + acp as the still-invalid combination after #1128 flipped
        // codex CLI to a legal value. gemini-cli has no ACP transport, so the merge
        // still produces a validation error tagged on the offending key. The invalid
        // value lives in USER config; project config stays valid so the project view
        // remains Valid in isolation.
        std::fs::write(
            &user_config_path,
            r#"
[tools.gemini-cli]
transport = "acp"
"#,
        )
        .expect("write invalid user config");

        write_project_config(
            td.path(),
            r#"
[tools.gemini-cli]
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
                .contains("tools.gemini-cli.transport"),
            "diagnostic should surface the merged-config key: {:?}",
            report.diagnostics[0]
        );
        assert!(
            rendered.contains("[error] effective-config: failed to load project config"),
            "text output should surface the effective-config diagnostic: {rendered}"
        );
        assert!(
            rendered.contains("Built-in tools: gemini-cli, opencode, codex, claude-code"),
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
                .contains("tools.gemini-cli.transport"),
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
}
