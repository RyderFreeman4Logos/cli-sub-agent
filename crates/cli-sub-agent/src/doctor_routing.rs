//! Routing diagnostic for `csa doctor routing`.
//!
//! Shows the complete routing table for all operation types (run, review,
//! debate), including effective tier, tool availability, and failover order.

use anyhow::Result;
use csa_config::{EffectiveModelCatalog, GlobalConfig, ProjectConfig, TierStrategy};
use csa_core::types::{OutputFormat, PRIMARY_TOOL_NAMES};
use std::{env, fmt::Write as _, path::Path};

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
    catalog_valid: bool,
    catalog_source: String,
    admission_status: String,
}

impl RoutingEntry {
    fn status_symbol(&self) -> &str {
        if !self.enabled {
            "✗ disabled"
        } else if !self.catalog_valid {
            "✗ catalog"
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
    catalog: &EffectiveModelCatalog,
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
                    let (catalog_valid, catalog_source, admission_status) =
                        match catalog.validate_parts(&tool, &provider, &model, &thinking) {
                            Ok(admission) => {
                                let status = admission.warning().map_or_else(
                                    || "admitted by effective model catalog".to_string(),
                                    |warning| format!("admitted with warning: {warning}"),
                                );
                                (true, admission.source_label(), status)
                            }
                            Err(error) => (
                                false,
                                error.source_label().to_string(),
                                format!("rejected by effective model catalog: {error}"),
                            ),
                        };
                    RoutingEntry {
                        rank: i + 1,
                        tool,
                        provider,
                        model,
                        thinking,
                        enabled,
                        binary_available,
                        catalog_valid,
                        catalog_source,
                        admission_status,
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
fn collect_routing_tables(
    config: &ProjectConfig,
    global: &GlobalConfig,
    catalog: &EffectiveModelCatalog,
) -> Vec<OperationRouting> {
    let mut tables = Vec::new();

    // run: uses tier_mapping "default" as the default tier
    let run_tier = config.tier_mapping.get("default").map(|s| s.as_str());
    tables.push(build_operation_routing(
        config,
        catalog,
        "run",
        run_tier,
        "tier_mapping.default",
    ));

    // review: uses review.tier from config
    let review_tier = config
        .review
        .as_ref()
        .and_then(|review| review.tier.as_deref())
        .or(global.review.tier.as_deref());
    tables.push(build_operation_routing(
        config,
        catalog,
        "review",
        review_tier,
        "review.tier",
    ));

    // debate: uses debate.tier from config
    let debate_tier = config
        .debate
        .as_ref()
        .and_then(|debate| debate.tier.as_deref())
        .or(global.debate.tier.as_deref());
    tables.push(build_operation_routing(
        config,
        catalog,
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

    let built_in_tools = PRIMARY_TOOL_NAMES
        .iter()
        .map(|tool| (*tool).to_string())
        .collect::<Vec<_>>();
    let vcs_backend = detect_vcs_backend(project_root);
    let effective = match csa_config::EffectiveConfig::load(project_root) {
        Ok(effective) => effective,
        Err(error) => {
            return Ok(RoutingReport {
                diagnostics: vec![RoutingDiagnostic {
                    key: "effective-config",
                    message: format!("failed to load project config: {error:#}"),
                }],
                context: RoutingContext {
                    built_in_tools,
                    global_config_source: "unavailable",
                    vcs_backend,
                },
                tables: Vec::new(),
                project_hint: Some(
                    "unable to render project-specific routing; see effective-config diagnostic"
                        .to_string(),
                ),
            });
        }
    };
    let context = RoutingContext {
        built_in_tools,
        global_config_source: "loaded",
        vcs_backend,
    };
    let global = effective.global;
    let catalog = effective.model_catalog;
    let config = effective.project;

    let Some(config) = config else {
        anyhow::bail!("No configuration found. Run 'csa init' first.");
    };

    let mut tables = collect_routing_tables(&config, &global, &catalog);

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
            .filter(|e| e.enabled && e.binary_available && e.catalog_valid)
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
                        "catalog_valid": e.catalog_valid,
                        "catalog_source": e.catalog_source,
                        "admission_status": e.admission_status,
                        "ready": e.enabled && e.binary_available && e.catalog_valid,
                    })
                })
                .collect();

            let failover: Vec<&str> = table
                .entries
                .iter()
                .filter(|e| e.enabled && e.binary_available && e.catalog_valid)
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
#[path = "doctor_routing_tests.rs"]
mod tests;
