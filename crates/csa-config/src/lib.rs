//! Project configuration loading and validation (.csa/config.toml).

pub mod acp;
pub mod config;
mod config_merge;
mod config_runtime;
pub mod gc;
pub mod global;
pub mod init;
pub mod mcp;
pub mod migrate;
pub mod paths;
pub mod validate;
pub mod weave_lock;

pub use acp::AcpConfig;
pub use config::{
    EnforcementMode, ProjectConfig, ProjectMeta, ResourcesConfig, SessionConfig, TierConfig,
    ToolConfig, ToolResourceProfile, ToolRestrictions,
};
pub use config_runtime::{DefaultSandboxOptions, default_sandbox_for_tool};
pub use gc::GcConfig;
pub use global::{GlobalConfig, GlobalMcpConfig};
pub use init::{detect_installed_tools, init_project};
pub use mcp::{McpFilter, McpRegistry, McpServerConfig};
pub use migrate::{Migration, MigrationRegistry, MigrationStep, Version, default_registry};
pub use paths::{APP_NAME, LEGACY_APP_NAME};
pub use validate::validate_config;
pub use weave_lock::{VersionCheckResult, WeaveLock, check_version};
