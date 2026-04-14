use anyhow::{Context, Result};
use csa_executor::Executor;
use std::fs;
use std::path::Path;

pub(super) fn persist_session_runtime_binary(
    session_dir: &Path,
    executor: &Executor,
) -> Result<()> {
    let metadata_path = session_dir.join(csa_session::metadata::METADATA_FILE_NAME);
    let mut metadata = if metadata_path.is_file() {
        let contents = fs::read_to_string(&metadata_path)
            .with_context(|| format!("Failed to read metadata: {}", metadata_path.display()))?;
        toml::from_str::<csa_session::metadata::SessionMetadata>(&contents)
            .with_context(|| format!("Failed to parse metadata: {}", metadata_path.display()))?
    } else {
        csa_session::metadata::SessionMetadata {
            tool: executor.tool_name().to_string(),
            tool_locked: true,
            runtime_binary: None,
        }
    };

    let runtime_binary = executor.runtime_binary_name();
    if metadata.runtime_binary.as_deref() == Some(runtime_binary) {
        return Ok(());
    }
    metadata.runtime_binary = Some(runtime_binary.to_string());

    let contents =
        toml::to_string_pretty(&metadata).context("Failed to serialize session metadata")?;
    fs::write(&metadata_path, contents)
        .with_context(|| format!("Failed to write metadata: {}", metadata_path.display()))?;
    Ok(())
}
