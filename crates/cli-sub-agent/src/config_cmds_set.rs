use anyhow::Result;
use csa_config::{GlobalConfig, ProjectConfig};

pub(crate) fn handle_config_set(
    key: String,
    value: String,
    project: bool,
    _global: bool,
    cd: Option<String>,
) -> Result<()> {
    validate_config_set_value(&key, &value)?;

    let path = if project {
        let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
        ProjectConfig::config_path(&project_root)
    } else {
        GlobalConfig::config_path()?
    };

    write_string_config_value(&path, &key, &value)
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

fn write_string_config_value(path: &std::path::Path, key: &str, value: &str) -> Result<()> {
    let mut doc = match std::fs::read_to_string(path) {
        Ok(content) if !content.trim().is_empty() => content
            .parse::<toml_edit::DocumentMut>()
            .map_err(|err| anyhow::anyhow!("TOML parse error: {err}"))?,
        Ok(_) => toml_edit::DocumentMut::new(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => toml_edit::DocumentMut::new(),
        Err(err) => return Err(err.into()),
    };

    set_document_string_value(&mut doc, key, value)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, doc.to_string())?;
    Ok(())
}

fn set_document_string_value(
    doc: &mut toml_edit::DocumentMut,
    key: &str,
    value: &str,
) -> Result<()> {
    let parts = parse_dotted_key(key)?;
    set_table_string_value(doc.as_table_mut(), &parts, value)
}

fn parse_dotted_key(key: &str) -> Result<Vec<&str>> {
    let parts: Vec<&str> = key.split('.').collect();
    if parts.is_empty() || parts.iter().any(|part| part.is_empty()) {
        anyhow::bail!("Invalid config key path: {key}");
    }
    Ok(parts)
}

fn set_table_string_value(table: &mut toml_edit::Table, parts: &[&str], value: &str) -> Result<()> {
    let Some((head, tail)) = parts.split_first() else {
        anyhow::bail!("Invalid empty config key path");
    };

    if tail.is_empty() {
        table[head] = toml_edit::value(value);
        return Ok(());
    }

    if !table.contains_key(head) || table[head].is_none() {
        table[head] = toml_edit::Item::Table(toml_edit::Table::new());
    }

    let Some(child) = table[head].as_table_mut() else {
        anyhow::bail!("Cannot set {head}: existing value is not a table");
    };

    set_table_string_value(child, tail, value)
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
    fn set_document_string_value_creates_nested_preferences_key() {
        let mut doc = toml_edit::DocumentMut::new();

        set_document_string_value(
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
