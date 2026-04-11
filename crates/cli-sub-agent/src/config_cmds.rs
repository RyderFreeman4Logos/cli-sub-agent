use anyhow::Result;
use tracing::{error, warn};

use csa_config::init::init_project;
use csa_config::{GlobalConfig, ProjectConfig, validate_config};
use csa_core::types::OutputFormat;

pub(crate) fn handle_config_show(cd: Option<String>, format: OutputFormat) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let config = ProjectConfig::load(&project_root)?
        .ok_or_else(|| anyhow::anyhow!("No configuration found. Run 'csa init' first."))?;
    let config = config.redacted_for_display();

    match format {
        OutputFormat::Json => {
            let json_str = serde_json::to_string_pretty(&build_project_display_json(&config)?)?;
            println!("{json_str}");
        }
        OutputFormat::Text => {
            let toml_str = toml::to_string_pretty(&build_project_display_toml(&config)?)?;
            print!("{toml_str}");
        }
    }
    Ok(())
}

pub(crate) fn handle_config_edit(cd: Option<String>) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let config_path = ProjectConfig::config_path(&project_root);

    if !config_path.exists() {
        error!("Configuration file does not exist. Run 'csa init' first.");
        return Ok(());
    }

    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let (program, args) = parse_editor_command(&editor)?;
    let status = std::process::Command::new(program)
        .args(args)
        .arg(&config_path)
        .status()?;

    if !status.success() {
        warn!("Editor exited with non-zero status");
    }

    Ok(())
}

fn parse_editor_command(editor: &str) -> Result<(String, Vec<String>)> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_single_quotes = false;
    let mut in_double_quotes = false;
    let mut escaping = false;
    let mut token_started = false;

    for ch in editor.chars() {
        if escaping {
            current.push(ch);
            escaping = false;
            token_started = true;
            continue;
        }

        match ch {
            '\\' if !in_single_quotes => {
                escaping = true;
                token_started = true;
            }
            '\'' if !in_double_quotes => {
                in_single_quotes = !in_single_quotes;
                token_started = true;
            }
            '"' if !in_single_quotes => {
                in_double_quotes = !in_double_quotes;
                token_started = true;
            }
            ch if ch.is_whitespace() && !in_single_quotes && !in_double_quotes => {
                if token_started {
                    parts.push(std::mem::take(&mut current));
                    token_started = false;
                }
            }
            _ => {
                current.push(ch);
                token_started = true;
            }
        }
    }

    if escaping || in_single_quotes || in_double_quotes {
        anyhow::bail!("Failed to parse $EDITOR: {editor}");
    }

    if token_started {
        parts.push(current);
    }

    let (program, args) = parts
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("$EDITOR is set but empty"))?;
    Ok((program.clone(), args.to_vec()))
}

pub(crate) fn handle_init(non_interactive: bool, full: bool, template: bool) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(None)?;

    if template {
        return handle_init_template(&project_root);
    }

    // Default (no flags) = minimal; --full = old default with tool detection.
    let minimal = !full;
    let config = init_project(&project_root, non_interactive, minimal)?;
    eprintln!(
        "Initialized project configuration at: {}",
        ProjectConfig::config_path(&project_root).display()
    );
    eprintln!("Project: {}", config.project.name);
    if minimal {
        eprintln!("  Mode: minimal (tools/tiers inherit from global config)");
        eprintln!("  Use 'csa init --full' to auto-detect tools and generate tiers.");
    }

    // Generate global config if it doesn't exist
    if let Ok(global_path) = GlobalConfig::config_path()
        && !global_path.exists()
    {
        match GlobalConfig::save_default_template() {
            Ok(path) => {
                eprintln!("Generated global config template at: {}", path.display());
                eprintln!("  Edit to configure API keys and concurrency limits.");
            }
            Err(e) => {
                warn!("Failed to generate global config: {}", e);
            }
        }
    }

    Ok(())
}

