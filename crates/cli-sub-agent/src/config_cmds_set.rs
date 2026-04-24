use anyhow::Result;
use csa_config::{GlobalConfig, ProjectConfig};

pub(crate) fn handle_config_set(
    key: String,
    value: String,
    project: bool,
    cd: Option<String>,
) -> Result<()> {
    validate_config_set_value(&key, &value)?;

    let path = if project {
        let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
        ProjectConfig::config_path(&project_root)
    } else {
        GlobalConfig::config_path()?
    };

    write_config_value(&path, &key, &value)
}

fn validate_config_set_value(key: &str, value: &str) -> Result<()> {
    if key == "preferences.primary_writer_spec" {
        csa_executor::ModelSpec::parse(value).map_err(|err| {
            anyhow::anyhow!(
                "Invalid preferences.primary_writer_spec: {err}\n\
                 Expected format: tool/provider/model/thinking_budget"
            )
        })?;
    }

    Ok(())
}

fn write_config_value(path: &std::path::Path, key: &str, value: &str) -> Result<()> {
    let mut doc = match std::fs::read_to_string(path) {
        Ok(content) if !content.trim().is_empty() => content
            .parse::<toml_edit::DocumentMut>()
            .map_err(|err| anyhow::anyhow!("TOML parse error: {err}"))?,
        Ok(_) => toml_edit::DocumentMut::new(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => toml_edit::DocumentMut::new(),
        Err(err) => return Err(err.into()),
    };

    set_document_config_value(&mut doc, key, value)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, doc.to_string())?;
    Ok(())
}

fn set_document_config_value(
    doc: &mut toml_edit::DocumentMut,
    key: &str,
    value: &str,
) -> Result<()> {
    let parts = parse_dotted_key(key)?;
    set_table_config_value(doc.as_table_mut(), &parts, key, value, "")
}

fn parse_dotted_key(key: &str) -> Result<Vec<&str>> {
    let parts: Vec<&str> = key.split('.').collect();
    if parts.is_empty() || parts.iter().any(|part| part.is_empty()) {
        anyhow::bail!("Invalid config key path: {key}");
    }
    Ok(parts)
}

fn set_table_config_value(
    table: &mut toml_edit::Table,
    parts: &[&str],
    target_key: &str,
    value: &str,
    parent_path: &str,
) -> Result<()> {
    let Some((head, tail)) = parts.split_first() else {
        anyhow::bail!("Invalid empty config key path");
    };
    let current_path = if parent_path.is_empty() {
        (*head).to_string()
    } else {
        format!("{parent_path}.{head}")
    };

    if tail.is_empty() {
        let toml_value = value
            .parse::<toml_edit::Value>()
            .unwrap_or_else(|_| toml_edit::Value::from(value));
        table[head] = toml_edit::Item::Value(toml_value);
        return Ok(());
    }

    if !table.contains_key(head) || table[head].is_none() {
        table[head] = toml_edit::Item::Table(toml_edit::Table::new());
    }

    let item = &mut table[head];
    if item.as_inline_table().is_some() {
        anyhow::bail!(
            "Cannot set config key '{}': existing path '{}' is an inline table; \
             inline tables are not currently supported. Convert it to a standard TOML table first.",
            target_key,
            current_path
        );
    }

    let Some(child) = item.as_table_mut() else {
        anyhow::bail!(
            "Cannot set config key '{}': existing path '{}' is not a table",
            target_key,
            current_path
        );
    };

    set_table_config_value(child, tail, target_key, value, &current_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_cmds::{build_global_display_toml, resolve_effective_key};
    use crate::test_env_lock::TEST_ENV_LOCK;

    struct EnvVarGuard {
        key: &'static str,
        original: Option<String>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let original = std::env::var(key).ok();
            // SAFETY: test-scoped env mutation guarded by a process-wide mutex.
            unsafe { std::env::set_var(key, value) };
            Self { key, original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            // SAFETY: test-scoped env mutation guarded by a process-wide mutex.
            unsafe {
                match self.original.as_deref() {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    #[test]
    fn set_document_config_value_creates_nested_preferences_string_key() {
        let mut doc = toml_edit::DocumentMut::new();

        set_document_config_value(
            &mut doc,
            "preferences.primary_writer_spec",
            "codex/openai/gpt-5.4/high",
        )
        .unwrap();

        assert_eq!(
            doc["preferences"]["primary_writer_spec"].as_str(),
            Some("codex/openai/gpt-5.4/high")
        );
    }

    #[test]
    fn set_document_config_value_preserves_boolean_type() {
        let mut doc = toml_edit::DocumentMut::new();

        set_document_config_value(&mut doc, "tools.codex.enabled", "false").unwrap();

        assert_eq!(doc["tools"]["codex"]["enabled"].as_bool(), Some(false));
    }

    #[test]
    fn set_document_config_value_preserves_integer_type() {
        let mut doc = toml_edit::DocumentMut::new();

        set_document_config_value(&mut doc, "defaults.max_concurrent", "7").unwrap();

        assert_eq!(doc["defaults"]["max_concurrent"].as_integer(), Some(7));
    }

    #[test]
    fn set_document_config_value_rejects_inline_table_parent_with_context() {
        let mut doc = "[tools]\ncodex = { enabled = true }\n"
            .parse::<toml_edit::DocumentMut>()
            .unwrap();

        let err = set_document_config_value(&mut doc, "tools.codex.model", "gpt-5")
            .expect_err("inline table parent should be rejected");

        let message = err.to_string();
        assert!(
            message.contains("Cannot set config key 'tools.codex.model'"),
            "{message}"
        );
        assert!(message.contains("existing path 'tools.codex'"), "{message}");
        assert!(
            message.contains("inline tables are not currently supported"),
            "{message}"
        );
    }

    #[test]
    fn handle_config_set_global_primary_writer_spec_round_trips() {
        let _env_lock = TEST_ENV_LOCK.blocking_lock();
        let dir = tempfile::tempdir().unwrap();
        let config_root = dir.path().join("xdg-config");
        std::fs::create_dir_all(&config_root).unwrap();
        let _home_guard = EnvVarGuard::set("HOME", dir.path());
        let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);

        handle_config_set(
            "preferences.primary_writer_spec".to_string(),
            "codex/openai/gpt-5.4/high".to_string(),
            false,
            None,
        )
        .unwrap();

        let value = resolve_effective_key(
            Some(dir.path()),
            "preferences.primary_writer_spec",
            false,
            false,
        )
        .unwrap()
        .expect("primary_writer_spec should resolve");
        assert_eq!(value.as_str(), Some("codex/openai/gpt-5.4/high"));

        let rendered = toml::to_string_pretty(
            &build_global_display_toml(&GlobalConfig::load().unwrap()).unwrap(),
        )
        .unwrap();
        assert!(rendered.contains("primary_writer_spec = \"codex/openai/gpt-5.4/high\""));
    }

    #[test]
    fn handle_config_set_rejects_invalid_primary_writer_spec() {
        let err = handle_config_set(
            "preferences.primary_writer_spec".to_string(),
            "codex/openai/missing-thinking".to_string(),
            false,
            None,
        )
        .expect_err("invalid model spec should fail before writing");

        assert!(
            err.to_string()
                .contains("Invalid preferences.primary_writer_spec"),
            "unexpected error: {err}"
        );
    }
}
