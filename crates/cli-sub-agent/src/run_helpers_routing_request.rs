//! RoutingRequest struct for tool and model routing parameters.

use std::path::Path;

use csa_config::ProjectConfig;
use csa_core::types::ToolName;

/// Request struct for tool and model routing.
///
/// Encapsulates the parameters for `resolve_tool_and_model` to avoid
/// a function with 12 positional parameters.
pub(crate) struct RoutingRequest<'a> {
    pub tool: Option<ToolName>,
    pub model_spec: Option<&'a str>,
    pub model: Option<&'a str>,
    pub thinking: Option<&'a str>,
    pub config: Option<&'a ProjectConfig>,
    pub project_root: &'a Path,
    pub force: bool,
    pub force_override_user_config: bool,
    pub needs_edit: bool,
    pub tier: Option<&'a str>,
    pub force_ignore_tier_setting: bool,
    pub tool_is_auto_resolved: bool,
}

impl<'a> RoutingRequest<'a> {
    /// Create a new routing request with the given project root and defaults for all other fields.
    ///
    /// Tests can use `RoutingRequest::new(Path::new("/tmp"))` as the base and override specific fields.
    pub fn new(project_root: &'a Path) -> Self {
        Self {
            tool: None,
            model_spec: None,
            model: None,
            thinking: None,
            config: None,
            project_root,
            force: false,
            force_override_user_config: false,
            needs_edit: false,
            tier: None,
            force_ignore_tier_setting: false,
            tool_is_auto_resolved: false,
        }
    }
}
