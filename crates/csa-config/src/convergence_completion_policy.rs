//! Safety policy for opt-in convergence completion execution.
//!
//! Completion can repair a checkout, run host commands, send provider input, and retain
//! evidence. The global configuration establishes the maximum authority available to a
//! project; project configuration may only remove authority, and the CLI must still make an
//! explicit execution request.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

/// Safety limits for convergence completion execution.
///
/// All permissions default to denied. A command needs an explicit CLI capability and every
/// required global and project policy permission before it may start completion work.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConvergenceCompletionPolicy {
    /// Permit completion to transition from reporting into execution.
    #[serde(default)]
    pub allow_execution: bool,
    /// Permit completion to send repository evidence to an admitted provider.
    #[serde(default)]
    pub allow_provider_egress: bool,
    /// Permit completion to run its fixed host command authority.
    #[serde(default)]
    pub allow_shell_commands: bool,
    /// Permit completion to pass explicitly admitted credentials to a provider process.
    #[serde(default)]
    pub allow_credential_inheritance: bool,
    /// Maximum number of days completion evidence may be retained. Zero forbids retention.
    #[serde(default)]
    pub max_retention_days: u16,
}

/// Optional project restrictions for convergence completion.
///
/// `None` means inherit the global ceiling for that one field; `Some(false)` or a lower
/// retention bound tightens it. A project file never grants authority by itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConvergenceCompletionPolicy {
    pub allow_execution: Option<bool>,
    pub allow_provider_egress: Option<bool>,
    pub allow_shell_commands: Option<bool>,
    pub allow_credential_inheritance: Option<bool>,
    pub max_retention_days: Option<u16>,
}

impl ConvergenceCompletionPolicy {
    /// Return whether every field is at its fail-closed default.
    #[must_use]
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }

    /// Resolve the effective safety policy.
    ///
    /// Precedence is intentionally not ordinary override precedence:
    /// `global ceiling ∩ project restriction ∩ explicit CLI capability`. Project configuration
    /// can only remove authority, and a CLI flag cannot restore authority denied by either
    /// configuration layer.
    #[must_use]
    pub fn effective(
        global: &Self,
        project: Option<&ProjectConvergenceCompletionPolicy>,
    ) -> EffectiveConvergenceCompletionPolicy {
        let project_allows = |selector: fn(&ProjectConvergenceCompletionPolicy) -> Option<bool>| {
            project.is_none_or(|policy| selector(policy).unwrap_or(true))
        };
        let max_retention_days = project.map_or(global.max_retention_days, |policy| {
            policy
                .max_retention_days
                .map_or(global.max_retention_days, |days| {
                    global.max_retention_days.min(days)
                })
        });
        EffectiveConvergenceCompletionPolicy {
            allow_execution: global.allow_execution
                && project_allows(|policy| policy.allow_execution),
            allow_provider_egress: global.allow_provider_egress
                && project_allows(|policy| policy.allow_provider_egress),
            allow_shell_commands: global.allow_shell_commands
                && project_allows(|policy| policy.allow_shell_commands),
            allow_credential_inheritance: global.allow_credential_inheritance
                && project_allows(|policy| policy.allow_credential_inheritance),
            max_retention_days,
        }
    }
}

/// Parse the optional policy table from raw project configuration.
///
/// # Errors
///
/// Returns an error for unknown fields or values outside the policy schema.
pub fn parse_project_convergence_completion_policy(
    raw: &toml::Value,
) -> Result<Option<ProjectConvergenceCompletionPolicy>> {
    raw.get("convergence_completion")
        .map(|value| value.clone().try_into())
        .transpose()
        .map_err(Into::into)
}

/// Fully resolved completion policy suitable for authorization evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EffectiveConvergenceCompletionPolicy {
    allow_execution: bool,
    allow_provider_egress: bool,
    allow_shell_commands: bool,
    allow_credential_inheritance: bool,
    max_retention_days: u16,
}

impl EffectiveConvergenceCompletionPolicy {
    /// Reject an execution attempt unless both policy layers and the explicit CLI capability
    /// grant every authority completion needs.
    ///
    /// # Errors
    ///
    /// Returns a deterministic explanation identifying the missing CLI capability or policy
    /// authority.
    pub fn require_explicit_execution(&self, execute_requested: bool) -> Result<()> {
        if !execute_requested {
            bail!(
                "completion report mode is read-only; rerun with --converge --execute-completion to request execution"
            );
        }
        if !self.allow_execution {
            bail!(
                "completion execution is denied by the effective [convergence_completion].allow_execution safety policy"
            );
        }
        if !self.allow_provider_egress {
            bail!(
                "completion execution is denied by the effective [convergence_completion].allow_provider_egress safety policy"
            );
        }
        if !self.allow_shell_commands {
            bail!(
                "completion execution is denied by the effective [convergence_completion].allow_shell_commands safety policy"
            );
        }
        if !self.allow_credential_inheritance {
            bail!(
                "completion execution is denied by the effective [convergence_completion].allow_credential_inheritance safety policy"
            );
        }
        if self.max_retention_days == 0 {
            bail!(
                "completion execution is denied by the effective [convergence_completion].max_retention_days safety policy"
            );
        }
        Ok(())
    }

    /// Return the effective retention bound for authorization evidence.
    #[must_use]
    pub fn max_retention_days(&self) -> u16 {
        self.max_retention_days
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn permitting_global_policy() -> ConvergenceCompletionPolicy {
        ConvergenceCompletionPolicy {
            allow_execution: true,
            allow_provider_egress: true,
            allow_shell_commands: true,
            allow_credential_inheritance: true,
            max_retention_days: 30,
        }
    }

    #[test]
    fn explicit_cli_capability_is_required_even_when_both_config_layers_permit_execution() {
        let effective = ConvergenceCompletionPolicy::effective(&permitting_global_policy(), None);
        let error = effective.require_explicit_execution(false).unwrap_err();
        assert!(error.to_string().contains("report mode is read-only"));
        effective.require_explicit_execution(true).unwrap();
    }

    #[test]
    fn project_policy_only_tightens_global_authority() {
        let project = ProjectConvergenceCompletionPolicy {
            allow_provider_egress: Some(false),
            max_retention_days: Some(7),
            ..Default::default()
        };
        let effective =
            ConvergenceCompletionPolicy::effective(&permitting_global_policy(), Some(&project));
        let error = effective.require_explicit_execution(true).unwrap_err();
        assert!(error.to_string().contains("allow_provider_egress"));
        assert_eq!(effective.max_retention_days(), 7);
    }
}
