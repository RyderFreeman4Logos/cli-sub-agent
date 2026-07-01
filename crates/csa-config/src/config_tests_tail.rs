use super::*;
use crate::global::ToolSelection;
use std::io;
use std::sync::{Arc, LazyLock, Mutex};
use tempfile::tempdir;
use tracing_subscriber::fmt::MakeWriter;

static TEST_TRACING_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

struct EnvVarGuard {
    key: &'static str,
    original: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let original = std::env::var(key).ok();
        // SAFETY: test-scoped env mutation is reverted in Drop.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: test-scoped env mutation is reverted in Drop.
        unsafe {
            match self.original.as_deref() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

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
        github: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        tool_state_dirs: HashMap::new(),
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
fn test_project_config_deserializes_github_override() {
    let config: ProjectConfig = toml::from_str(
        r#"
[project]
name = "test-project"

[github]
config_dir = "/tmp/project-gh"
"#,
    )
    .unwrap();

    assert_eq!(
        config
            .github
            .as_ref()
            .and_then(|cfg| cfg.config_dir.as_deref()),
        Some("/tmp/project-gh")
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
        logs.contains("Failed to parse config while resolving layered project settings"),
        "unexpected logs: {logs}"
    );
    assert!(
        logs.contains(&project_path.display().to_string()),
        "unexpected logs: {logs}"
    );
    assert!(logs.contains("unclosed table"), "unexpected logs: {logs}");
}

#[test]
fn test_resolve_github_config_dir_falls_back_to_home_gh_aider() {
    let dir = tempdir().unwrap();
    let project_path = dir.path().join(".csa").join("config.toml");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let _home_guard = EnvVarGuard::set("HOME", &home);
    let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", dir.path().join("xdg-config"));

    assert_eq!(
        ProjectConfig::resolve_github_config_dir_with_paths(None, &project_path).unwrap(),
        Some(home.join(".config/gh-aider").to_string_lossy().into_owned())
    );
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
        github: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        tool_state_dirs: HashMap::new(),
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
        github: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        tool_state_dirs: HashMap::new(),
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
        github: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        tool_state_dirs: HashMap::new(),
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
    assert_eq!(
        cfg.result_report_spill_threshold_bytes,
        DEFAULT_RESULT_REPORT_SPILL_THRESHOLD_BYTES
    );
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
fn test_session_config_plan_injection_defaults_to_true_when_missing() {
    let toml_str = r#"
transcript_enabled = false
"#;
    let cfg: SessionConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(cfg.plan_injection, None);
    assert!(cfg.resolved_plan_injection());
    assert!(cfg.is_default());
}

#[test]
fn test_session_config_deserializes_plan_injection_override() {
    let toml_str = r#"
plan_injection = false
"#;
    let cfg: SessionConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(cfg.plan_injection, Some(false));
    assert!(!cfg.resolved_plan_injection());
    assert!(!cfg.is_default());
}

#[test]
fn test_session_plan_injection_project_overrides_global() {
    let dir = tempdir().unwrap();

    let user_dir = dir.path().join("user");
    std::fs::create_dir_all(&user_dir).unwrap();
    let user_path = user_dir.join("config.toml");
    std::fs::write(
        &user_path,
        r#"
        [session]
        plan_injection = true
    "#,
    )
    .unwrap();

    let project_dir = dir.path().join("project").join(".csa");
    std::fs::create_dir_all(&project_dir).unwrap();
    let project_path = project_dir.join("config.toml");
    std::fs::write(
        &project_path,
        r#"
        [project]
        name = "test-project"

        [session]
        plan_injection = false
    "#,
    )
    .unwrap();

    let config = ProjectConfig::load_with_paths(Some(&user_path), &project_path)
        .unwrap()
        .unwrap();

    assert_eq!(config.session.plan_injection, Some(false));
    assert!(!config.session.resolved_plan_injection());
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
fn test_run_config_default_does_not_require_writer_commit() {
    let cfg = RunConfig::default();
    assert!(!cfg.writer_must_commit);
}

#[test]
fn test_run_config_deserializes_writer_must_commit() {
    let toml_str = r#"
writer_must_commit = true
"#;
    let cfg: RunConfig = toml::from_str(toml_str).unwrap();
    assert!(cfg.writer_must_commit);
}

#[test]
fn test_run_config_is_default_reflects_writer_must_commit() {
    let cfg = RunConfig {
        writer_must_commit: true,
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

#[test]
fn test_session_config_deserializes_result_report_spill_threshold() {
    let toml_str = r#"
result_report_spill_threshold_bytes = 2048
"#;
    let cfg: SessionConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(cfg.result_report_spill_threshold_bytes, 2048);
}

#[test]
fn test_session_config_is_default_reflects_result_report_spill_threshold() {
    let cfg = SessionConfig {
        result_report_spill_threshold_bytes: 2048,
        ..Default::default()
    };
    assert!(!cfg.is_default());
}

#[test]
fn test_session_config_default_fork_prefix_budget_is_none() {
    let cfg = SessionConfig::default();
    assert!(cfg.fork_prefix_budget.is_none());
    assert_eq!(
        cfg.resolved_fork_prefix_budget(),
        DEFAULT_FORK_PREFIX_BUDGET_TOKENS
    );
    assert!(cfg.is_default());
}

#[test]
fn test_session_config_fork_prefix_budget_roundtrip() {
    let cfg = SessionConfig {
        fork_prefix_budget: Some(DEFAULT_FORK_PREFIX_BUDGET_TOKENS),
        ..Default::default()
    };
    let toml_str = toml::to_string(&cfg).expect("serialize");
    assert!(
        toml_str.contains("fork_prefix_budget = 32768"),
        "expected fork_prefix_budget in serialized output, got: {toml_str}"
    );
    let parsed: SessionConfig = toml::from_str(&toml_str).expect("deserialize");
    assert_eq!(
        parsed.fork_prefix_budget,
        Some(DEFAULT_FORK_PREFIX_BUDGET_TOKENS)
    );
    assert_eq!(
        parsed.resolved_fork_prefix_budget(),
        DEFAULT_FORK_PREFIX_BUDGET_TOKENS
    );
}

#[test]
fn test_session_config_is_default_reflects_fork_prefix_budget() {
    let cfg = SessionConfig {
        fork_prefix_budget: Some(DEFAULT_FORK_PREFIX_BUDGET_TOKENS),
        ..Default::default()
    };
    assert!(!cfg.is_default());
}

#[test]
fn test_session_config_resolved_fork_prefix_budget_clamps_below_min() {
    let cfg = SessionConfig {
        fork_prefix_budget: Some(1024),
        ..Default::default()
    };
    assert_eq!(
        cfg.resolved_fork_prefix_budget(),
        FORK_PREFIX_BUDGET_MIN_TOKENS
    );
}

#[test]
fn test_session_config_resolved_fork_prefix_budget_clamps_above_max() {
    let cfg = SessionConfig {
        fork_prefix_budget: Some(1_000_000),
        ..Default::default()
    };
    assert_eq!(
        cfg.resolved_fork_prefix_budget(),
        FORK_PREFIX_BUDGET_MAX_TOKENS
    );
}

#[test]
fn test_session_config_resolved_fork_prefix_budget_passes_through_in_range() {
    let cfg = SessionConfig {
        fork_prefix_budget: Some(65_536),
        ..Default::default()
    };
    assert_eq!(cfg.resolved_fork_prefix_budget(), 65_536);
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

#[test]
fn test_resources_config_default_has_fatal_error_markers() {
    let cfg = ResourcesConfig::default();

    assert!(
        cfg.fatal_error_markers
            .iter()
            .any(|marker| marker == "HTTP 429"),
        "defaults should detect rate-limit HTTP failures"
    );
    assert!(
        cfg.fatal_error_markers
            .iter()
            .any(|marker| marker == "500 Internal Server Error"),
        "defaults should detect provider 5xx failures"
    );
    // #1652: over-broad bare-prose markers must NOT be defaults — they match benign model
    // output (e.g. "avoid the rate limit", "handle the provider error") and, combined with a
    // >30s no-progress pause, fast-fail healthy live sessions.
    for forbidden in [
        "rate limit",
        "provider error",
        "provider returned error",
        "overloaded",
    ] {
        assert!(
            !cfg.fatal_error_markers.iter().any(|m| m == forbidden),
            "default markers must not include over-broad bare phrase {forbidden:?} (#1652 FP)"
        );
    }
    assert!(cfg.is_default());
}

#[test]
fn test_resources_config_deser_fatal_error_markers_custom() {
    let toml_str = r#"
fatal_error_markers = ["custom fatal", "HTTP 418"]
"#;
    let cfg: ResourcesConfig = toml::from_str(toml_str).unwrap();

    assert_eq!(
        cfg.fatal_error_markers,
        vec!["custom fatal".to_string(), "HTTP 418".to_string()]
    );
    assert!(!cfg.is_default());
}

// ---------------------------------------------------------------------------
// enforce_tool_enabled: "Currently enabled tools:" alternatives hint
// ---------------------------------------------------------------------------

/// When a tool is disabled but other tools are enabled, the error message must
/// include a "Currently enabled tools:" line listing the alternatives.
#[test]
fn test_enforce_tool_enabled_includes_alternatives_when_others_enabled() {
    let mut tools = HashMap::new();
    // Disable codex; leave all others implicitly enabled (not in the map defaults to enabled).
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
        github: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        tool_state_dirs: HashMap::new(),
        filesystem_sandbox: Default::default(),
    };

    let result = config.enforce_tool_enabled("codex", false);
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("Currently enabled tools:"),
        "error should contain 'Currently enabled tools:' hint; got: {msg}"
    );
    // codex must NOT appear in the alternatives list
    let alternatives_line = msg
        .lines()
        .find(|l| l.contains("Currently enabled tools:"))
        .unwrap_or("");
    assert!(
        !alternatives_line.contains("codex"),
        "rejected tool 'codex' must not appear in alternatives: {alternatives_line}"
    );
}

/// When all known tools are disabled, the "Currently enabled tools:" line must
/// be omitted entirely — no point listing an empty set.
#[test]
fn test_enforce_tool_enabled_omits_hint_when_no_alternatives() {
    let mut tools = HashMap::new();
    for name in crate::global::all_known_tools() {
        tools.insert(
            name.as_str().to_string(),
            ToolConfig {
                enabled: false,
                ..Default::default()
            },
        );
    }

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
        github: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        tool_state_dirs: HashMap::new(),
        filesystem_sandbox: Default::default(),
    };

    let result = config.enforce_tool_enabled("codex", false);
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        !msg.contains("Currently enabled tools:"),
        "hint line must be absent when no alternatives exist; got: {msg}"
    );
}
