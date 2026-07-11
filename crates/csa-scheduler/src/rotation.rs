//! Round-robin tool rotation within tiers.
//!
//! State is persisted in `{project_state}/rotation.toml` and protected by
//! a blocking `flock` (rotation decisions are fast, so blocking is fine).

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use csa_config::{EffectiveModelCatalog, ProjectConfig, TierStrategy};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;
use std::path::Path;
use tracing::debug;

/// Per-tier rotation state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierRotation {
    /// Index of the last tool used (0-based into the tier's models list).
    pub last_index: u32,
    /// When the last rotation happened.
    pub last_used_at: DateTime<Utc>,
}

/// Top-level rotation state file.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RotationState {
    #[serde(default)]
    pub tiers: HashMap<String, TierRotation>,
}

/// Select the next tool from a tier using round-robin.
///
/// Returns `(tool_name, model_spec_string)` or `None` if all tools in the
/// tier are disabled or filtered out.
///
/// `task_type` is used to look up the tier via `tier_mapping` (falls back
/// to tier3).  `needs_edit` filters out tools that are not fully write-capable
/// (either `allow_edit_existing_files = false` or `allow_write_new_files = false`).
/// Returns `Err` when `needs_edit` is true and all enabled tools in the tier have
/// write restrictions — the caller should surface this as a hard error.
#[cfg(test)]
fn resolve_tier_tool_rotated(
    config: &ProjectConfig,
    task_type: &str,
    project_root: &Path,
    needs_edit: bool,
) -> Result<Option<(String, String)>> {
    let catalog = EffectiveModelCatalog::shipped()?;
    resolve_tier_tool_rotated_with_catalog(config, &catalog, task_type, project_root, needs_edit)
}

pub fn resolve_tier_tool_rotated_with_catalog(
    config: &ProjectConfig,
    catalog: &EffectiveModelCatalog,
    task_type: &str,
    project_root: &Path,
    needs_edit: bool,
) -> Result<Option<(String, String)>> {
    // 1. Resolve tier name from task_type
    let tier_name = match resolve_tier_name(config, task_type) {
        Some(name) => name,
        None => return Ok(None),
    };

    // 2. Get tier's model list
    let tier = match config.tiers.get(&tier_name) {
        Some(t) => t,
        None => return Ok(None),
    };

    if tier.models.is_empty() {
        return Ok(None);
    }

    // 3. Build list of eligible (index, tool_name, model_spec) entries
    let mut eligible: Vec<(usize, String, String)> = Vec::new();
    for (index, spec) in tier.models.iter().enumerate() {
        let parts: Vec<&str> = spec.split('/').collect();
        if parts.len() != 4 {
            continue;
        }
        match catalog.validate_parts(parts[0], parts[1], parts[2], parts[3]) {
            Ok(_) => {}
            Err(error)
                if error.kind() == csa_core::model_catalog::CatalogErrorKind::DisabledModel =>
            {
                bail!("tier model '{spec}' is tombstoned and cannot be skipped: {error}");
            }
            Err(_) => continue,
        }
        let tool_name = parts[0];
        // Skip disabled tools
        if !config.is_tool_enabled(tool_name) {
            continue;
        }
        // Skip tools that are not fully write-capable when writing is needed.
        // A tool must have both allow_edit_existing_files and allow_write_new_files
        // set to true (or absent) to qualify for csa run races.
        if needs_edit && !config.is_tool_write_capable(tool_name) {
            continue;
        }
        eligible.push((index, tool_name.to_string(), spec.clone()));
    }

    if eligible.is_empty() {
        // When needs_edit is true and there are enabled tools that were filtered
        // out due to write restrictions, return a clear error instead of silently
        // falling through — the caller cannot fix this by trying a fallback.
        if needs_edit {
            let has_enabled_write_restricted = tier.models.iter().any(|spec| {
                let t = spec.split('/').next().unwrap_or("");
                config.is_tool_enabled(t) && !config.is_tool_write_capable(t)
            });
            if has_enabled_write_restricted {
                return Err(anyhow::anyhow!(
                    "No writable tool available in tier '{}': all enabled tools have write \
                     restrictions (allow_edit_existing_files = false or \
                     allow_write_new_files = false). \
                     Check [tools.<name>.restrictions] in your config, or use \
                     --force to bypass tier routing.",
                    tier_name
                ));
            }
        }
        return Ok(None);
    }

    let strategy = tier.strategy;

    // 4. Atomic flock + read/write rotation state
    let state_dir = csa_session::get_session_root(project_root)?;
    let rotation_path = state_dir.join("rotation.toml");

    let result = with_rotation_lock(&rotation_path, |state| {
        let total = tier.models.len();

        // Priority: always start from 0 (first eligible wins).
        // RoundRobin: advance from last used position.
        let start = match strategy {
            TierStrategy::Priority => 0,
            TierStrategy::RoundRobin => {
                let last_index = state
                    .tiers
                    .get(&tier_name)
                    .map(|t| t.last_index as usize)
                    .unwrap_or(0);
                (last_index + 1) % total
            }
        };

        let mut chosen = None;
        for offset in 0..total {
            let candidate_idx = (start + offset) % total;
            if let Some((_, tool, spec)) = eligible.iter().find(|(i, _, _)| *i == candidate_idx) {
                chosen = Some((candidate_idx, tool.clone(), spec.clone()));
                break;
            }
        }

        match chosen {
            Some((idx, tool, spec)) => {
                state.tiers.insert(
                    tier_name.clone(),
                    TierRotation {
                        last_index: idx as u32,
                        last_used_at: Utc::now(),
                    },
                );
                debug!(
                    tier = %tier_name,
                    tool = %tool,
                    index = idx,
                    ?strategy,
                    "Selected tool from tier"
                );
                Ok(Some((tool, spec)))
            }
            None => Ok(None),
        }
    })?;

    Ok(result)
}

