//! Antigravity CLI settings.json model override.
//!
//! `agy` (antigravity-cli) does not accept `-m <model>`; instead it reads the
//! active model from `~/.gemini/antigravity-cli/settings.json` (`model` field,
//! DISPLAY format such as `"Gemini 3.1 Pro (High)"`). To honour the CSA
//! `model_override` for an antigravity-cli session, we atomically rewrite the
//! settings.json file before spawning `agy` and restore the original contents
//! when the guard is dropped after the process exits.
//!
//! Because settings.json is a singleton shared across all `agy` invocations on
//! the host, concurrent antigravity-cli sessions would race on this file.
//! Callers MUST serialize antigravity-cli sessions (effectively
//! `max_concurrent = 1` for this tool).
//!
//! Notes:
//! - If `model_override` is `None` or normalises to the `"default"` sentinel,
//!   no settings.json edit is performed and `apply_model` returns `None`.
//! - Atomicity is achieved via `write` + `rename` within the settings
//!   directory, so partial reads by a concurrent `agy` cannot observe a
//!   half-written file.

use anyhow::{Context, Result};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const SETTINGS_DIR: &str = ".gemini/antigravity-cli";
const SETTINGS_FILE: &str = "settings.json";
const MODEL_FIELD: &str = "model";

/// RAII guard that restores `~/.gemini/antigravity-cli/settings.json` to its
/// original state when dropped.
///
/// Created by [`AntigravitySettingsGuard::apply_model`]; the guard's lifetime
/// MUST span the lifetime of the spawned `agy` process so that the model
/// selection it staged is read by `agy` at startup, and so that the original
/// host configuration is restored before any unrelated subsequent `agy` run.
pub(crate) struct AntigravitySettingsGuard {
    settings_path: PathBuf,
    original: Option<String>,
}

impl AntigravitySettingsGuard {
    /// Set the `model` field in `~/.gemini/antigravity-cli/settings.json` to
    /// `model_override` (after the same `"default"`/empty filtering used by
    /// [`crate::executor::arg_helpers::effective_gemini_model_override`]) and
    /// return a guard that will restore the original contents on drop.
    ///
    /// Returns `Ok(None)` when no override should be applied (either the
    /// caller passed `None`/`"default"`, or `$HOME` is not set so we cannot
    /// locate the settings file).
    pub(crate) fn apply_model(model_override: &Option<String>) -> Result<Option<Self>> {
        let Some(model) = effective_override(model_override) else {
            return Ok(None);
        };
        let model = model.as_str();
        let Some(settings_path) = settings_file_path() else {
            tracing::warn!(
                "antigravity-cli model override requested but $HOME is unset; \
                 skipping settings.json rewrite (agy will use whatever model is configured)"
            );
            return Ok(None);
        };
        Self::apply_model_at(&settings_path, model).map(Some)
    }

    fn apply_model_at(settings_path: &Path, model: &str) -> Result<Self> {
        let parent = settings_path
            .parent()
            .with_context(|| format!("settings path has no parent: {}", settings_path.display()))?;
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create antigravity-cli settings directory {}",
                parent.display()
            )
        })?;

        let original = match fs::read_to_string(settings_path) {
            Ok(s) => Some(s),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => {
                return Err(e).with_context(|| {
                    format!(
                        "failed to read antigravity-cli settings.json at {}",
                        settings_path.display()
                    )
                });
            }
        };

        let new_content = rewrite_model(original.as_deref(), model)?;
        write_atomic(settings_path, &new_content)?;
        Ok(Self {
            settings_path: settings_path.to_path_buf(),
            original,
        })
    }
}

impl Drop for AntigravitySettingsGuard {
    fn drop(&mut self) {
        let result = match &self.original {
            Some(content) => write_atomic(&self.settings_path, content),
            None => fs::remove_file(&self.settings_path).or_else(|e| {
                if e.kind() == io::ErrorKind::NotFound {
                    Ok(())
                } else {
                    Err(anyhow::Error::from(e))
                }
            }),
        };
        if let Err(e) = result {
            tracing::warn!(
                path = %self.settings_path.display(),
                error = %e,
                "failed to restore antigravity-cli settings.json"
            );
        }
    }
}

