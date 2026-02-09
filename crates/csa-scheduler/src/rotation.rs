//! Round-robin tool rotation within tiers.
//!
//! State is persisted in `{project_state}/rotation.toml` and protected by
//! a blocking `flock` (rotation decisions are fast, so blocking is fine).

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use csa_config::ProjectConfig;
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
/// to tier3).  `needs_edit` filters out tools whose `allow_edit_existing_files`
/// restriction is false.
pub fn resolve_tier_tool_rotated(
    config: &ProjectConfig,
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
    let eligible: Vec<(usize, String, String)> = tier
        .models
        .iter()
        .enumerate()
        .filter_map(|(i, spec)| {
            let tool_name = spec.split('/').next()?;
            // Skip disabled tools
            if !config.is_tool_enabled(tool_name) {
                return None;
            }
            // Skip tools that can't edit when editing is needed
            if needs_edit && !config.can_tool_edit_existing(tool_name) {
                return None;
            }
            Some((i, tool_name.to_string(), spec.clone()))
        })
        .collect();

    if eligible.is_empty() {
        return Ok(None);
    }

    // 4. Atomic flock + read/write rotation state
    let state_dir = csa_session::get_session_root(project_root)?;
    let rotation_path = state_dir.join("rotation.toml");

    let result = with_rotation_lock(&rotation_path, |state| {
        let tier_state = state.tiers.get(&tier_name);
        let last_index = tier_state.map(|t| t.last_index as usize).unwrap_or(0);

        // Find next eligible starting from (last_index + 1) % total
        let total = tier.models.len();
        let start = (last_index + 1) % total;

        let mut chosen = None;
        for offset in 0..total {
            let candidate_idx = (start + offset) % total;
            if let Some((_, ref tool, ref spec)) =
                eligible.iter().find(|(i, _, _)| *i == candidate_idx)
            {
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
                    "Round-robin selected tool"
                );
                Ok(Some((tool, spec)))
            }
            None => Ok(None),
        }
    })?;

    Ok(result)
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
    use csa_config::{ProjectConfig, ProjectMeta, TierConfig, ToolConfig};
    use tempfile::tempdir;

    fn make_config(models: Vec<&str>, disabled_tools: Vec<&str>) -> ProjectConfig {
        let mut tools = HashMap::new();
        for tool in disabled_tools {
            tools.insert(
                tool.to_string(),
                ToolConfig {
                    enabled: false,
                    restrictions: None,
                    suppress_notify: false,
                },
            );
        }

        let mut tiers = HashMap::new();
        tiers.insert(
            "tier3".to_string(),
            TierConfig {
                description: "test tier".to_string(),
                models: models.iter().map(|s| s.to_string()).collect(),
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
            tools,
            review: None,
            debate: None,
            tiers,
            tier_mapping,
            aliases: HashMap::new(),
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
}
