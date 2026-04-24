use std::collections::BTreeSet;

use anyhow::Result;
use tracing::{error, warn};

use csa_config::config::CURRENT_SCHEMA_VERSION;
use csa_config::init::init_project;
use csa_config::{GlobalConfig, ProjectConfig, validate_config};
use csa_core::types::OutputFormat;

#[path = "config_cmds_helpers.rs"]
mod helpers;
use helpers::{format_missing_key_message, format_toml_value, resolve_key, suggest_key_paths};
#[path = "config_cmds_display.rs"]
mod display;
use display::{inject_resolved_tool_transports_json, inject_resolved_tool_transports_toml};
#[path = "config_cmds_set.rs"]
mod set;
pub(crate) use set::handle_config_set;

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
# cloud_bot_poll_interval_seconds = 30       # Shell helper poll interval during wait script (default: 30)
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

/// Get a config value by dotted key path.
///
/// Lookup order prefers the same effective TOML tree that powers `config show`,
/// then falls back to raw project/global TOML so unknown-but-present sections
/// (for example in forward-compatible configs) remain queryable.
pub(crate) fn handle_config_get(
    key: String,
    default: Option<String>,
    project_only: bool,
    global_only: bool,
    cd: Option<String>,
) -> Result<()> {
    if project_only && is_global_only_key(&key) {
        return match default {
            Some(d) => {
                println!("{d}");
                Ok(())
            }
            None => anyhow::bail!("Key not found: {key}"),
        };
    }

    let global_only_lookup = global_only || is_global_only_key(&key);
    let project_root = (!global_only_lookup)
        .then(|| crate::pipeline::determine_project_root(cd.as_deref()))
        .transpose()?;
    let lookup = build_config_get_lookup(
        project_root.as_deref(),
        &key,
        project_only,
        global_only_lookup,
    )?;

    let resolved = resolve_lookup_sources_with_diagnostics(&lookup.sources, &key)?;
    if let Some(value) = resolved.value {
        if resolved.diagnostics.should_warn_raw_global_parse_fallback() {
            eprintln!("warning: global config has parse errors; showing raw value");
        }
        println!("{}", format_toml_value(&value));
        return Ok(());
    }

    // Fall back to --default or report key not found
    match default {
        Some(d) => {
            println!("{d}");
            Ok(())
        }
        None => anyhow::bail!(
            "{}",
            format_missing_key_message(
                &key,
                &suggest_key_paths(&key, &collect_lookup_keys(&lookup.sources)?)
            )
        ),
    }
}

