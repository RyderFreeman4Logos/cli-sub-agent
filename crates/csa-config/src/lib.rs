//! Project configuration loading and validation (.csa/config.toml).

pub mod config;
pub mod init;
pub mod validate;

pub use config::{
    ProjectConfig, ProjectMeta, ResourcesConfig, TierConfig, ToolConfig, ToolRestrictions,
};
pub use init::{detect_installed_tools, init_project};
pub use validate::validate_config;
