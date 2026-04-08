//! Routing diagnostic for `csa doctor routing`.
//!
//! Shows the complete routing table for all operation types (run, review,
//! debate), including effective tier, tool availability, and failover order.

use anyhow::Result;
use csa_config::{GlobalConfig, ProjectConfig, TierStrategy};
use csa_core::types::OutputFormat;
use std::env;
use std::process::Command;

/// Map tool name (from model spec) to its executable binary name.
fn tool_exe_name(tool: &str) -> &str {
    match tool {
        "gemini-cli" => "gemini",
        "opencode" => "opencode",
        "codex" => "codex-acp",
        "claude-code" => "claude-code-acp",
        _ => tool,
    }
}

/// Check if a tool binary is available in PATH.
fn is_tool_binary_available(tool: &str) -> bool {
    let exe = tool_exe_name(tool);
    Command::new("which")
        .arg(exe)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
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
                    let binary_available = is_tool_binary_available(&tool);
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

/// Run routing diagnostic.
pub async fn run_doctor_routing(
    format: OutputFormat,
    operation_filter: Option<String>,
    tier_filter: Option<String>,
) -> Result<()> {
    let cwd = env::current_dir()?;
    let config = ProjectConfig::load(&cwd)?
        .ok_or_else(|| anyhow::anyhow!("No configuration found. Run 'csa init' first."))?;
    let global = GlobalConfig::load().unwrap_or_default();

    if config.tiers.is_empty() {
        match format {
            OutputFormat::Json => println!(r#"{{"routing":[]}}"#),
            OutputFormat::Text => {
                eprintln!("No tiers configured. Routing diagnostic requires tier configuration.");
            }
        }
        return Ok(());
    }

    let mut tables = collect_routing_tables(&config, &global);

    // Apply operation filter
    if let Some(ref op) = operation_filter {
        tables.retain(|t| t.operation == *op);
        if tables.is_empty() {
            anyhow::bail!(
                "Unknown operation '{}'. Valid operations: run, review, debate",
                op
            );
        }
    }

    // Apply tier filter
    if let Some(ref tier) = tier_filter {
        let resolved = config
            .resolve_tier_selector(tier)
            .unwrap_or_else(|| tier.clone());
        tables.retain(|t| t.tier_name.as_deref() == Some(resolved.as_str()));
    }

    match format {
        OutputFormat::Text => print_routing_text(&tables),
        OutputFormat::Json => print_routing_json(&tables)?,
    }

    Ok(())
}

/// Render routing tables as human-readable text.
fn print_routing_text(tables: &[OperationRouting]) {
    for table in tables {
        println!("=== Routing: {} ===", table.operation);

        match (&table.tier_name, &table.tier_description) {
            (Some(name), Some(desc)) => {
                let strategy_label = match table.strategy {
                    TierStrategy::Priority => "priority",
                    TierStrategy::RoundRobin => "round-robin",
                };
                println!("Tier: {name} ({desc})");
                println!("Strategy: {strategy_label}");
                println!("Source: {}", table.source);
            }
            (Some(name), None) => {
                println!("Tier: {name} (NOT FOUND)");
                println!("Source: {}", table.source);
            }
            (None, _) => {
                println!("Tier: (none) — {}", table.source);
            }
        }

        if table.entries.is_empty() {
            println!();
            continue;
        }

        println!();
        println!(
            "  {:<3} {:<13} {:<11} {:<25} {:<10} Status",
            "#", "Tool", "Provider", "Model", "Thinking"
        );

        for entry in &table.entries {
            println!(
                "  {:<3} {:<13} {:<11} {:<25} {:<10} {}",
                entry.rank,
                entry.tool,
                entry.provider,
                entry.model,
                entry.thinking,
                entry.status_symbol(),
            );
        }

        // Failover order (only enabled + available tools)
        let failover: Vec<&str> = table
            .entries
            .iter()
            .filter(|e| e.enabled && e.binary_available)
            .map(|e| e.tool.as_str())
            .collect();
        if !failover.is_empty() {
            println!();
            println!("Failover order: {}", failover.join(" → "));
        }

        println!();
    }
}

/// Render routing tables as JSON.
fn print_routing_json(tables: &[OperationRouting]) -> Result<()> {
    let routing: Vec<serde_json::Value> = tables
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

    let output = serde_json::json!({ "routing": routing });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(tool_exe_name("gemini-cli"), "gemini");
        assert_eq!(tool_exe_name("codex"), "codex-acp");
        assert_eq!(tool_exe_name("claude-code"), "claude-code-acp");
        assert_eq!(tool_exe_name("opencode"), "opencode");
        assert_eq!(tool_exe_name("unknown-tool"), "unknown-tool");
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
}