/// Returns true when the error was produced because all enabled tier tools have write restrictions.
///
/// Callers that need to distinguish this hard error from "no tier configured" (which is
/// `Ok(None)`) should use this predicate rather than matching on error message strings.
pub fn is_no_writable_tier_tool_error(e: &anyhow::Error) -> bool {
    e.to_string().contains("No writable tool available in tier")
}

/// Resolve tier name from task_type via config tier_mapping, with fallback.
fn resolve_tier_name(config: &ProjectConfig, task_type: &str) -> Option<String> {
    config.tier_mapping.get(task_type).cloned().or_else(|| {
        if config.tiers.contains_key("tier3") {
            Some("tier3".to_string())
        } else {
            config
                .tiers
                .keys()
                .find(|k| k.starts_with("tier-3-") || k.starts_with("tier3"))
                .cloned()
        }
    })
}

/// Execute `f` while holding a blocking exclusive flock on `rotation_path`.
///
/// Reads the existing state (or default), passes it mutably to `f`, and
/// writes the result back if `f` returned Ok.
fn with_rotation_lock<F, T>(rotation_path: &Path, f: F) -> Result<T>
where
    F: FnOnce(&mut RotationState) -> Result<T>,
{
    // Ensure parent directory exists
    if let Some(parent) = rotation_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(rotation_path)
        .with_context(|| format!("Failed to open rotation file: {}", rotation_path.display()))?;

    // Blocking exclusive lock (rotation ops are fast)
    acquire_blocking_flock(&file)?;

    // Read existing state
    let mut state = read_rotation_state(&file)?;

    // Run the callback
    let result = f(&mut state)?;

    // Write back
    write_rotation_state(&file, &state)?;

    // Unlock (also released on drop/close, but be explicit)
    release_flock(&file);

    Ok(result)
}

fn acquire_blocking_flock(file: &File) -> Result<()> {
    let fd = file.as_raw_fd();
    // SAFETY: fd is a valid file descriptor from an open File.
    // LOCK_EX requests an exclusive blocking lock.
    let ret = unsafe { libc::flock(fd, libc::LOCK_EX) };
    if ret != 0 {
        anyhow::bail!(
            "Failed to acquire rotation lock: {}",
            std::io::Error::last_os_error()
        );
    }
    Ok(())
}