/// Generate a fully-commented TOML template at `.csa/config.toml`.
///
/// All sections are present but commented out, so the config file exists
/// (preventing accidental `csa init` re-runs) while every setting falls
/// through to the global config or built-in defaults.
fn handle_init_template(project_root: &std::path::Path) -> Result<()> {
    let config_path = ProjectConfig::config_path(project_root);
    if config_path.exists() {
        anyhow::bail!("Configuration already exists at {}", config_path.display());
    }

    let project_name = project_root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unnamed".to_string());
    // Escape the project name for safe TOML embedding (handles quotes, backslashes).
    let escaped_name = project_name.replace('\\', "\\\\").replace('"', "\\\"");

    let template = format!(
        r##"# CSA Project Configuration — generated by `csa init --template`
# Uncomment and edit sections as needed.  Commented-out values fall through
# to the global config (~/.config/cli-sub-agent/config.toml) or built-in defaults.

schema_version = 1

[project]
name = "{escaped_name}"
created_at = "{now}"
max_recursion_depth = 5

# ─── Resources ──────────────────────────────────────────────────
# [resources]
# min_free_memory_mb = 4096
# idle_timeout_seconds = 120
# liveness_dead_seconds = 600
#
# [resources.initial_estimates]
# gemini-cli = 150
# opencode = 500
# codex = 800
# claude-code = 1200

# ─── Resource Sandbox ─────────────────────────────────────────────
# [resources]
# enforcement_mode = "best-effort"   # "required" | "best-effort" | "off"
# memory_max_mb = 8192               # Max RSS per tool process (>= 256)
# memory_swap_max_mb = 4096          # Max swap per tool process
# pids_max = 512                     # Max PIDs per tool process tree (>= 10)

# ─── Tool Configuration ────────────────────────────────────────
# setting_sources: controls which MCP settings to load for ACP-backed tools.
#   [] = load nothing (lean mode), ["project"] = project only, omit = load all.
#
# [tools.codex]
# enabled = true
# suppress_notify = true
#
# [tools.claude-code]
# enabled = true
# suppress_notify = true
# setting_sources = ["project"]    # load only project-level settings
#
# [tools.gemini-cli]
# enabled = true
# suppress_notify = true
# [tools.gemini-cli.restrictions]
# allow_edit_existing_files = false
#
# [tools.opencode]
# enabled = true
# suppress_notify = true

# ─── Model Tiers ───────────────────────────────────────────────
# Format: "tool/provider/model/thinking_budget"
#
# [tiers.tier-1-quick]
# description = "Quick tasks — fast, cheap"
# models = ["codex/openai/gpt-5.3-codex-spark/xhigh"]
#
# [tiers.tier-2-standard]
# description = "Standard tasks"
# models = ["codex/openai/gpt-5.3-codex/high"]
#
# [tiers.tier-3-complex]
# description = "Complex reasoning, architecture, deep analysis"
# models = ["claude-code/anthropic/default/xhigh"]

# ─── Task-to-Tier Mapping ──────────────────────────────────────
# [tier_mapping]
# default = "tier-2-standard"
# architecture_design = "tier-3-complex"
# code_review = "tier-3-complex"
# feature_implementation = "tier-2-standard"
# documentation = "tier-1-quick"
# quick_question = "tier-1-quick"
# security_audit = "tier-3-complex"
# bug_fix = "tier-2-standard"

# ─── PR Review ──────────────────────────────────────────────────
# [pr_review]
# cloud_bot = true                           # false to skip cloud bot review entirely
# cloud_bot_name = "gemini-code-assist"      # Bot name (for @mention and display)
# cloud_bot_trigger = "auto"                 # "auto" (bot auto-reviews) | "comment" (@bot review)
# cloud_bot_login = ""                       # Override bot GitHub login (default: "<name>[bot]")
# cloud_bot_retrigger_command = ""           # Command to re-trigger review after force-push
#                                            # Default: "/gemini review" for gemini-code-assist,
#                                            #          "@<name> review" for others
# cloud_bot_wait_seconds = 60                # Quiet wait before polling (default: kv_cache.frequent_poll_seconds = 60)
# cloud_bot_poll_max_seconds = 240           # Max poll duration after quiet wait (default: kv_cache.long_poll_seconds = 240)
# merge_strategy = "merge"                   # "merge" | "rebase" (squash is forbidden, default: "merge")
# delete_branch = false                      # Delete remote branch after merge (default: false)

# ─── Aliases ────────────────────────────────────────────────────
# [aliases]
# fast = "codex/openai/gpt-5-codex-mini/low"
# heavy = "claude-code/anthropic/default/high"
"##,
        escaped_name = escaped_name,
        now = chrono::Utc::now().to_rfc3339(),
    );

    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&config_path, &template)?;

    // Update .gitignore
    csa_config::init::update_gitignore(project_root)?;

    eprintln!("Generated config template at: {}", config_path.display());
    eprintln!("  All sections are commented out — uncomment to override global settings.");
    Ok(())
}

