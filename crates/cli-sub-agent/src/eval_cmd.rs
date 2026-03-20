//! Handler for `csa eval` subcommand.

use anyhow::Result;

pub fn handle_eval(project: Option<String>, days: u32, json: bool) -> Result<()> {
    let state_dir = csa_config::paths::state_dir()
        .ok_or_else(|| anyhow::anyhow!("cannot determine state directory"))?;

    let project_key = match project {
        Some(key) => key,
        None => {
            let cwd = std::env::current_dir()?;
            crate::pipeline_project_key::resolve_memory_project_key(&cwd)
                .ok_or_else(|| anyhow::anyhow!("cannot determine project key from CWD"))?
        }
    };

    // Convert project key to storage key format (same as session manager)
    let storage_key = project_key
        .trim_start_matches('/')
        .replace('/', std::path::MAIN_SEPARATOR_STR);

    let sessions = csa_eval::scan_sessions(&state_dir, &storage_key, days)?;

    if sessions.is_empty() {
        println!("No sessions found");
        return Ok(());
    }

    let report = csa_eval::build_report(&storage_key, days, &sessions);

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_text_report(&report);
    }

    Ok(())
}

fn print_text_report(report: &csa_eval::EvalReport) {
    println!("=== CSA Eval Report ===");
    println!("Project:    {}", report.project_key);
    println!("Period:     {} days", report.period_days);
    println!("Sessions:   {}", report.sessions_analyzed);
    println!();

    // Token stats
    println!("--- Token Usage ---");
    println!(
        "Total input:      {}",
        format_tokens(report.token_stats.total_input)
    );
    println!(
        "Total output:     {}",
        format_tokens(report.token_stats.total_output)
    );
    println!(
        "Avg per session:  {:.0}",
        report.token_stats.avg_per_session
    );
    println!(
        "Est. cost (USD):  ${:.4}",
        report.token_stats.estimated_cost_usd
    );
    println!();

    // Failure patterns
    if report.failure_patterns.is_empty() {
        println!("No failure patterns detected.");
    } else {
        println!("--- Failure Patterns ---");
        for pattern in &report.failure_patterns {
            let tool = pattern.tool_involved.as_deref().unwrap_or("(any)");
            println!(
                "  {:<15} tool={:<15} count={}  examples: {}",
                pattern.category,
                tool,
                pattern.count,
                pattern.example_session_ids.join(", ")
            );
        }
    }
}

fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}