/// Normalize a user-provided model name: if it matches a known slug alias,
/// resolve to the canonical display name. Otherwise pass through unchanged
/// (the model name may be a valid display name we don't know about).
fn normalize_model_alias(input: &str) -> String {
    let trimmed = input.trim();
    let slug = trimmed
        .to_ascii_lowercase()
        .replace(' ', "-")
        .replace(['(', ')'], "");
    let aliases = csa_core::model_catalog::shipped_model_aliases().unwrap_or_default();
    for alias in &aliases {
        if alias.aliases.iter().any(|candidate| candidate == &slug) {
            return alias.canonical.clone();
        }
    }
    // Also check if the input matches a known display name case-insensitively
    for alias in &aliases {
        if alias.canonical.eq_ignore_ascii_case(trimmed) {
            return alias.canonical.clone();
        }
    }
    trimmed.to_string()
}

fn effective_override(model_override: &Option<String>) -> Option<String> {
    model_override
        .as_deref()
        .map(normalize_model_alias)
        .filter(|m| !m.eq_ignore_ascii_case("default"))
        .filter(|m| !m.is_empty())
}

fn settings_file_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(SETTINGS_DIR).join(SETTINGS_FILE))
}

fn rewrite_model(original: Option<&str>, model: &str) -> Result<String> {
    let mut value = match original {
        Some(text) => {
            let parsed: serde_json::Value = serde_json::from_str(text).with_context(|| {
                "antigravity-cli settings.json is not valid JSON; refusing to overwrite \
                 (run `agy` once to regenerate, or fix the file manually)"
            })?;
            if !parsed.is_object() {
                anyhow::bail!(
                    "antigravity-cli settings.json root is not a JSON object; \
                     refusing to overwrite"
                );
            }
            parsed
        }
        None => serde_json::Value::Object(serde_json::Map::new()),
    };
    let object = value
        .as_object_mut()
        .expect("settings.json root must be a JSON object at this point");
    object.insert(
        MODEL_FIELD.to_string(),
        serde_json::Value::String(model.to_string()),
    );
    let mut serialized = serde_json::to_string_pretty(&value)
        .context("failed to serialize antigravity-cli settings.json")?;
    serialized.push('\n');
    Ok(serialized)
}

