use anyhow::Result;

use csa_config::ProjectConfig;
use csa_core::types::OutputFormat;

/// Handle `csa tiers list`.
pub(crate) fn handle_tiers_list(cd: Option<String>, format: OutputFormat) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let config = ProjectConfig::load(&project_root)?
        .ok_or_else(|| anyhow::anyhow!("No configuration found. Run 'csa init' first."))?;

    if config.tiers.is_empty() {
        match format {
            OutputFormat::Json => {
                println!(r#"{{"tiers":[],"tier_mapping":{{}}}}"#);
            }
            OutputFormat::Text => {
                eprintln!("No tiers configured. Run 'csa init' to generate default tiers.");
            }
        }
        return Ok(());
    }

    match format {
        OutputFormat::Json => print_tiers_json(&config),
        OutputFormat::Text => print_tiers_text(&config),
    }

    Ok(())
}

fn print_tiers_text(config: &ProjectConfig) {
    let mut tier_names: Vec<&String> = config.tiers.keys().collect();
    tier_names.sort();

    for name in &tier_names {
        let tier = &config.tiers[*name];
        println!("{}: {} [round-robin]", name, tier.description);
        for (i, model) in tier.models.iter().enumerate() {
            println!("  {}. {}", i + 1, model);
        }
        println!();
    }

    // Print tier mapping
    if !config.tier_mapping.is_empty() {
        println!("Tier mapping:");
        let mut mappings: Vec<(&String, &String)> = config.tier_mapping.iter().collect();
        mappings.sort_by_key(|(k, _)| *k);
        for (task_type, tier_name) in mappings {
            println!("  {} -> {}", task_type, tier_name);
        }
        println!();
    }

    println!("Note: Models within the same tier use round-robin by default.");
    println!("Escalation: move to the next higher tier.");
}

fn print_tiers_json(config: &ProjectConfig) {
    let mut tier_names: Vec<&String> = config.tiers.keys().collect();
    tier_names.sort();

    let tiers: Vec<serde_json::Value> = tier_names
        .iter()
        .map(|name| {
            let tier = &config.tiers[*name];
            serde_json::json!({
                "name": name,
                "description": tier.description,
                "rotation": "round-robin",
                "models": tier.models,
            })
        })
        .collect();

    let output = serde_json::json!({
        "tiers": tiers,
        "tier_mapping": config.tier_mapping,
    });

    println!("{}", serde_json::to_string_pretty(&output).unwrap());
}
