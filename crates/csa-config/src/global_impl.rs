use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::global::{
    DEFAULT_KV_CACHE_LONG_POLL_SECS, GlobalConfig, KvCacheValueSource, ResolvedKvCacheValue,
    heterogeneous_counterpart, sort_tools_by_priority,
};
use crate::mcp::McpServerConfig;
use crate::paths;
use csa_core::types::ToolName;

impl GlobalConfig {
    /// Return a copy suitable for user-facing display/logging.
    ///
    /// Sensitive fields (e.g. API keys) are masked.
    pub fn redacted_for_display(&self) -> Self {
        let mut redacted = self.clone();
        redacted.memory.llm = redacted.memory.llm.redacted_for_display();
        for tool_cfg in redacted.tools.values_mut() {
            if tool_cfg.api_key.is_some() {
                tool_cfg.api_key = Some("***REDACTED***".to_string());
            }
        }
        redacted
    }

    /// Load global config from `~/.config/cli-sub-agent/config.toml`.
    ///
    /// Returns `Default` if the file does not exist or if the config
    /// directory cannot be determined (e.g., no HOME in containers).
    pub fn load() -> Result<Self> {
        let path = match paths::config_dir() {
            Some(dir) => dir.join("config.toml"),
            None => return Ok(Self::default()),
        };
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read global config: {}", path.display()))?;
        let config: Self = toml::from_str(&content)
            .with_context(|| format!("Failed to parse global config: {}", path.display()))?;
        Ok(config.sanitized(Some(&path)))
    }

    /// Resolve the `csa session wait` long-poll cap from the global config.
    ///
    /// Missing or invalid config falls back to the documented KV cache default.
    /// Once `[kv_cache]` exists, `long_poll_seconds` still defaults to 240 if omitted.
    pub fn resolve_session_wait_long_poll_seconds() -> u64 {
        Self::resolve_session_wait_long_poll_seconds_with_source().seconds
    }

    pub fn resolve_session_wait_long_poll_seconds_with_source() -> ResolvedKvCacheValue {
        let config_dir = paths::config_dir();
        Self::resolve_session_wait_long_poll_seconds_from_dir_with_source(config_dir.as_deref())
    }

    #[cfg(test)]
    pub(crate) fn resolve_session_wait_long_poll_seconds_from_dir(
        config_dir: Option<&Path>,
    ) -> u64 {
        Self::resolve_session_wait_long_poll_seconds_from_dir_with_source(config_dir).seconds
    }

    pub(crate) fn resolve_session_wait_long_poll_seconds_from_dir_with_source(
        config_dir: Option<&Path>,
    ) -> ResolvedKvCacheValue {
        let path = config_dir.map(|dir| dir.join("config.toml"));
        Self::resolve_session_wait_long_poll_seconds_from_path_with_source(path.as_deref())
    }

    pub fn resolve_session_wait_long_poll_seconds_from_path(path: Option<&Path>) -> u64 {
        Self::resolve_session_wait_long_poll_seconds_from_path_with_source(path).seconds
    }

