use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutputCompactionConfig {
    pub sidecar_dir: PathBuf,
    pub threshold_bytes: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionConfig {
    #[serde(default)]
    pub no_load: Vec<String>,
    #[serde(default)]
    pub extra_load: Vec<String>,
    pub tier: Option<String>,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
    #[serde(default)]
    pub mcp_proxy_socket: Option<String>,
    #[serde(skip)]
    pub tool_output_compaction: Option<ToolOutputCompactionConfig>,
}
