use crate::config::{EnforcementMode, ProjectConfig, ToolResourceProfile};

/// Known tool-to-profile mapping based on runtime characteristics.
///
/// Lightweight: native binaries with predictable memory (Rust, Go).
/// Heavyweight: Node.js runtimes with dynamic plugin/MCP loading.
fn default_profile(tool: &str) -> ToolResourceProfile {
    match tool {
        "codex" | "opencode" => ToolResourceProfile::Lightweight,
        "claude-code" | "gemini-cli" => ToolResourceProfile::Heavyweight,
        _ => ToolResourceProfile::Heavyweight,
    }
}

/// Default resource limits for each profile.
struct ProfileDefaults {
    enforcement: EnforcementMode,
    memory_max_mb: Option<u64>,
    memory_swap_max_mb: Option<u64>,
}

fn profile_defaults(profile: ToolResourceProfile) -> ProfileDefaults {
    match profile {
        ToolResourceProfile::Lightweight => ProfileDefaults {
            enforcement: EnforcementMode::Off,
            memory_max_mb: None,
            memory_swap_max_mb: None,
        },
        ToolResourceProfile::Heavyweight => ProfileDefaults {
            enforcement: EnforcementMode::BestEffort,
            memory_max_mb: Some(2048),
            memory_swap_max_mb: Some(0),
        },
        ToolResourceProfile::Custom => ProfileDefaults {
            enforcement: EnforcementMode::Off,
            memory_max_mb: None,
            memory_swap_max_mb: None,
        },
    }
}

impl ProjectConfig {
    /// Resolve the resource profile for a tool.
    ///
    /// If the tool has any explicit resource override (enforcement_mode,
    /// memory_max_mb, or memory_swap_max_mb), returns `Custom`.
    /// Otherwise returns the default profile for the tool's runtime.
    pub fn tool_resource_profile(&self, tool: &str) -> ToolResourceProfile {
        if let Some(tc) = self.tools.get(tool) {
            if tc.enforcement_mode.is_some()
                || tc.memory_max_mb.is_some()
                || tc.memory_swap_max_mb.is_some()
            {
                return ToolResourceProfile::Custom;
            }
        }
        default_profile(tool)
    }

    /// Resolve sandbox enforcement mode for a specific tool.
    ///
    /// Priority: tool-level override > project resources > inherent profile default.
    ///
    /// The inherent profile is the tool's *runtime-based* profile (Lightweight or
    /// Heavyweight), NOT the resolved profile. A tool that resolves to `Custom`
    /// because it has `memory_max_mb` overrides should still inherit the enforcement
    /// mode of its inherent profile (e.g. Heavyweight → BestEffort) rather than
    /// falling back to `Off`.
    pub fn tool_enforcement_mode(&self, tool: &str) -> EnforcementMode {
        // 1. Explicit per-tool override
        if let Some(mode) = self.tools.get(tool).and_then(|t| t.enforcement_mode) {
            return mode;
        }
        // 2. Project-level resources setting
        if let Some(mode) = self.resources.enforcement_mode {
            return mode;
        }
        // 3. Inherent profile default (runtime-based, ignoring Custom overrides)
        profile_defaults(default_profile(tool)).enforcement
    }

    /// Resolve sandbox enforcement mode (defaults to `Off`).
    ///
    /// Legacy method: returns the global enforcement mode without per-tool
    /// or profile awareness. Prefer [`tool_enforcement_mode`] for new code.
    pub fn enforcement_mode(&self) -> EnforcementMode {
        self.resources
            .enforcement_mode
            .unwrap_or(EnforcementMode::Off)
    }

    /// Resolve memory_max_mb for a tool.
    ///
    /// Priority: tool-level override > project resources > profile default > None.
    pub fn sandbox_memory_max_mb(&self, tool: &str) -> Option<u64> {
        self.tools
            .get(tool)
            .and_then(|t| t.memory_max_mb)
            .or(self.resources.memory_max_mb)
            .or_else(|| {
                let profile = self.tool_resource_profile(tool);
                profile_defaults(profile).memory_max_mb
            })
    }

    /// Resolve memory_swap_max_mb for a tool.
    ///
    /// Priority: tool-level override > project resources > profile default > None.
    pub fn sandbox_memory_swap_max_mb(&self, tool: &str) -> Option<u64> {
        self.tools
            .get(tool)
            .and_then(|t| t.memory_swap_max_mb)
            .or(self.resources.memory_swap_max_mb)
            .or_else(|| {
                let profile = self.tool_resource_profile(tool);
                profile_defaults(profile).memory_swap_max_mb
            })
    }

    /// Resolve node_heap_limit_mb: tool-level override > project resources > profile default > None.
    pub fn sandbox_node_heap_limit_mb(&self, tool: &str) -> Option<u64> {
        self.tools
            .get(tool)
            .and_then(|t| t.node_heap_limit_mb)
            .or(self.resources.node_heap_limit_mb)
            .or_else(|| match default_profile(tool) {
                ToolResourceProfile::Heavyweight => Some(2048),
                _ => None,
            })
    }

    /// Resolve pids_max from project resources config.
    pub fn sandbox_pids_max(&self) -> Option<u32> {
        self.resources.pids_max
    }

    /// Check if notification hooks should be suppressed for a tool.
    ///
    /// Defaults to `true` (suppress) since CSA always runs tools as
    /// non-interactive sub-agents where desktop notifications are not useful.
    pub fn should_suppress_notify(&self, tool: &str) -> bool {
        self.tools
            .get(tool)
            .map(|t| t.suppress_notify)
            .unwrap_or(true)
    }

    /// Resolve lean_mode for a tool.
    ///
    /// Defaults to `true` for Heavyweight tools, `false` otherwise.
    pub fn tool_lean_mode(&self, tool: &str) -> bool {
        self.tools
            .get(tool)
            .and_then(|t| t.lean_mode)
            .unwrap_or_else(|| matches!(default_profile(tool), ToolResourceProfile::Heavyweight))
    }

    /// Check if a tool is allowed to edit existing files.
    pub fn can_tool_edit_existing(&self, tool: &str) -> bool {
        self.tools
            .get(tool)
            .and_then(|t| t.restrictions.as_ref())
            .map(|r| r.allow_edit_existing_files)
            .unwrap_or(true)
    }
}

/// Default sandbox options derived from a tool's resource profile.
/// Used when no ProjectConfig is available (e.g., no .csa/config.toml).
#[derive(Debug, Clone)]
pub struct DefaultSandboxOptions {
    pub enforcement: EnforcementMode,
    pub memory_max_mb: Option<u64>,
    pub memory_swap_max_mb: Option<u64>,
    pub lean_mode: bool,
    pub node_heap_limit_mb: Option<u64>,
}

/// Get default sandbox options for a tool based on its resource profile.
/// This is the public gateway to the private `default_profile()` / `profile_defaults()`.
pub fn default_sandbox_for_tool(tool: &str) -> DefaultSandboxOptions {
    let profile = default_profile(tool);
    let defaults = profile_defaults(profile);
    DefaultSandboxOptions {
        enforcement: defaults.enforcement,
        memory_max_mb: defaults.memory_max_mb,
        memory_swap_max_mb: defaults.memory_swap_max_mb,
        lean_mode: matches!(profile, ToolResourceProfile::Heavyweight),
        node_heap_limit_mb: match profile {
            ToolResourceProfile::Heavyweight => Some(2048),
            _ => None,
        },
    }
}

#[cfg(test)]
#[path = "config_runtime_tests.rs"]
mod tests;
