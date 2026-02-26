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
    /// Priority: tool-level override > project resources > inherent profile default > None.
    ///
    /// Uses the inherent (runtime-based) profile for fallback so that a tool
    /// resolving to Custom (e.g. only enforcement_mode set) still inherits
    /// Heavyweight memory defaults.
    pub fn sandbox_memory_max_mb(&self, tool: &str) -> Option<u64> {
        self.tools
            .get(tool)
            .and_then(|t| t.memory_max_mb)
            .or(self.resources.memory_max_mb)
            .or_else(|| profile_defaults(default_profile(tool)).memory_max_mb)
    }

    /// Resolve memory_swap_max_mb for a tool.
    ///
    /// Priority: tool-level override > project resources > inherent profile default > None.
    ///
    /// Uses the inherent (runtime-based) profile for fallback (same rationale
    /// as `sandbox_memory_max_mb`).
    pub fn sandbox_memory_swap_max_mb(&self, tool: &str) -> Option<u64> {
        self.tools
            .get(tool)
            .and_then(|t| t.memory_swap_max_mb)
            .or(self.resources.memory_swap_max_mb)
            .or_else(|| profile_defaults(default_profile(tool)).memory_swap_max_mb)
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
    ///
    /// Deprecated: prefer [`tool_setting_sources`] which provides finer control.
    pub fn tool_lean_mode(&self, tool: &str) -> bool {
        self.tools
            .get(tool)
            .and_then(|t| t.lean_mode)
            .unwrap_or_else(|| matches!(default_profile(tool), ToolResourceProfile::Heavyweight))
    }

    /// Resolve setting_sources for a tool.
    ///
    /// Priority: `setting_sources` (explicit) > `lean_mode` (deprecated compat) > profile default.
    /// - `Some(sources)` → pass `settingSources: sources` in ACP session meta.
    /// - `None` → no override (load everything).
    ///
    /// Heavyweight tools default to `Some(vec![])` (lean/load nothing).
    /// Lightweight tools default to `None` (load everything).
    pub fn tool_setting_sources(&self, tool: &str) -> Option<Vec<String>> {
        if let Some(tc) = self.tools.get(tool) {
            // Explicit setting_sources takes priority.
            if let Some(ref sources) = tc.setting_sources {
                return Some(sources.clone());
            }
            // Deprecated lean_mode fallback.
            if let Some(lean) = tc.lean_mode {
                eprintln!(
                    "warning: config: tool '{tool}': 'lean_mode' is deprecated; \
                     use 'setting_sources' instead"
                );
                return if lean { Some(vec![]) } else { None };
            }
        }
        // Profile-based default: Heavyweight → lean (empty sources), Lightweight → None.
        if matches!(default_profile(tool), ToolResourceProfile::Heavyweight) {
            Some(vec![])
        } else {
            None
        }
    }

    /// Check if a tool is allowed to edit existing files.
    pub fn can_tool_edit_existing(&self, tool: &str) -> bool {
        self.tools
            .get(tool)
            .and_then(|t| t.restrictions.as_ref())
            .map(|r| r.allow_edit_existing_files)
            .unwrap_or(true)
    }

    /// Resolve Codex PTY fork trust behavior from tool config.
    ///
    /// Returns false by default.
    pub fn codex_auto_trust(&self) -> bool {
        self.tools
            .get("codex")
            .map(|cfg| cfg.codex_auto_trust)
            .unwrap_or(false)
    }
}

/// Default sandbox options derived from a tool's resource profile.
/// Used when no ProjectConfig is available (e.g., no .csa/config.toml).
#[derive(Debug, Clone)]
pub struct DefaultSandboxOptions {
    pub enforcement: EnforcementMode,
    pub memory_max_mb: Option<u64>,
    pub memory_swap_max_mb: Option<u64>,
    /// Selective MCP/setting sources for ACP session meta.
    /// `Some(vec![])` = lean mode (load nothing). `None` = load everything.
    pub setting_sources: Option<Vec<String>>,
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
        setting_sources: if matches!(profile, ToolResourceProfile::Heavyweight) {
            Some(vec![])
        } else {
            None
        },
        node_heap_limit_mb: match profile {
            ToolResourceProfile::Heavyweight => Some(2048),
            _ => None,
        },
    }
}

#[cfg(test)]
#[path = "config_runtime_tests.rs"]
mod tests;
