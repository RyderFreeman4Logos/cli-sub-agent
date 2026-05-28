//! Workspace token health analysis.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use clap::Args;
use serde::Serialize;
use tokuin::tokenizers::{ConservativeTokenizer, Tokenizer};

const DEFAULT_MODEL: &str = "gpt-4o";

#[derive(Debug, Clone, Args)]
pub struct HealthArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,

    /// Token threshold for BLOCK classification
    #[arg(long, default_value_t = 8000)]
    pub threshold: usize,

    /// Token threshold for WARNING classification
    #[arg(long, default_value_t = 6000)]
    pub warning: usize,

    /// Comma-separated file extensions to scan
    #[arg(long, value_delimiter = ',', default_value = "rs")]
    pub extensions: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum HealthStatus {
    Block,
    Warning,
    Ok,
}

#[derive(Debug, Serialize)]
struct HealthFile {
    path: String,
    tokens: usize,
    status: HealthStatus,
}

#[derive(Debug, Serialize)]
struct HealthSummary {
    block: usize,
    warning: usize,
    ok: usize,
    total: usize,
    mean: usize,
    max: usize,
}

#[derive(Debug, Serialize)]
struct HealthOutput<'a> {
    workspace: &'a str,
    threshold: usize,
    warning_threshold: usize,
    files: &'a [HealthFile],
    summary: &'a HealthSummary,
}

pub fn handle_health(args: HealthArgs) -> Result<()> {
    if args.warning >= args.threshold {
        anyhow::bail!(
            "--warning must be lower than --threshold (got warning={}, threshold={})",
            args.warning,
            args.threshold
        );
    }

    let extensions = normalize_extensions(&args.extensions)?;
    let tracked_files = list_tracked_files()?;
    let crate_count = workspace_crate_count(&tracked_files);
    let tokenizer = ConservativeTokenizer::new(DEFAULT_MODEL).map_err(|e| {
        anyhow::anyhow!("Failed to create tokenizer for model '{DEFAULT_MODEL}': {e}")
    })?;

    let mut files = Vec::new();
    for path in tracked_files {
        if should_skip_file(&path) || !matches_extension(&path, &extensions) {
            continue;
        }

        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read file: {}", path.display()))?;
        let tokens = tokenizer
            .count_tokens(&content)
            .map_err(|e| anyhow::anyhow!("Failed to count tokens for {}: {e}", path.display()))?;
        let status = classify(tokens, args.threshold, args.warning);

        files.push(HealthFile {
            path: path.display().to_string(),
            tokens,
            status,
        });
    }

    files.sort_by(|left, right| {
        status_rank(right.status)
            .cmp(&status_rank(left.status))
            .then_with(|| right.tokens.cmp(&left.tokens))
            .then_with(|| left.path.cmp(&right.path))
    });

    let summary = summarize(&files);
    let workspace = workspace_name();

    if args.json {
        print_json(&workspace, args.threshold, args.warning, &files, &summary)?;
    } else {
        print_text(
            &workspace,
            crate_count,
            args.threshold,
            args.warning,
            &files,
            &summary,
        );
    }

    Ok(())
}

fn normalize_extensions(raw: &[String]) -> Result<Vec<String>> {
    let extensions: Vec<String> = raw
        .iter()
        .map(|extension| extension.trim().trim_start_matches('.').to_string())
        .filter(|extension| !extension.is_empty())
        .collect();

    if extensions.is_empty() {
        anyhow::bail!("--extensions must include at least one non-empty extension");
    }

    Ok(extensions)
}

fn list_tracked_files() -> Result<Vec<PathBuf>> {
    let output = Command::new("git")
        .arg("ls-files")
        .output()
        .context("Failed to run git ls-files")?;

    if !output.status.success() {
        anyhow::bail!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(PathBuf::from)
        .collect())
}

fn should_skip_file(path: &Path) -> bool {
    let path_str = path.to_string_lossy();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");

    matches!(
        file_name,
        "AGENTS.md" | "FACTORY.md" | "PATTERN.md" | "SKILL.md" | "workflow.toml"
    ) || file_name.ends_with(".lock")
        || file_name.ends_with("lock.json")
        || file_name.ends_with("lock.yaml")
        || file_name.ends_with("_tests.rs")
        || file_name.ends_with("_test.rs")
        || path_str.starts_with(".test-target/")
        || path_str.contains("/tests/")
        || path_str.contains("/benches/")
}

fn matches_extension(path: &Path, extensions: &[String]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extensions.iter().any(|expected| extension == expected))
        .unwrap_or(false)
}

fn classify(tokens: usize, threshold: usize, warning: usize) -> HealthStatus {
    if tokens > threshold {
        HealthStatus::Block
    } else if tokens > warning {
        HealthStatus::Warning
    } else {
        HealthStatus::Ok
    }
}

fn status_rank(status: HealthStatus) -> u8 {
    match status {
        HealthStatus::Block => 2,
        HealthStatus::Warning => 1,
        HealthStatus::Ok => 0,
    }
}

