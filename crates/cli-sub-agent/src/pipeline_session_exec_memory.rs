use std::path::Path;

use csa_config::{MemoryBackend, MemoryConfig};
use tracing::{debug, info};

use crate::memory_capture;

use super::MemoryInjectionOptions;

pub(super) fn append_memory_section(
    memory_cfg: Option<&MemoryConfig>,
    memory_injection: Option<&MemoryInjectionOptions>,
    raw_prompt: &str,
    memory_project_key: Option<&str>,
    project_root: &Path,
    tool_name: &str,
    effective_prompt: &mut String,
) {
    let memory_disabled =
        memory_injection.is_none() || memory_injection.is_some_and(|opts| opts.disabled);
    let Some(memory_cfg) = memory_cfg else {
        return;
    };
    if !memory_cfg.inject || memory_disabled {
        return;
    }
    if csa_hooks::mempal_capture::tool_has_own_mempal(tool_name) {
        debug!(
            tool = tool_name,
            "skipping mempal prompt injection for {tool_name} (has own integration)"
        );
        return;
    }

    let memory_query = memory_injection
        .and_then(|opts| opts.query_override.as_deref())
        .unwrap_or(raw_prompt);
    if let Some(memory_section) =
        build_memory_section_for_backend(memory_cfg, memory_query, memory_project_key, project_root)
    {
        info!(
            bytes = memory_section.len(),
            "Injecting memory context into prompt"
        );
        effective_prompt.push_str(&memory_section);
    }
}

fn build_memory_section_for_backend(
    memory_cfg: &MemoryConfig,
    memory_query: &str,
    memory_project_key: Option<&str>,
    project_root: &Path,
) -> Option<String> {
    match csa_memory::resolve_backend(memory_cfg.backend) {
        MemoryBackend::Mempal => memory_capture::build_memory_section_from_mempal(
            memory_query,
            project_root,
            memory_cfg.inject_token_budget,
        ),
        MemoryBackend::Legacy | MemoryBackend::Auto => {
            memory_capture::build_memory_section(memory_cfg, memory_query, memory_project_key)
        }
    }
}