/// Get a raw config value by dotted key path.
///
/// Reads raw TOML files (not the merged/defaulted effective config).
/// Fallback order: project `.csa/config.toml` → global config → `--default`.
/// Use `--project` to skip global, `--global` to skip project.
pub(crate) fn handle_config_get(
    key: String,
    default: Option<String>,
    project_only: bool,
    global_only: bool,
    cd: Option<String>,
) -> Result<()> {
    let project_root = (!global_only)
        .then(|| crate::pipeline::determine_project_root(cd.as_deref()))
        .transpose()?;
    let key_is_global_only = is_global_only_key(&key);

    // Try project config first (unless --global flag)
    if !global_only && !key_is_global_only {
        let project_root = project_root
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Failed to determine project root"))?;
        let project_config_path = ProjectConfig::config_path(project_root);
        match load_and_resolve(&project_config_path, &key) {
            Ok(Some(value)) => {
                println!("{}", format_toml_value(&value));
                return Ok(());
            }
            Ok(None) => {} // Key not found, try next source
            Err(e) => anyhow::bail!(
                "Failed to read project config {}: {e}",
                project_config_path.display()
            ),
        }
    }

    if let Some(value) =
        resolve_effective_key(project_root.as_deref(), &key, project_only, global_only)?
    {
        println!("{}", format_toml_value(&value));
        return Ok(());
    }

    // Try global config (unless --project flag)
    if !project_only {
        match GlobalConfig::config_path() {
            Ok(global_path) => {
                match load_and_resolve(&global_path, &key) {
                    Ok(Some(value)) => {
                        println!("{}", format_toml_value(&value));
                        return Ok(());
                    }
                    Ok(None) => {} // Key not found
                    Err(e) => anyhow::bail!(
                        "Failed to read global config {}: {e}",
                        global_path.display()
                    ),
                }
            }
            Err(e) if global_only && default.is_none() => {
                anyhow::bail!("Cannot determine global config path: {e}");
            }
            Err(_) => {} // Non-critical when falling through to default
        }
    }

    // Fall back to --default or report key not found
    match default {
        Some(d) => {
            println!("{d}");
            Ok(())
        }
        None => anyhow::bail!("Key not found: {key}"),
    }
}

fn is_global_only_key(key: &str) -> bool {
    key.starts_with("kv_cache.")
}

/// Load a TOML file and resolve a dotted key path.
///
/// Returns `Ok(None)` if the file doesn't exist or the key path is absent.
/// Returns `Err` if the file exists but cannot be read or parsed.
fn load_and_resolve(path: &std::path::Path, key: &str) -> Result<Option<toml::Value>> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => anyhow::bail!("{e}"),
    };
    // Use `toml::from_str` instead of `str::parse()` — the `FromStr for
    // toml::Value` impl in toml 1.0 has a parser bug that rejects valid
    // TOML files, while the serde `Deserialize` path works correctly.
    let root: toml::Value =
        toml::from_str(&content).map_err(|e| anyhow::anyhow!("TOML parse error: {e}"))?;
    Ok(resolve_key(&root, key))
}

fn build_execution_toml(execution: &csa_config::ExecutionConfig) -> toml::Value {
    let mut table = toml::map::Map::new();
    table.insert(
        "min_timeout_seconds".to_string(),
        toml::Value::Integer(execution.min_timeout_seconds as i64),
    );
    table.insert(
        "auto_weave_upgrade".to_string(),
        toml::Value::Boolean(execution.auto_weave_upgrade),
    );
    toml::Value::Table(table)
}

fn build_project_display_toml(config: &ProjectConfig) -> Result<toml::Value> {
    let mut root = toml::Value::try_from(config.clone())?;
    root.as_table_mut()
        .expect("serialized project config should be a TOML table")
        .insert(
            "execution".to_string(),
            build_execution_toml(&config.execution),
        );
    Ok(root)
}

