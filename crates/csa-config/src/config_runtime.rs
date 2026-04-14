use std::path::PathBuf;

use crate::config::{EnforcementMode, ProjectConfig, ToolResourceProfile, ToolTransport};

/// Known tool-to-profile mapping based on runtime characteristics.
///
/// Lightweight: native binaries with predictable memory (Rust, Go).
/// Heavyweight: Node.js runtimes with dynamic plugin/MCP loading.
///
/// Note: codex uses codex-acp (Node.js) as its backend process, which can
/// consume 5+ GB under load. It must be classified as Heavyweight to ensure
/// cgroup memory limits are applied by default. See: OOM incident 2026-03-13
/// where codex+codex-acp reached 21 GB with EnforcementMode::Off.
fn default_profile(tool: &str) -> ToolResourceProfile {
    match tool {
        "opencode" => ToolResourceProfile::Lightweight,
        "claude-code" | "codex" | "gemini-cli" => ToolResourceProfile::Heavyweight,
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
            // 4096 MB: Node.js tools (claude-code, codex) need headroom for
            // the runtime + subprocesses (cargo, npm) in large workspaces.
            // The previous 2048 MB default caused OOM kills in monorepos.
            // See: GitHub issue #508.
            memory_max_mb: Some(4096),
            memory_swap_max_mb: Some(0),
        },
        ToolResourceProfile::Custom => ProfileDefaults {
            enforcement: EnforcementMode::Off,
            memory_max_mb: None,
            memory_swap_max_mb: None,
        },
    }
}

fn default_memory_max_mb_for_tool(tool: &str) -> Option<u64> {
    match tool {
        "gemini-cli" => {
            // Gemini CLI workloads are highly variable. A hard 2GB default often
            // fails in real projects before useful output is produced.
            None
        }
        "codex" => {
            // Codex uses codex-acp (Node.js) as backend which alone can consume
            // 5+ GB. When the tool also drives Rust compilation (cargo, rustc,
            // proc-macro expansion), 4096 MB is insufficient and 8192 MB still
            // OOMs in large workspaces. 12288 MB (12 GB) provides headroom for
            // Node.js runtime + full Rust compilation toolchain.
            // See: GitHub issue #555.
            Some(12288)
        }
        _ => profile_defaults(default_profile(tool)).memory_max_mb,
    }
}

fn default_memory_swap_max_mb_for_tool(tool: &str) -> Option<u64> {
    if tool == "gemini-cli" {
        return None;
    }
    profile_defaults(default_profile(tool)).memory_swap_max_mb
}

fn default_node_heap_limit_mb_for_tool(tool: &str) -> Option<u64> {
    if tool == "gemini-cli" {
        // Let gemini-cli decide Node heap sizing unless user explicitly pins it.
        return None;
    }
    match default_profile(tool) {
        ToolResourceProfile::Heavyweight => Some(2048),
        _ => None,
    }
}

