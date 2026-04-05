//! Project configuration loading and validation (.csa/config.toml).

pub mod acp;
pub mod config;
pub mod config_filesystem_sandbox;
mod config_merge;
pub mod config_resources;
mod config_runtime;
pub(crate) mod config_session;
mod config_tiers;
pub mod config_tool;
pub mod gc;
pub mod global;
mod global_env;
pub mod init;
pub mod mcp;
pub mod memory;
pub mod migrate;
pub mod paths;
pub mod project_profile;
pub mod tool_selection;
pub mod validate;
pub mod weave_lock;

pub use acp::AcpConfig;
pub use config::{
    DEFAULT_DAEMON_WAIT_SECS, EnforcementMode, ExecutionConfig, HooksSection, ProjectConfig,
    ProjectMeta, SessionConfig, TierConfig, TierStrategy, ToolConfig, ToolFilesystemSandboxConfig,
    ToolResourceProfile, ToolRestrictions,
};
pub use config_filesystem_sandbox::FilesystemSandboxConfig;
pub use config_resources::ResourcesConfig;
pub use config_runtime::{DefaultSandboxOptions, default_sandbox_for_tool};
pub use gc::GcConfig;
pub use global::{
    ExecutionEnvOptions, GateMode, GateStep, GlobalConfig, GlobalMcpConfig, ReviewConfig,
    ToolSelection,
};
pub use init::{detect_installed_tools, init_project};
pub use mcp::{McpFilter, McpRegistry, McpServerConfig, McpTransport};
pub use memory::{MemoryConfig, MemoryEphemeralConfig, MemoryLlmConfig};
pub use migrate::{Migration, MigrationRegistry, MigrationStep, Version, default_registry};
pub use paths::{APP_NAME, LEGACY_APP_NAME};
pub use project_profile::{ProjectProfile, detect_project_profile};
pub use validate::validate_config;
pub use weave_lock::{VersionCheckResult, WeaveLock, check_version};