fn build_project_display_json(config: &ProjectConfig) -> Result<serde_json::Value> {
    let mut root = serde_json::to_value(config)?;
    root.as_object_mut()
        .expect("serialized project config should be a JSON object")
        .insert(
            "execution".to_string(),
            serde_json::json!({
                "min_timeout_seconds": config.execution.min_timeout_seconds,
                "auto_weave_upgrade": config.execution.auto_weave_upgrade,
            }),
        );
    Ok(root)
}

fn build_global_display_toml(config: &GlobalConfig) -> Result<toml::Value> {
    let mut root = toml::Value::try_from(config.clone())?;
    root.as_table_mut()
        .expect("serialized global config should be a TOML table")
        .insert(
            "execution".to_string(),
            build_execution_toml(&config.execution),
        );
    Ok(root)
}

fn resolve_effective_key(
    project_root: Option<&std::path::Path>,
    key: &str,
    project_only: bool,
    global_only: bool,
) -> Result<Option<toml::Value>> {
    if key.starts_with("kv_cache.") {
        if project_only {
            return Ok(None);
        }
        return resolve_effective_global_key(key);
    }

    if !key.starts_with("execution.") {
        return Ok(None);
    }

    if !global_only
        && let Some(project_root) = project_root
        && let Some(value) = if project_only {
            resolve_project_execution_key(project_root, key)?
        } else {
            resolve_effective_execution_key(project_root, key)?
        }
    {
        return Ok(Some(value));
    }

    if project_only {
        return Ok(None);
    }

    resolve_effective_global_key(key)
}

fn resolve_effective_global_key(key: &str) -> Result<Option<toml::Value>> {
    if !(key.starts_with("execution.") || key.starts_with("kv_cache.")) {
        return Ok(None);
    }

    let config = GlobalConfig::load()?;
    let root = build_global_display_toml(&config.redacted_for_display())?;
    Ok(resolve_key(&root, key))
}

fn resolve_effective_execution_key(
    project_root: &std::path::Path,
    key: &str,
) -> Result<Option<toml::Value>> {
    if !key.starts_with("execution.") {
        return Ok(None);
    }

    if let Some(config) = ProjectConfig::load(project_root)? {
        let root = build_project_display_toml(&config.redacted_for_display())?;
        return Ok(resolve_key(&root, key));
    }

    let root = build_global_display_toml(&GlobalConfig::default())?;
    Ok(resolve_key(&root, key))
}

fn resolve_project_execution_key(
    project_root: &std::path::Path,
    key: &str,
) -> Result<Option<toml::Value>> {
    if !key.starts_with("execution.") {
        return Ok(None);
    }

    if let Some(config) = ProjectConfig::load_project_only(project_root)? {
        let root = build_project_display_toml(&config.redacted_for_display())?;
        return Ok(resolve_key(&root, key));
    }

    let root = build_global_display_toml(&GlobalConfig::default())?;
    Ok(resolve_key(&root, key))
}

/// Navigate a TOML value by dotted key path (e.g., "tools.codex.enabled").
fn resolve_key(root: &toml::Value, key: &str) -> Option<toml::Value> {
    let mut current = root;
    for part in key.split('.') {
        current = current.as_table()?.get(part)?;
    }
    Some(current.clone())
}

/// Format a TOML value for stdout (inline for scalars, pretty for tables/arrays).
fn format_toml_value(value: &toml::Value) -> String {
    match value {
        toml::Value::String(s) => s.clone(),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::Table(_) | toml::Value::Array(_) => {
            toml::to_string_pretty(value).unwrap_or_else(|_| format!("{value:?}"))
        }
        toml::Value::Datetime(d) => d.to_string(),
    }
}

pub(crate) fn handle_config_validate(cd: Option<String>) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let config = ProjectConfig::load(&project_root)?
        .ok_or_else(|| anyhow::anyhow!("No configuration found. Run 'csa init' first."))?;

    // Check schema version compatibility
    config.check_schema_version()?;

    // Run full validation
    validate_config(&project_root)?;

    eprintln!("Configuration is valid (schema v{})", config.schema_version);
    Ok(())
}

#[cfg(test)]
#[path = "config_cmds_tests.rs"]
mod tests;
