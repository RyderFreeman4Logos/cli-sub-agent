//! Project configuration loading and validation (.csa/config.toml).

pub mod acp;
pub mod config;
pub mod config_filesystem_sandbox;
mod config_github;
mod config_merge;
mod config_raw;
pub mod config_resources;
mod config_runtime;
pub(crate) mod config_session;
mod config_tier_helpers;
mod config_tiers;
pub mod config_tool;
mod configured_models;
mod convergence_completion_policy;
mod effective_config;
pub mod gc;
pub mod global;
mod global_caller_hints;
mod global_env;
mod global_impl;
mod global_kv_cache;
mod global_template;
pub mod init;
pub mod mcp;
pub mod memory;
pub mod migrate;
pub mod paths;
pub mod project_profile;
mod project_prune;
pub mod provider_detection;
pub mod tool_selection;
pub mod validate;
pub mod weave_lock;

pub use acp::AcpConfig;
pub use config::{
    DEFAULT_COOLDOWN_SECS, DEFAULT_FORK_PREFIX_BUDGET_TOKENS,
    DEFAULT_RESULT_REPORT_SPILL_THRESHOLD_BYTES, EnforcementMode, ExecutionConfig,
    FORK_PREFIX_BUDGET_MAX_TOKENS, FORK_PREFIX_BUDGET_MIN_TOKENS, HooksSection, PostExecGateConfig,
    ProjectConfig, ProjectMeta, RunConfig, SessionConfig, SnapshotTrigger, TierConfig,
    TierStrategy, ToolConfig, ToolFilesystemSandboxConfig, ToolResourceProfile, ToolRestrictions,
    VcsConfig,
};
pub use config_session::{
    DEFAULT_TOOL_OUTPUT_THRESHOLD_BYTES, RunLargeDiffWarningConfig, RunLargeDiffWarningMode,
};
pub type MergedConfig = ProjectConfig;
pub use config_filesystem_sandbox::FilesystemSandboxConfig;
pub use config_resources::ResourcesConfig;
pub use config_runtime::{DefaultSandboxOptions, default_sandbox_for_tool};
pub use config_tool::{TransportKind, default_transport_for_tool};
pub use convergence_completion_policy::{
    ConvergenceCompletionPolicy, EffectiveConvergenceCompletionPolicy,
    ProjectConvergenceCompletionPolicy, parse_project_convergence_completion_policy,
};
pub use csa_core::model_catalog::{
    CatalogAdmission, CatalogErrorKind, CatalogLegalityError, CatalogLoadError, CatalogProvenance,
    CatalogWarning, CatalogWarningKind, ConfiguredSpecError, EffectiveModelCatalog,
    ReasoningEffort,
};
pub use effective_config::EffectiveConfig;
pub use gc::GcConfig;
pub use global::{
    AiConfigSymlinkCheckConfig, BudgetConfig, DEFAULT_CLAUDE_STATE_DIR, DEFAULT_CODEX_STATE_DIR,
    DEFAULT_KV_CACHE_FREQUENT_POLL_SECS, DEFAULT_KV_CACHE_LONG_POLL_SECS, ExecutionEnvOptions,
    ExperimentalConfig, GateMode, GateStep, GithubConfig, GlobalConfig, GlobalHooksConfig,
    GlobalMcpConfig, KvCacheConfig, KvCacheValueSource, LEGACY_SESSION_WAIT_FALLBACK_SECS,
    PreflightConfig, ProviderTtls, ResolvedKvCacheValue, RetryConfig, ReviewConfig,
    SessionWaitConfig, StateDirConfig, StateDirOnExceed, TierPolicyConfig, ToolSelection,
    default_tool_state_dirs, ensure_default_tool_state_dirs,
};
pub use global_caller_hints::{
    CallerHintsConfig, DEFAULT_CODEX_SESSION_WAIT_MCP_INTERNAL_TIMEOUT_SEC,
    DEFAULT_CODEX_SESSION_WAIT_MCP_TOOL_TIMEOUT_SEC, DEFAULT_CODEX_SESSION_WAIT_YIELD_MS,
};
pub use init::{detect_installed_tools, init_project};
pub use mcp::{McpFilter, McpRegistry, McpServerConfig, McpTransport};
pub use memory::{MemoryBackend, MemoryConfig, MemoryEphemeralConfig, MemoryLlmConfig};
pub use migrate::{Migration, MigrationRegistry, MigrationStep, Version, default_registry};
pub use paths::{APP_NAME, LEGACY_APP_NAME};
pub use project_profile::{ProjectProfile, detect_project_profile};
pub use provider_detection::{
    ModelProvider, detect_model_provider, parse_model_provider, provider_ttl,
};
pub use validate::validate_config;
pub use weave_lock::{VersionCheckResult, WeaveLock, check_version};