    pub fn resolve_session_wait_long_poll_seconds_from_path_with_source(
        path: Option<&Path>,
    ) -> ResolvedKvCacheValue {
        let Some(path) = path else {
            return ResolvedKvCacheValue::documented_default();
        };

        let content = match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return ResolvedKvCacheValue::documented_default();
            }
            Err(err) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "Failed to read global config while resolving session wait timeout"
                );
                return ResolvedKvCacheValue::documented_default();
            }
        };

        let raw: toml::Value = match toml::from_str(&content) {
            Ok(raw) => raw,
            Err(err) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "Failed to parse global config while resolving session wait timeout"
                );
                return ResolvedKvCacheValue::documented_default();
            }
        };

        let Some(kv_cache) = raw.get("kv_cache").and_then(toml::Value::as_table) else {
            return ResolvedKvCacheValue::documented_default();
        };

        match kv_cache.get("long_poll_seconds") {
            None => ResolvedKvCacheValue::section_default(),
            Some(value) => match value.as_integer() {
                Some(seconds) if seconds > 0 => match u64::try_from(seconds) {
                    Ok(seconds) => ResolvedKvCacheValue {
                        seconds,
                        source: KvCacheValueSource::Configured,
                    },
                    Err(_) => {
                        tracing::warn!(
                            path = %path.display(),
                            key = "kv_cache.long_poll_seconds",
                            value = seconds,
                            fallback = DEFAULT_KV_CACHE_LONG_POLL_SECS,
                            "Ignoring out-of-range KV cache interval; using section default"
                        );
                        ResolvedKvCacheValue::section_default()
                    }
                },
                _ => {
                    tracing::warn!(
                        path = %path.display(),
                        key = "kv_cache.long_poll_seconds",
                        fallback = DEFAULT_KV_CACHE_LONG_POLL_SECS,
                        "Ignoring invalid KV cache interval; using section default"
                    );
                    ResolvedKvCacheValue::section_default()
                }
            },
        }
    }

    fn sanitized(mut self, path: Option<&Path>) -> Self {
        self.kv_cache = self.kv_cache.sanitized(path);
        self
    }

    /// Get the resolved maximum concurrent count for a tool.
    ///
    /// Lookup order: tool-specific override -> defaults.max_concurrent.
    pub fn max_concurrent(&self, tool: &str) -> u32 {
        self.tools
            .get(tool)
            .and_then(|t| t.max_concurrent)
            .unwrap_or(self.defaults.max_concurrent)
    }

    /// Sort tools by user-configured priority order.
    ///
    /// Tools in `preferences.tool_priority` appear first (in priority order).
    /// Tools NOT in the priority list retain their original relative order.
    /// Returns unchanged when no priority is configured.
    pub fn sort_by_priority(&self, tools: &[ToolName]) -> Vec<ToolName> {
        sort_tools_by_priority(tools, &self.preferences.tool_priority)
    }

    /// Get environment variables to inject for a tool.
    pub fn env_vars(&self, tool: &str) -> Option<&HashMap<String, String>> {
        self.tools
            .get(tool)
            .map(|t| &t.env)
            .filter(|m| !m.is_empty())
    }

    /// Get API key fallback for a tool (used when OAuth quota is exhausted).
    pub fn api_key_fallback(&self, tool: &str) -> Option<&str> {
        self.tools.get(tool).and_then(|t| t.api_key.as_deref())
    }

    /// Whether gemini-cli may retry after stripping unhealthy MCP servers.
    ///
    /// Missing config defaults to `true` to keep MCP degradation non-fatal.
    pub fn allow_degraded_mcp(&self, tool: &str) -> bool {
        self.tools
            .get(tool)
            .and_then(|t| t.allow_degraded_mcp)
            .unwrap_or(true)
    }

    /// Get the thinking budget lock for a tool from global config.
    pub fn thinking_lock(&self, tool: &str) -> Option<&str> {
        self.tools
            .get(tool)
            .and_then(|t| t.thinking_lock.as_deref())
    }

    /// Get globally configured MCP servers.
    pub fn mcp_servers(&self) -> &[McpServerConfig] {
        &self.mcp.servers
    }

    /// Path to the global config file: `~/.config/cli-sub-agent/config.toml`.
    pub fn config_path() -> Result<PathBuf> {
        let dir = paths::config_dir_write().context("Failed to determine config directory")?;
        Ok(dir.join("config.toml"))
    }

    /// Path to the global slots directory.
    ///
    /// Base state directory for all CSA data (`~/.local/state/cli-sub-agent/`).
    ///
    /// Used by `--global` GC to scan all project session trees.
    pub fn state_base_dir() -> Result<PathBuf> {
        let base = paths::state_dir().unwrap_or_else(paths::state_dir_fallback);
        Ok(base)
    }

    /// Resolution order:
    /// 1. `~/.local/state/cli-sub-agent/slots/` (XDG state dir on Linux)
    /// 2. Platform-equivalent state dir (macOS/Windows)
    /// 3. `$TMPDIR/cli-sub-agent-state/slots/` (fallback when state_dir unavailable)
    /// 4. `$TMPDIR/cli-sub-agent-state/slots/` (fallback when HOME/XDG unset, e.g. containers)
    ///
    /// This function never fails — it always returns a usable path.
    pub fn slots_dir() -> Result<PathBuf> {
        let base = paths::state_dir_write().unwrap_or_else(paths::state_dir_fallback);
        Ok(base.join("slots"))
    }

    /// Generate default config TOML with comments as a template.
    pub fn default_template() -> String {
        crate::global_template::default_template()
    }

    /// Save the default template to the config path, creating directories as needed.
    /// Returns the path where the file was written.
    pub fn save_default_template() -> Result<PathBuf> {
        let path = Self::config_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create config directory: {}", parent.display())
            })?;
        }
        std::fs::write(&path, Self::default_template())
            .with_context(|| format!("Failed to write global config: {}", path.display()))?;
        Ok(path)
    }

    /// Resolve the review tool based on config and parent tool context.
    ///
    /// In `auto` mode:
    /// - Parent is `claude-code` → `codex` (model heterogeneity)
    /// - Parent is `codex` → `claude-code`
    /// - Otherwise → error with guidance to configure manually
    pub fn resolve_review_tool(&self, parent_tool: Option<&str>) -> Result<String> {
        if let Some(single) = self.review.tool.as_single() {
            return Ok(single.to_string());
        }
        // auto or whitelist — both use auto resolution
        resolve_auto_tool("review", parent_tool)
    }

    /// Resolve the debate tool based on config and parent tool context.
    ///
    /// In `auto` mode:
    /// - Parent is `claude-code` → `codex` (model heterogeneity)
    /// - Parent is `codex` → `claude-code`
    /// - Otherwise → error with guidance to configure manually
    pub fn resolve_debate_tool(&self, parent_tool: Option<&str>) -> Result<String> {
        if let Some(single) = self.debate.tool.as_single() {
            return Ok(single.to_string());
        }
        // auto or whitelist — both use auto resolution
        resolve_auto_tool("debate", parent_tool)
    }

    /// List all known tool names (from config + static list).
    pub fn all_tool_slots(&self) -> Vec<(&str, u32)> {
        let static_tools = ["gemini-cli", "opencode", "codex", "claude-code"];
        let mut result: Vec<(&str, u32)> = static_tools
            .iter()
            .map(|t| (*t, self.max_concurrent(t)))
            .collect();

        // Add any extra tools from config not in static list
        for tool in self.tools.keys() {
            if !static_tools.contains(&tool.as_str()) {
                result.push((tool.as_str(), self.max_concurrent(tool)));
            }
        }

        result
    }
}

/// Resolve "auto" tool selection using the heterogeneous counterpart mapping.
fn resolve_auto_tool(section: &str, parent_tool: Option<&str>) -> Result<String> {
    match parent_tool.and_then(heterogeneous_counterpart) {
        Some(counterpart) => Ok(counterpart.to_string()),
        None => {
            let context = match parent_tool {
                Some(p) => format!("parent is '{p}'"),
                None => "no parent tool context".to_string(),
            };
            Err(anyhow::anyhow!(
                "Cannot auto-detect {section} tool: {context}. \
                 Set [{section}] tool to an explicit tool (e.g., \"codex\" or \"claude-code\") \
                 in ~/.config/cli-sub-agent/config.toml"
            ))
        }
    }
}