fn release_flock(file: &File) {
    let fd = file.as_raw_fd();
    // SAFETY: fd is valid; LOCK_UN releases the advisory lock.
    unsafe {
        libc::flock(fd, libc::LOCK_UN);
    }
}

fn read_rotation_state(file: &File) -> Result<RotationState> {
    let mut contents = String::new();
    // Use a reference to avoid consuming the File
    let mut reader = std::io::BufReader::new(file);
    reader.read_to_string(&mut contents)?;
    if contents.trim().is_empty() {
        return Ok(RotationState::default());
    }
    toml::from_str(&contents).context("Failed to parse rotation.toml")
}

fn write_rotation_state(file: &File, state: &RotationState) -> Result<()> {
    use std::io::Seek;
    let content = toml::to_string_pretty(state)?;
    let mut writer = std::io::BufWriter::new(file);
    // Truncate and rewrite
    writer
        .get_ref()
        .set_len(0)
        .context("Failed to truncate rotation file")?;
    writer.seek(std::io::SeekFrom::Start(0))?;
    writer
        .write_all(content.as_bytes())
        .context("Failed to write rotation state")?;
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use csa_config::{ProjectConfig, ProjectMeta, TierConfig, TierStrategy, ToolConfig};
    use tempfile::tempdir;

    fn make_config(models: Vec<&str>, disabled_tools: Vec<&str>) -> ProjectConfig {
        make_config_with_strategy(models, disabled_tools, TierStrategy::default())
    }

    fn make_config_with_strategy(
        models: Vec<&str>,
        disabled_tools: Vec<&str>,
        strategy: TierStrategy,
    ) -> ProjectConfig {
        let mut tools = HashMap::new();
        for tool in disabled_tools {
            tools.insert(
                tool.to_string(),
                ToolConfig {
                    enabled: false,
                    restrictions: None,
                    suppress_notify: true,
                    ..Default::default()
                },
            );
        }

        let mut tiers = HashMap::new();
        tiers.insert(
            "tier3".to_string(),
            TierConfig {
                description: "test tier".to_string(),
                models: models.iter().map(|s| s.to_string()).collect(),
                strategy,
                token_budget: None,
                max_turns: None,
            },
        );

        let mut tier_mapping = HashMap::new();
        tier_mapping.insert("default".to_string(), "tier3".to_string());

        ProjectConfig {
            schema_version: 1,
            project: ProjectMeta {
                name: "test".to_string(),
                created_at: Utc::now(),
                max_recursion_depth: 5,
            },
            resources: Default::default(),
            acp: Default::default(),
            tools,
            review: None,
            debate: None,
            tiers,
            tier_mapping,
            aliases: HashMap::new(),
            tool_aliases: HashMap::new(),
            tool_state_dirs: HashMap::new(),
            preferences: None,
            github: None,
            session: Default::default(),
            memory: Default::default(),
            hooks: Default::default(),
            run: Default::default(),
            execution: Default::default(),
            session_wait: None,
            preflight: Default::default(),
            vcs: Default::default(),
            filesystem_sandbox: Default::default(),
        }
    }

    #[test]
    fn test_rotation_round_robin() {
        let temp = tempdir().unwrap();
        let rotation_path = temp.path().join("rotation.toml");

        let models = [
            "gemini-cli/google/gemini-2.5-pro/0",
            "codex/openai/o4-mini/0",
            "claude-code/anthropic/sonnet/0",
        ];

        // First call: should pick index 1 (starting from default 0, next = 1)
        let result = with_rotation_lock(&rotation_path, |state| {
            let tier = "tier3";
            let last = state
                .tiers
                .get(tier)
                .map(|t| t.last_index as usize)
                .unwrap_or(0);
            let next = (last + 1) % models.len();
            state.tiers.insert(
                tier.to_string(),
                TierRotation {
                    last_index: next as u32,
                    last_used_at: Utc::now(),
                },
            );
            Ok(next)
        })
        .unwrap();
        assert_eq!(result, 1);

        // Second call: should pick index 2
        let result = with_rotation_lock(&rotation_path, |state| {
            let tier = "tier3";
            let last = state
                .tiers
                .get(tier)
                .map(|t| t.last_index as usize)
                .unwrap_or(0);
            let next = (last + 1) % models.len();
            state.tiers.insert(
                tier.to_string(),
                TierRotation {
                    last_index: next as u32,
                    last_used_at: Utc::now(),
                },
            );
            Ok(next)
        })
        .unwrap();
        assert_eq!(result, 2);

        // Third call: should wrap to index 0
        let result = with_rotation_lock(&rotation_path, |state| {
            let tier = "tier3";
            let last = state
                .tiers
                .get(tier)
                .map(|t| t.last_index as usize)
                .unwrap_or(0);
            let next = (last + 1) % models.len();
            state.tiers.insert(
                tier.to_string(),
                TierRotation {
                    last_index: next as u32,
                    last_used_at: Utc::now(),
                },
            );
            Ok(next)
        })
        .unwrap();
        assert_eq!(result, 0);
    }

    #[test]
    fn test_rotation_state_persistence() {
        let temp = tempdir().unwrap();
        let rotation_path = temp.path().join("rotation.toml");

        // Write state
        with_rotation_lock(&rotation_path, |state| {
            state.tiers.insert(
                "tier3".to_string(),
                TierRotation {
                    last_index: 2,
                    last_used_at: Utc::now(),
                },
            );
            Ok(())
        })
        .unwrap();

        // Read back in a new lock scope
        let idx = with_rotation_lock(&rotation_path, |state| {
            Ok(state.tiers.get("tier3").map(|t| t.last_index).unwrap_or(0))
        })
        .unwrap();

        assert_eq!(idx, 2);
    }

    #[test]
    fn test_resolve_tier_name_mapping() {
        let config = make_config(vec!["gemini-cli/g/m/0"], vec![]);
        assert_eq!(
            resolve_tier_name(&config, "default"),
            Some("tier3".to_string())
        );
    }

    #[test]
    fn test_resolve_tier_name_fallback() {
        let config = make_config(vec!["gemini-cli/g/m/0"], vec![]);
        // "review" is not in tier_mapping, fallback to tier3
        assert_eq!(
            resolve_tier_name(&config, "review"),
            Some("tier3".to_string())
        );
    }

    #[test]
    fn test_eligible_skips_disabled() {
        let config = make_config(
            vec!["gemini-cli/g/m/0", "codex/openai/o4-mini/0"],
            vec!["gemini-cli"],
        );

        let tier = config.tiers.get("tier3").unwrap();
        let eligible: Vec<_> = tier
            .models
            .iter()
            .enumerate()
            .filter_map(|(i, spec)| {
                let tool = spec.split('/').next()?;
                if !config.is_tool_enabled(tool) {
                    return None;
                }
                Some((i, tool.to_string(), spec.clone()))
            })
            .collect();

        assert_eq!(eligible.len(), 1);
        assert_eq!(eligible[0].1, "codex");
    }

    #[test]
    fn effective_catalog_filters_rejected_spec_before_rotation() {
        let temp = tempdir().unwrap();
        let config = make_config_with_strategy(
            vec![
                "codex/openai/not-declared/high",
                "codex/openai/config-only-fake/high",
            ],
            vec![],
            TierStrategy::Priority,
        );
        let catalog = EffectiveModelCatalog::from_toml_str(
            r#"
[model_catalog]
mode = "replace"
closed = true

[[model_catalog.entries]]
tool = "codex"
provider = "openai"
model = "config-only-fake"
reasoning_efforts = ["high"]
"#,
            "scheduler-test",
        )
        .unwrap();

        let selected = resolve_tier_tool_rotated_with_catalog(
            &config,
            &catalog,
            "default",
            temp.path(),
            false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(selected.1, "codex/openai/config-only-fake/high");
    }
}

#[cfg(test)]
#[path = "rotation_tests_tail.rs"]
mod rotation_tests_tail;
