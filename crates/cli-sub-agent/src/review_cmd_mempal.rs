use std::path::Path;

use csa_config::{GlobalConfig, MemoryBackend, ProjectConfig};
use tracing::warn;

pub(crate) fn maybe_capture_review_mempal(
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
    project_root: &Path,
    session_id: Option<&str>,
    tool_name: &str,
) {
    let memory_config = project_config
        .map(|config| &config.memory)
        .filter(|memory| !memory.is_default())
        .unwrap_or(&global_config.memory);
    if !matches!(
        csa_memory::resolve_backend(memory_config.backend),
        MemoryBackend::Mempal
    ) {
        return;
    }
    let Some(session_id) = session_id else {
        return;
    };
    match csa_session::get_session_dir(project_root, session_id) {
        Ok(session_dir) => {
            let result_path = session_dir.join("result.toml");
            csa_hooks::mempal_capture::spawn_mempal_ingest(
                memory_config,
                "csa-review",
                &result_path,
                project_root,
                Some(tool_name),
            );
        }
        Err(err) => warn!(
            session_id,
            error = %err,
            "Unable to resolve review session dir for mempal capture"
        ),
    }
}