fn summarize(files: &[HealthFile]) -> HealthSummary {
    let block = files
        .iter()
        .filter(|file| file.status == HealthStatus::Block)
        .count();
    let warning = files
        .iter()
        .filter(|file| file.status == HealthStatus::Warning)
        .count();
    let total = files.len();
    let ok = total.saturating_sub(block + warning);
    let token_total = files.iter().map(|file| file.tokens).sum::<usize>();
    let mean = token_total.checked_div(total).unwrap_or(0);
    let max = files.iter().map(|file| file.tokens).max().unwrap_or(0);

    HealthSummary {
        block,
        warning,
        ok,
        total,
        mean,
        max,
    }
}

fn workspace_name() -> String {
    std::env::current_dir()
        .ok()
        .and_then(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| "workspace".to_string())
}

fn workspace_crate_count(tracked_files: &[PathBuf]) -> usize {
    tracked_files
        .iter()
        .filter(|path| {
            let mut components = path.components();
            matches!(
                (
                    components
                        .next()
                        .and_then(|component| component.as_os_str().to_str()),
                    components.next(),
                    components
                        .next()
                        .and_then(|component| component.as_os_str().to_str()),
                    components.next(),
                ),
                (Some("crates"), Some(_), Some("Cargo.toml"), None)
            )
        })
        .count()
}

fn print_json(
    workspace: &str,
    threshold: usize,
    warning_threshold: usize,
    files: &[HealthFile],
    summary: &HealthSummary,
) -> Result<()> {
    let output = HealthOutput {
        workspace,
        threshold,
        warning_threshold,
        files,
        summary,
    };
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn print_text(
    workspace: &str,
    crate_count: usize,
    threshold: usize,
    warning: usize,
    files: &[HealthFile],
    summary: &HealthSummary,
) {
    println!("Workspace: {workspace} ({crate_count} crates)");
    println!();

    let block_files: Vec<&HealthFile> = files
        .iter()
        .filter(|file| file.status == HealthStatus::Block)
        .collect();
    let warning_files: Vec<&HealthFile> = files
        .iter()
        .filter(|file| file.status == HealthStatus::Warning)
        .collect();
    let width = block_files
        .iter()
        .chain(warning_files.iter())
        .map(|file| file.path.len())
        .max()
        .unwrap_or(0);

    println!("  BLOCK (>{threshold} tokens):");
    print_file_group(&block_files, width);
    println!();

    println!("  WARNING ({warning}-{threshold} tokens):");
    print_file_group(&warning_files, width);
    println!();

    println!(
        "  Summary: {} BLOCK, {} WARNING, {} OK",
        summary.block, summary.warning, summary.ok
    );
    println!(
        "  Mean: {} tokens/file  Max: {}  Over budget: {:.1}%",
        summary.mean,
        summary.max,
        over_budget_percent(summary.block, summary.total)
    );
}

fn print_file_group(files: &[&HealthFile], width: usize) {
    if files.is_empty() {
        println!("    (none)");
        return;
    }

    for file in files {
        println!(
            "    {path:<width$}  {tokens:>6} tokens",
            path = file.path,
            tokens = file.tokens,
        );
    }
}

fn over_budget_percent(block: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        (block as f64 / total as f64) * 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_token_counts_by_budget() {
        assert_eq!(classify(8001, 8000, 6000), HealthStatus::Block);
        assert_eq!(classify(8000, 8000, 6000), HealthStatus::Warning);
        assert_eq!(classify(6001, 8000, 6000), HealthStatus::Warning);
        assert_eq!(classify(6000, 8000, 6000), HealthStatus::Ok);
    }

    #[test]
    fn skips_monolith_exempt_files() {
        for path in [
            "Cargo.lock",
            "AGENTS.md",
            "patterns/demo/SKILL.md",
            "patterns/demo/PATTERN.md",
            "patterns/demo/workflow.toml",
            "crates/demo/src/demo_tests.rs",
            "crates/demo/tests/integration.rs",
            "crates/demo/benches/bench.rs",
        ] {
            assert!(should_skip_file(Path::new(path)), "expected skip: {path}");
        }

        assert!(!should_skip_file(Path::new("crates/demo/src/lib.rs")));
    }

    #[test]
    fn summarizes_file_status_counts() {
        let files = vec![
            HealthFile {
                path: "block.rs".to_string(),
                tokens: 9000,
                status: HealthStatus::Block,
            },
            HealthFile {
                path: "warning.rs".to_string(),
                tokens: 7000,
                status: HealthStatus::Warning,
            },
            HealthFile {
                path: "ok.rs".to_string(),
                tokens: 1000,
                status: HealthStatus::Ok,
            },
        ];

        let summary = summarize(&files);

        assert_eq!(summary.block, 1);
        assert_eq!(summary.warning, 1);
        assert_eq!(summary.ok, 1);
        assert_eq!(summary.total, 3);
        assert_eq!(summary.mean, 5666);
        assert_eq!(summary.max, 9000);
        assert!((over_budget_percent(summary.block, summary.total) - 100.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn keeps_command_entrypoints_reachable_in_test_crates() {
        let _handler: fn(HealthArgs) -> Result<()> = handle_health;
        let _print_json: fn(&str, usize, usize, &[HealthFile], &HealthSummary) -> Result<()> =
            print_json;
        let _print_text: fn(&str, usize, usize, usize, &[HealthFile], &HealthSummary) = print_text;

        assert_eq!(DEFAULT_MODEL, "gpt-4o");
    }
}
