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
    build_memory_section_for_resolved_backend(
        csa_memory::resolve_backend(memory_cfg.backend),
        memory_cfg,
        memory_query,
        memory_project_key,
        project_root,
    )
}

fn build_memory_section_for_resolved_backend(
    backend: MemoryBackend,
    memory_cfg: &MemoryConfig,
    memory_query: &str,
    memory_project_key: Option<&str>,
    project_root: &Path,
) -> Option<String> {
    match backend {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env_lock::ScopedTestEnvVar;
    use csa_memory::{MemoryEntry, MemorySource, MemoryStore};
    use tempfile::tempdir;
    use ulid::Ulid;

    #[test]
    fn auto_backend_fallback_injects_legacy_memory_when_resolved_legacy() {
        let temp = tempdir().expect("create tempdir");
        let _state = ScopedTestEnvVar::set("XDG_STATE_HOME", temp.path().join("state"));
        let memory_dir = temp
            .path()
            .join("state")
            .join("cli-sub-agent")
            .join("memory");
        let store = MemoryStore::new(memory_dir);
        let now = chrono::Utc::now();
        store
            .append(&MemoryEntry {
                id: Ulid::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV").expect("valid ulid"),
                timestamp: now,
                project: Some("test-project".to_string()),
                tool: Some("codex".to_string()),
                session_id: Some("01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string()),
                tags: Vec::new(),
                content: "legacy memory fallback preserves prompt context".to_string(),
                facts: Vec::new(),
                source: MemorySource::PostRun,
                valid_from: Some(now),
                valid_until: None,
            })
            .expect("append legacy memory");

        let config = MemoryConfig {
            backend: MemoryBackend::Auto,
            inject: true,
            inject_token_budget: 200,
            ..MemoryConfig::default()
        };

        let section = build_memory_section_for_resolved_backend(
            MemoryBackend::Legacy,
            &config,
            "legacy fallback context",
            Some("test-project"),
            temp.path(),
        )
        .expect("legacy memory section");

        assert!(section.contains("previous sessions"));
        assert!(section.contains("legacy memory fallback"));
        assert!(section.contains("<!-- CSA:MEMORY:END -->"));
    }
}