fn write_atomic(path: &Path, content: &str) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("path has no parent directory: {}", path.display()))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create directory {}", parent.display()))?;

    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("settings.json");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let temp_path = parent.join(format!(
        ".{}.csa-tmp.{}.{}",
        file_name,
        std::process::id(),
        nanos
    ));

    if let Err(e) = fs::write(&temp_path, content) {
        let _ = fs::remove_file(&temp_path);
        return Err(e).with_context(|| {
            format!(
                "failed to write antigravity-cli settings temp file {}",
                temp_path.display()
            )
        });
    }
    if let Err(e) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(e).with_context(|| {
            format!(
                "failed to atomically replace antigravity-cli settings file {}",
                path.display()
            )
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_json_value(path: &Path) -> serde_json::Value {
        let raw = fs::read_to_string(path).expect("settings.json must exist");
        serde_json::from_str(&raw).expect("settings.json must be valid JSON")
    }

    #[test]
    fn apply_model_inserts_into_existing_settings() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        let original =
            "{\n  \"enableTelemetry\": false,\n  \"trustedWorkspaces\": [\"/tmp/x\"]\n}\n";
        fs::write(&path, original).unwrap();

        let guard = AntigravitySettingsGuard::apply_model_at(&path, "Gemini 3.1 Pro (High)")
            .expect("guard must apply");
        let updated = read_json_value(&path);
        assert_eq!(updated["model"], "Gemini 3.1 Pro (High)");
        assert_eq!(updated["enableTelemetry"], false);
        assert_eq!(updated["trustedWorkspaces"][0], "/tmp/x");

        drop(guard);

        let restored = fs::read_to_string(&path).unwrap();
        assert_eq!(
            restored, original,
            "drop must restore original contents byte-for-byte"
        );
    }

    #[test]
    fn apply_model_overrides_existing_model_field_and_restores() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        let original = "{\"model\": \"Gemini 2.5 Pro\"}";
        fs::write(&path, original).unwrap();

        let guard = AntigravitySettingsGuard::apply_model_at(&path, "Gemini 3.1 Pro (High)")
            .expect("guard must apply");
        let updated = read_json_value(&path);
        assert_eq!(updated["model"], "Gemini 3.1 Pro (High)");

        drop(guard);
        let restored = fs::read_to_string(&path).unwrap();
        assert_eq!(restored, original);
    }

    #[test]
    fn apply_model_creates_settings_when_absent_and_removes_on_drop() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("settings.json");
        assert!(!path.exists());

        let guard =
            AntigravitySettingsGuard::apply_model_at(&path, "Gemini 3.1 Pro (High)").unwrap();
        let created = read_json_value(&path);
        assert_eq!(created["model"], "Gemini 3.1 Pro (High)");

        drop(guard);
        assert!(
            !path.exists(),
            "drop must remove a settings file that did not exist before apply"
        );
    }

    #[test]
    fn apply_model_returns_none_for_default_sentinel_and_empty() {
        // We can test the effective_override predicate directly.
        assert!(effective_override(&None).is_none());
        assert!(effective_override(&Some(String::new())).is_none());
        assert!(effective_override(&Some("   ".to_string())).is_none());
        assert!(effective_override(&Some("default".to_string())).is_none());
        assert!(effective_override(&Some("Default".to_string())).is_none());
        assert_eq!(
            effective_override(&Some("Gemini 3.1 Pro (High)".to_string())),
            Some("Gemini 3.1 Pro (High)".to_string())
        );
    }

    #[test]
    fn apply_model_rejects_non_object_settings_json() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        fs::write(&path, "[1, 2, 3]").unwrap();

        let result = AntigravitySettingsGuard::apply_model_at(&path, "Gemini 3.1 Pro (High)");
        assert!(result.is_err(), "non-object JSON root must error out");
        // Original file untouched.
        assert_eq!(fs::read_to_string(&path).unwrap(), "[1, 2, 3]");
    }

    #[test]
    fn apply_model_rejects_invalid_json() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        fs::write(&path, "{not json").unwrap();

        let result = AntigravitySettingsGuard::apply_model_at(&path, "Gemini 3.1 Pro (High)");
        assert!(result.is_err(), "invalid JSON must error out");
        assert_eq!(fs::read_to_string(&path).unwrap(), "{not json");
    }
}

#[test]
fn issue_2347_normalize_model_alias_resolves_slugs() {
    assert_eq!(
        normalize_model_alias("gemini-3.1-pro-high"),
        "Gemini 3.1 Pro (High)"
    );
    assert_eq!(
        normalize_model_alias("gemini-3.5-flash"),
        "Gemini 3.5 Flash (High)"
    );
    assert_eq!(
        normalize_model_alias("opus-thinking"),
        "Claude Opus 4.6 (Thinking)"
    );
}

#[test]
fn issue_2347_normalize_model_alias_preserves_display_names() {
    assert_eq!(
        normalize_model_alias("Gemini 3.1 Pro (High)"),
        "Gemini 3.1 Pro (High)"
    );
    assert_eq!(
        normalize_model_alias("claude opus 4.6 (thinking)"),
        "Claude Opus 4.6 (Thinking)"
    );
}

#[test]
fn issue_2347_normalize_model_alias_passes_through_unknown() {
    assert_eq!(
        normalize_model_alias("some-custom-model"),
        "some-custom-model"
    );
}