struct ConfigGetLookup {
    sources: Vec<LookupSourceSpec>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct LookupDiagnostics {
    raw_global_value_from_invalid_effective_global: bool,
    raw_global_value_from_invalid_effective_project: bool,
}

impl LookupDiagnostics {
    fn should_warn_raw_global_parse_fallback(self) -> bool {
        self.raw_global_value_from_invalid_effective_global
            || self.raw_global_value_from_invalid_effective_project
    }
}

#[derive(Debug)]
struct LookupResolution {
    value: Option<toml::Value>,
    diagnostics: LookupDiagnostics,
}

fn is_global_only_key(key: &str) -> bool {
    key.starts_with("kv_cache.") || key.starts_with("state_dir.")
}

#[derive(Debug, Clone)]
enum LookupSourceSpec {
    EffectiveProject {
        project_root: std::path::PathBuf,
        include_global_fallback: bool,
    },
    RawProject {
        path: std::path::PathBuf,
    },
    EffectiveGlobal {
        allow_raw_fallback: bool,
    },
    RawGlobal {
        path: std::path::PathBuf,
    },
}

impl LookupSourceSpec {
    fn allows_deferred_error(&self) -> bool {
        matches!(
            self,
            LookupSourceSpec::EffectiveProject {
                include_global_fallback: true,
                ..
            } | LookupSourceSpec::EffectiveGlobal {
                allow_raw_fallback: true
            }
        )
    }
}

fn build_config_get_lookup(
    project_root: Option<&std::path::Path>,
    key: &str,
    project_only: bool,
    global_only: bool,
) -> Result<ConfigGetLookup> {
    let mut sources = Vec::new();
    let prefer_effective = prefers_effective_lookup(key)?;
    let use_raw_global_only = global_key_prefers_raw_lookup(key);

    if !global_only {
        let project_root =
            project_root.ok_or_else(|| anyhow::anyhow!("Failed to determine project root"))?;

        let effective_project = LookupSourceSpec::EffectiveProject {
            project_root: project_root.to_path_buf(),
            include_global_fallback: !project_only,
        };
        let raw_project = LookupSourceSpec::RawProject {
            path: ProjectConfig::config_path(project_root),
        };

        if prefer_effective {
            sources.push(effective_project);
            sources.push(raw_project);
        } else {
            sources.push(raw_project);
            sources.push(effective_project);
        }
    }

    if !project_only {
        let raw_global = GlobalConfig::config_path()
            .ok()
            .map(|path| LookupSourceSpec::RawGlobal { path });

        if use_raw_global_only {
            if let Some(raw_global) = raw_global {
                sources.push(raw_global);
            }
        } else if prefer_effective {
            sources.push(LookupSourceSpec::EffectiveGlobal {
                allow_raw_fallback: raw_global.is_some(),
            });
            if let Some(raw_global) = raw_global {
                sources.push(raw_global);
            }
        } else {
            if let Some(raw_global) = raw_global {
                sources.push(raw_global);
            }
            sources.push(LookupSourceSpec::EffectiveGlobal {
                allow_raw_fallback: false,
            });
        }
    }

    Ok(ConfigGetLookup { sources })
}

fn load_lookup_root(source: &LookupSourceSpec) -> Result<Option<toml::Value>> {
    match source {
        LookupSourceSpec::EffectiveProject {
            project_root,
            include_global_fallback,
        } => {
            let config = if *include_global_fallback {
                ProjectConfig::load(project_root)?
            } else {
                ProjectConfig::load_project_only(project_root)?
            };
            config
                .map(|cfg| build_project_display_toml(&cfg.redacted_for_display()))
                .transpose()
        }
        LookupSourceSpec::RawProject { path } | LookupSourceSpec::RawGlobal { path } => {
            load_toml_root(path)
        }
        LookupSourceSpec::EffectiveGlobal { .. } => Ok(Some(build_global_display_toml(
            &GlobalConfig::load()?.redacted_for_display(),
        )?)),
    }
}

#[cfg(test)]
fn resolve_effective_key(
    project_root: Option<&std::path::Path>,
    key: &str,
    project_only: bool,
    global_only: bool,
) -> Result<Option<toml::Value>> {
    if project_only && is_global_only_key(key) {
        return Ok(None);
    }

    let lookup = build_config_get_lookup(
        project_root,
        key,
        project_only,
        global_only || is_global_only_key(key),
    )?;
    resolve_lookup_sources(&lookup.sources, key)
}

#[cfg(test)]
fn resolve_lookup_sources(sources: &[LookupSourceSpec], key: &str) -> Result<Option<toml::Value>> {
    Ok(resolve_lookup_sources_with_diagnostics(sources, key)?.value)
}

fn resolve_lookup_sources_with_diagnostics(
    sources: &[LookupSourceSpec],
    key: &str,
) -> Result<LookupResolution> {
    let mut deferred_error = None;
    let mut saw_deferred_effective_global_error = false;
    let mut saw_deferred_effective_project_error = false;

    for source in sources {
        match load_lookup_root(source) {
            Ok(Some(root)) => {
                if let Some(value) = resolve_key(&root, key) {
                    let raw_global_source = matches!(source, LookupSourceSpec::RawGlobal { .. });
                    return Ok(LookupResolution {
                        value: Some(value),
                        diagnostics: LookupDiagnostics {
                            raw_global_value_from_invalid_effective_global: raw_global_source
                                && saw_deferred_effective_global_error,
                            raw_global_value_from_invalid_effective_project: raw_global_source
                                && saw_deferred_effective_project_error,
                        },
                    });
                }
            }
            Ok(None) => {}
            Err(err) if source.allows_deferred_error() => {
                // Effective project lookups can fail because global config is broken.
                // Keep going so an explicit raw project value can still be resolved.
                if matches!(
                    source,
                    LookupSourceSpec::EffectiveGlobal {
                        allow_raw_fallback: true
                    }
                ) {
                    saw_deferred_effective_global_error = true;
                }
                if matches!(
                    source,
                    LookupSourceSpec::EffectiveProject {
                        include_global_fallback: true,
                        ..
                    }
                ) {
                    saw_deferred_effective_project_error = true;
                }
                deferred_error.get_or_insert(err);
            }
            Err(err) => return Err(err),
        }
    }

    if let Some(err) = deferred_error {
        return Err(err);
    }

    Ok(LookupResolution {
        value: None,
        diagnostics: LookupDiagnostics::default(),
    })
}

fn collect_lookup_keys(sources: &[LookupSourceSpec]) -> Result<BTreeSet<String>> {
    let mut keys = BTreeSet::new();
    let mut deferred_error = None;

    for source in sources {
        match load_lookup_root(source) {
            Ok(Some(root)) => collect_key_paths(&root, None, &mut keys),
            Ok(None) => {}
            Err(err) if source.allows_deferred_error() => {
                deferred_error.get_or_insert(err);
            }
            Err(err) => return Err(err),
        }
    }

    if keys.is_empty()
        && let Some(err) = deferred_error
    {
        return Err(err);
    }

    Ok(keys)
}

fn prefers_effective_lookup(key: &str) -> Result<bool> {
    let Some(top_level) = key.split('.').next() else {
        return Ok(false);
    };
    Ok(known_effective_top_level_sections()?.contains(top_level))
}

fn global_key_prefers_raw_lookup(key: &str) -> bool {
    key.starts_with("kv_cache.") || key.starts_with("state_dir.")
}

fn known_effective_top_level_sections() -> Result<BTreeSet<String>> {
    let mut sections = BTreeSet::new();
    collect_top_level_sections(&default_project_display_toml()?, &mut sections);
    collect_top_level_sections(&default_global_display_toml()?, &mut sections);
    Ok(sections)
}

fn collect_top_level_sections(root: &toml::Value, out: &mut BTreeSet<String>) {
    if let Some(table) = root.as_table() {
        out.extend(table.keys().cloned());
    }
}

fn default_project_display_toml() -> Result<toml::Value> {
    let config: ProjectConfig =
        toml::from_str(&format!("schema_version = {CURRENT_SCHEMA_VERSION}\n"))?;
    build_project_display_toml(&config)
}

fn default_global_display_toml() -> Result<toml::Value> {
    build_global_display_toml(&GlobalConfig::default())
}

fn collect_key_paths(root: &toml::Value, prefix: Option<&str>, out: &mut BTreeSet<String>) {
    if let Some(prefix) = prefix {
        out.insert(prefix.to_string());
    }

    if let Some(table) = root.as_table() {
        for (key, value) in table {
            let next = match prefix {
                Some(prefix) => format!("{prefix}.{key}"),
                None => key.clone(),
            };
            collect_key_paths(value, Some(&next), out);
        }
    }
}

/// Load a TOML file into a root value.
///
/// Returns `Ok(None)` if the file doesn't exist.
/// Returns `Err` if the file exists but cannot be read or parsed.
fn load_toml_root(path: &std::path::Path) -> Result<Option<toml::Value>> {
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
    Ok(Some(root))
}

#[cfg(test)]
/// Load a TOML file and resolve a dotted key path.
///
/// Returns `Ok(None)` if the file doesn't exist or the key path is absent.
/// Returns `Err` if the file exists but cannot be read or parsed.
fn load_and_resolve(path: &std::path::Path, key: &str) -> Result<Option<toml::Value>> {
    Ok(load_toml_root(path)?.and_then(|root| resolve_key(&root, key)))
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
    let root_table = root
        .as_table_mut()
        .expect("serialized project config should be a TOML table");
    root_table.insert(
        "execution".to_string(),
        build_execution_toml(&config.execution),
    );
    inject_resolved_tool_transports_toml(root_table, config);
    Ok(root)
}

fn build_project_display_json(config: &ProjectConfig) -> Result<serde_json::Value> {
    let mut root = serde_json::to_value(config)?;
    let root_object = root
        .as_object_mut()
        .expect("serialized project config should be a JSON object");
    root_object.insert(
        "execution".to_string(),
        serde_json::json!({
            "min_timeout_seconds": config.execution.min_timeout_seconds,
            "auto_weave_upgrade": config.execution.auto_weave_upgrade,
        }),
    );
    inject_resolved_tool_transports_json(root_object, config);
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

#[cfg(test)]
fn resolve_effective_global_key(key: &str) -> Result<Option<toml::Value>> {
    if !(key.starts_with("execution.") || key.starts_with("kv_cache.")) {
        return Ok(None);
    }

    let config = GlobalConfig::load()?;
    let root = build_global_display_toml(&config.redacted_for_display())?;
    Ok(resolve_key(&root, key))
}

#[cfg(test)]
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

#[cfg(test)]
#[path = "config_cmds_transport_display_tests.rs"]
mod transport_display_tests;

#[cfg(test)]
#[path = "config_cmds_lookup_tests.rs"]
mod lookup_tests;
