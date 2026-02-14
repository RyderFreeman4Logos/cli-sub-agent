//! Project configuration loading and validation (.csa/config.toml).

pub mod config;
pub mod global;
pub mod init;
pub mod mcp;
pub mod validate;

pub use config::{
    ProjectConfig, ProjectMeta, ResourcesConfig, TierConfig, ToolConfig, ToolRestrictions,
};
pub use global::GlobalConfig;
pub use init::{detect_installed_tools, init_project};
pub use mcp::{McpRegistry, McpServerConfig};
pub use validate::validate_config;
