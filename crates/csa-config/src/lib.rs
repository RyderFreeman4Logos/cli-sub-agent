//! Project configuration loading and validation (.csa/config.toml).

pub mod config;
pub mod global;
pub mod init;
pub mod mcp;
pub mod validate;

pub use config::{
    EnforcementMode, ProjectConfig, ProjectMeta, ResourcesConfig, TierConfig, ToolConfig,
    ToolRestrictions,
};
pub use global::{GlobalConfig, GlobalMcpConfig};
pub use init::{detect_installed_tools, init_project};
pub use mcp::{McpFilter, McpRegistry, McpServerConfig};
pub use validate::validate_config;
