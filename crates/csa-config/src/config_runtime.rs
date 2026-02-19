use crate::config::{EnforcementMode, ProjectConfig};

impl ProjectConfig {
    /// Resolve sandbox enforcement mode (defaults to `Off`).
    pub fn enforcement_mode(&self) -> EnforcementMode {
        self.resources
            .enforcement_mode
            .unwrap_or(EnforcementMode::Off)
    }

    /// Resolve memory_max_mb: tool-level override > project resources > None.
    pub fn sandbox_memory_max_mb(&self, tool: &str) -> Option<u64> {
        self.tools
            .get(tool)
            .and_then(|t| t.memory_max_mb)
            .or(self.resources.memory_max_mb)
    }

    /// Resolve memory_swap_max_mb: tool-level override > project resources > None.
    pub fn sandbox_memory_swap_max_mb(&self, tool: &str) -> Option<u64> {
        self.tools
            .get(tool)
            .and_then(|t| t.memory_swap_max_mb)
            .or(self.resources.memory_swap_max_mb)
    }

    /// Resolve node_heap_limit_mb: tool-level override > project resources > None.
    pub fn sandbox_node_heap_limit_mb(&self, tool: &str) -> Option<u64> {
        self.tools
            .get(tool)
            .and_then(|t| t.node_heap_limit_mb)
            .or(self.resources.node_heap_limit_mb)
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

    /// Resolve lean_mode for a tool (defaults to false).
    pub fn tool_lean_mode(&self, tool: &str) -> bool {
        self.tools
            .get(tool)
            .and_then(|t| t.lean_mode)
            .unwrap_or(false)
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
