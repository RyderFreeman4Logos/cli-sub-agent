use super::*;
use crate::global::ToolSelection;
use std::io;
use std::sync::{Arc, LazyLock, Mutex};
use tempfile::tempdir;
use tracing_subscriber::fmt::MakeWriter;

static TEST_TRACING_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[derive(Clone, Default)]
struct SharedLogBuffer {
    inner: Arc<Mutex<Vec<u8>>>,
}

impl SharedLogBuffer {
    fn contents(&self) -> String {
        String::from_utf8(self.inner.lock().expect("log buffer poisoned").clone())
            .expect("log buffer should be valid UTF-8")
    }
}

impl<'a> MakeWriter<'a> for SharedLogBuffer {
    type Writer = SharedLogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        SharedLogWriter {
            inner: Arc::clone(&self.inner),
        }
    }
}

struct SharedLogWriter {
    inner: Arc<Mutex<Vec<u8>>>,
}

impl io::Write for SharedLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner
            .lock()
            .expect("log buffer poisoned")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn test_load_nonexistent_returns_none() {
    let dir = tempdir().unwrap();
    // Use load_with_paths to isolate from real ~/.config/cli-sub-agent/config.toml on host.
    let project_path = dir.path().join(".csa").join("config.toml");
    let result = ProjectConfig::load_with_paths(None, &project_path).unwrap();
    assert!(result.is_none());
}

#[test]
fn test_save_and_load_roundtrip_with_review_override() {
    let dir = tempdir().unwrap();

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta {
            name: "test-project".to_string(),
            created_at: Utc::now(),
            max_recursion_depth: 5,
        },
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools: HashMap::new(),
        review: Some(crate::global::ReviewConfig {
            tool: ToolSelection::Single("codex".to_string()),
            ..Default::default()
        }),
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        tool_aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };

    config.save(dir.path()).unwrap();

    // Use load_with_paths to avoid accidental merge with host user config.
    let project_path = dir.path().join(".csa").join("config.toml");
    let loaded = ProjectConfig::load_with_paths(None, &project_path).unwrap();
    let loaded = loaded.unwrap();

    assert_eq!(
        loaded.review.unwrap().tool,
        ToolSelection::Single("codex".to_string())
    );
}

#[test]
fn test_project_config_deserializes_session_wait_override() {
    let config: ProjectConfig = toml::from_str(
        r#"
[project]
name = "test-project"

[session_wait]
memory_warn_mb = 8192
"#,
    )
    .unwrap();

    assert_eq!(
        config
            .session_wait
            .as_ref()
            .and_then(|cfg| cfg.memory_warn_mb),
        Some(8192)
    );
}

#[test]
fn test_resolve_session_wait_memory_warn_logs_parse_failures_at_error_level() {
    let _tracing_guard = TEST_TRACING_LOCK.lock().expect("tracing lock poisoned");
    let dir = tempdir().unwrap();
    let project_path = dir.path().join(".csa").join("config.toml");
    std::fs::create_dir_all(project_path.parent().expect("project config parent")).unwrap();
    std::fs::write(&project_path, "[session_wait\nmemory_warn_mb = 8192\n").unwrap();

    let buffer = SharedLogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::ERROR)
        .with_writer(buffer.clone())
        .without_time()
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    assert_eq!(
        ProjectConfig::resolve_session_wait_memory_warn_mb_with_paths(None, &project_path),
        None
    );

    let logs = buffer.contents();
    assert!(
        logs.contains(
            "Failed to parse config while resolving session wait memory warning threshold"
        ),
        "unexpected logs: {logs}"
    );
    assert!(
        logs.contains(&project_path.display().to_string()),
        "unexpected logs: {logs}"
    );
    assert!(logs.contains("unclosed table"), "unexpected logs: {logs}");
}

#[test]
fn test_enforce_tool_enabled_enabled_tool_returns_ok() {
    let mut tools = HashMap::new();
    tools.insert("codex".to_string(), ToolConfig::default());

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools,
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        tool_aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };

    assert!(config.enforce_tool_enabled("codex", false).is_ok());
}

#[test]
fn test_enforce_tool_enabled_unconfigured_tool_returns_ok() {
    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        tool_aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };

    assert!(config.enforce_tool_enabled("codex", false).is_ok());
}

