//! Project configuration loading and validation (.csa/config.toml).

pub mod config;
mod config_runtime;
pub mod global;
pub mod init;
pub mod mcp;
pub mod migrate;
pub mod validate;
pub mod weave_lock;

pub use config::{
    EnforcementMode, ProjectConfig, ProjectMeta, ResourcesConfig, TierConfig, ToolConfig,
    ToolResourceProfile, ToolRestrictions,
};
pub use global::{GlobalConfig, GlobalMcpConfig};
pub use init::{detect_installed_tools, init_project};
pub use mcp::{McpFilter, McpRegistry, McpServerConfig};
pub use migrate::{Migration, MigrationRegistry, MigrationStep, Version};
pub use validate::validate_config;
pub use weave_lock::WeaveLock;