impl ProjectConfig {
    /// Resolve the resource profile for a tool.
    ///
    /// If the tool has any explicit resource override (enforcement_mode,
    /// memory_max_mb, or memory_swap_max_mb), returns `Custom`.
    /// Otherwise returns the default profile for the tool's runtime.
    pub fn tool_resource_profile(&self, tool: &str) -> ToolResourceProfile {
        if let Some(tc) = self.tools.get(tool)
            && (tc.enforcement_mode.is_some()
                || tc.memory_max_mb.is_some()
                || tc.memory_swap_max_mb.is_some())
        {
            return ToolResourceProfile::Custom;
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
    ///
    /// Safety net: if the resolved mode is `Off` (from profile default, not user
    /// explicit) but the user set `memory_max_mb`, auto-promote to `BestEffort`
    /// to prevent silent limit bypass. See: serde(default) trap (rule rust/016).
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
        let profile_mode = profile_defaults(default_profile(tool)).enforcement;

        // 4. Safety net: auto-promote Off → BestEffort when user set memory limits
        //    but didn't explicitly set enforcement_mode. Prevents silent limit bypass.
        if matches!(profile_mode, EnforcementMode::Off) {
            let has_user_memory_limit = self
                .tools
                .get(tool)
                .is_some_and(|t| t.memory_max_mb.is_some() || t.memory_swap_max_mb.is_some());
            if has_user_memory_limit {
                tracing::warn!(
                    tool,
                    "Auto-promoting enforcement_mode Off → BestEffort: \
                     memory limits are set but enforcement_mode was not explicitly configured. \
                     Set enforcement_mode explicitly to suppress this warning."
                );
                return EnforcementMode::BestEffort;
            }
        }

        profile_mode
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
            .or_else(|| default_memory_max_mb_for_tool(tool))
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
            .or_else(|| default_memory_swap_max_mb_for_tool(tool))
    }

    /// Resolve node_heap_limit_mb: tool-level override > project resources > profile default > None.
    pub fn sandbox_node_heap_limit_mb(&self, tool: &str) -> Option<u64> {
        self.tools
            .get(tool)
            .and_then(|t| t.node_heap_limit_mb)
            .or(self.resources.node_heap_limit_mb)
            .or_else(|| default_node_heap_limit_mb_for_tool(tool))
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

    /// Resolve the per-tool default model override for explicit `--tool` runs.
    pub fn tool_default_model(&self, tool: &str) -> Option<&str> {
        self.tools
            .get(tool)
            .and_then(|t| t.default_model.as_deref())
    }

    /// Resolve the per-tool default thinking budget for explicit `--tool` runs.
    pub fn tool_default_thinking(&self, tool: &str) -> Option<&str> {
        self.tools
            .get(tool)
            .and_then(|t| t.default_thinking.as_deref())
    }

    /// Resolve the per-tool transport override.
    pub fn tool_transport(&self, tool: &str) -> Option<ToolTransport> {
        self.tools.get(tool).and_then(|t| t.transport)
    }

    /// Check if a tool is allowed to edit existing files.
    pub fn can_tool_edit_existing(&self, tool: &str) -> bool {
        self.tools
            .get(tool)
            .and_then(|t| t.restrictions.as_ref())
            .map(|r| r.allow_edit_existing_files)
            .unwrap_or(true)
    }

    /// Check if a tool is allowed to create new files.
    pub fn can_tool_write_new(&self, tool: &str) -> bool {
        self.tools
            .get(tool)
            .and_then(|t| t.restrictions.as_ref())
            .map(|r| r.allow_write_new_files)
            .unwrap_or(true)
    }

    /// Check if a tool is fully read-only (cannot edit existing or create new files).
    pub fn is_tool_read_only(&self, tool: &str) -> bool {
        !self.can_tool_edit_existing(tool) && !self.can_tool_write_new(tool)
    }

    /// Returns the writable paths for a tool's filesystem sandbox.
    ///
    /// Priority chain (REPLACE semantics):
    /// 1. Tool-level `writable_paths` from `[tools.<name>.filesystem_sandbox]` — if set, REPLACES project root
    /// 2. Legacy `tool_writable_overrides` from `[filesystem_sandbox]` — if set, REPLACES project root
    /// 3. Global `extra_writable` from `[filesystem_sandbox]` — always APPENDED
    /// 4. Default: `None` — caller should use project root as writable
    ///
    /// When tool-level paths are set (layer 1 or 2), the project root is NOT
    /// included as writable. Session dir and tool config dirs are always
    /// preserved by the `IsolationPlanBuilder` separately.
    pub fn sandbox_writable_paths(&self, tool: &str) -> Option<Vec<PathBuf>> {
        // Layer 1: new-style per-tool filesystem_sandbox config
        let tool_paths = self
            .tools
            .get(tool)
            .and_then(|t| t.filesystem_sandbox.as_ref())
            .and_then(|fs| fs.writable_paths.as_ref());

        if let Some(paths) = tool_paths {
            // REPLACE semantics: tool-level paths replace project root.
            // Global extra_writable is still appended.
            let mut result = paths.clone();
            result.extend(self.filesystem_sandbox.extra_writable.iter().cloned());
            return Some(result);
        }

        // Layer 2: legacy tool_writable_overrides
        if let Some(paths) = self.filesystem_sandbox.tool_writable_overrides.get(tool) {
            let mut result = paths.clone();
            result.extend(self.filesystem_sandbox.extra_writable.iter().cloned());
            return Some(result);
        }

        // Layer 3/4: no per-tool override — caller uses project root as writable.
        // If global extra_writable is set, return it so caller can append.
        None
    }

    /// Returns the filesystem sandbox enforcement mode for a tool.
    ///
    /// Priority chain:
    /// 1. Tool-level `[tools.<name>.filesystem_sandbox].enforcement_mode`
    /// 2. Global `[filesystem_sandbox].enforcement_mode`
    /// 3. Default: `None` (caller decides)
    ///
    /// Safety net: if tool-level FS sandbox section exists with `writable_paths`
    /// but enforcement_mode resolves to `"off"` or is absent, auto-promote to
    /// `"best-effort"` with a warning — configuring paths without enforcement
    /// is almost certainly a mistake.
    pub fn tool_fs_enforcement_mode(&self, tool: &str) -> Option<String> {
        let tool_fs = self
            .tools
            .get(tool)
            .and_then(|t| t.filesystem_sandbox.as_ref());

        // Layer 1: tool-level enforcement_mode
        if let Some(mode) = tool_fs.and_then(|fs| fs.enforcement_mode.as_ref()) {
            // Safety net: writable_paths configured but enforcement is "off"
            if mode == "off" && tool_fs.is_some_and(|fs| fs.writable_paths.is_some()) {
                tracing::warn!(
                    tool,
                    "Auto-promoting filesystem enforcement_mode 'off' → 'best-effort': \
                     writable_paths are configured but enforcement is off, which would \
                     make the paths meaningless. Set enforcement_mode = 'off' on the \
                     global [filesystem_sandbox] to suppress this warning."
                );
                return Some("best-effort".to_string());
            }
            return Some(mode.clone());
        }

        // Safety net: tool has writable_paths but no enforcement_mode at all
        if tool_fs.is_some_and(|fs| fs.writable_paths.is_some()) {
            // Check global enforcement before auto-promoting
            if let Some(ref global_mode) = self.filesystem_sandbox.enforcement_mode {
                return Some(global_mode.clone());
            }
            tracing::warn!(
                tool,
                "Auto-promoting filesystem enforcement_mode to 'best-effort': \
                 tool has writable_paths configured but no enforcement_mode set. \
                 Set enforcement_mode explicitly to suppress this warning."
            );
            return Some("best-effort".to_string());
        }

        // Layer 2: global enforcement_mode
        self.filesystem_sandbox.enforcement_mode.clone()
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
        memory_max_mb: default_memory_max_mb_for_tool(tool),
        memory_swap_max_mb: default_memory_swap_max_mb_for_tool(tool),
        setting_sources: if matches!(profile, ToolResourceProfile::Heavyweight) {
            Some(vec![])
        } else {
            None
        },
        node_heap_limit_mb: default_node_heap_limit_mb_for_tool(tool),
    }
}

#[cfg(test)]
#[path = "config_runtime_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "config_runtime_fs_sandbox_tests.rs"]
mod fs_sandbox_tests;