#[test]
fn test_enforce_tool_enabled_force_override_bypasses_disabled() {
    let mut tools = HashMap::new();
    tools.insert(
        "codex".to_string(),
        ToolConfig {
            enabled: false,
            ..Default::default()
        },
    );

    let config = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools,
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        tool_aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };

    assert!(config.enforce_tool_enabled("codex", true).is_ok());
}

// ── SessionConfig tests ──────────────────────────────────────────

#[test]
fn test_session_config_default_has_structured_output_enabled() {
    let cfg = SessionConfig::default();
    assert!(cfg.structured_output);
    assert_eq!(cfg.resolved_spool_max_mb(), 32);
    assert!(cfg.resolved_spool_keep_rotated());
}

#[test]
fn test_session_config_is_default_reflects_structured_output() {
    let mut cfg = SessionConfig::default();
    assert!(cfg.is_default());

    cfg.structured_output = false;
    assert!(!cfg.is_default());
}

#[test]
fn test_session_config_deserializes_structured_output() {
    let toml_str = r#"
transcript_enabled = false
transcript_redaction = true
structured_output = false
"#;
    let cfg: SessionConfig = toml::from_str(toml_str).unwrap();
    assert!(!cfg.structured_output);
}

#[test]
fn test_session_config_defaults_structured_output_when_missing() {
    let toml_str = r#"
transcript_enabled = false
"#;
    let cfg: SessionConfig = toml::from_str(toml_str).unwrap();
    assert!(cfg.structured_output);
}

#[test]
fn test_session_config_default_does_not_require_commit_on_mutation() {
    let cfg = SessionConfig::default();
    assert!(!cfg.require_commit_on_mutation);
}

#[test]
fn test_session_config_deserializes_require_commit_on_mutation() {
    let toml_str = r#"
transcript_enabled = false
require_commit_on_mutation = true
"#;
    let cfg: SessionConfig = toml::from_str(toml_str).unwrap();
    assert!(cfg.require_commit_on_mutation);
}

#[test]
fn test_session_config_is_default_reflects_require_commit_on_mutation() {
    let cfg = SessionConfig {
        require_commit_on_mutation: true,
        ..Default::default()
    };
    assert!(!cfg.is_default());
}

#[test]
fn test_session_config_deserializes_spool_settings() {
    let toml_str = r#"
spool_max_mb = 64
spool_keep_rotated = false
"#;
    let cfg: SessionConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(cfg.spool_max_mb, Some(64));
    assert_eq!(cfg.spool_keep_rotated, Some(false));
    assert_eq!(cfg.resolved_spool_max_mb(), 64);
    assert!(!cfg.resolved_spool_keep_rotated());
}

#[test]
fn test_session_config_is_default_reflects_spool_overrides() {
    let cfg = SessionConfig {
        spool_max_mb: Some(64),
        ..Default::default()
    };
    assert!(!cfg.is_default());

    let cfg = SessionConfig {
        spool_keep_rotated: Some(false),
        ..Default::default()
    };
    assert!(!cfg.is_default());
}

// ---------------------------------------------------------------------------
// ResourcesConfig: initial_response_timeout_seconds
// ---------------------------------------------------------------------------

#[test]
fn test_resources_config_default_has_no_initial_response_timeout() {
    let cfg = ResourcesConfig::default();
    assert_eq!(cfg.initial_response_timeout_seconds, None);
}

#[test]
fn test_resources_config_is_default_with_default_initial_response_timeout() {
    let cfg = ResourcesConfig::default();
    assert!(cfg.is_default());
}

#[test]
fn test_resources_config_is_default_false_with_custom_initial_response_timeout() {
    let cfg = ResourcesConfig {
        initial_response_timeout_seconds: Some(60),
        ..Default::default()
    };
    assert!(!cfg.is_default());
}

#[test]
fn test_resources_config_deser_initial_response_timeout_custom() {
    let toml_str = r#"
initial_response_timeout_seconds = 60
"#;
    let cfg: ResourcesConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(cfg.initial_response_timeout_seconds, Some(60));
}

#[test]
fn test_resources_config_deser_initial_response_timeout_zero_disabled() {
    let toml_str = r#"
initial_response_timeout_seconds = 0
"#;
    let cfg: ResourcesConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(cfg.initial_response_timeout_seconds, Some(0));
}

#[test]
fn test_resources_config_deser_initial_response_timeout_omitted_defaults_to_none() {
    let toml_str = r#"
idle_timeout_seconds = 250
"#;
    let cfg: ResourcesConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(cfg.initial_response_timeout_seconds, None);
}
