use std::path::Path;

use csa_config::{GlobalConfig, ProjectConfig};

pub(crate) const BRANCH_GUARD_EXIT_CODE: i32 = 2;

const HARDCODED_PROTECTED_BRANCHES: &[&str] = &["main", "master", "dev", "develop"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BranchGuardBypassSource {
    CliFlag,
    TrustedConfig,
    ReadOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum VcsBranchState {
    OnBranch {
        current: String,
        detected_default: Option<String>,
    },
    Indeterminate {
        current: Option<String>,
        detected_default: Option<String>,
        reason: String,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct BranchGuardRequest {
    pub(crate) cli_bypass: bool,
    pub(crate) trusted_config_bypass: bool,
    pub(crate) project_config_requested_bypass: bool,
    pub(crate) read_only_mode: bool,
    pub(crate) branch_state: VcsBranchState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BranchGuardDecision {
    Allow {
        source: Option<BranchGuardBypassSource>,
    },
    Refuse(BranchGuardRefusal),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BranchGuardRefusal {
    pub(crate) current_branch: Option<String>,
    pub(crate) detected_default: Option<String>,
    pub(crate) reason: String,
    pub(crate) bypass_source: String,
}

impl BranchGuardRefusal {
    pub(crate) fn render_stderr(&self) -> String {
        format!(
            "csa run: refusing to run on protected branch {}\n\
             detected default branch: {}\n\
             reason: {}\n\
             recommend: git checkout -b feat/<your-feature> && csa run ...\n\
             escape hatch: pass --allow-base-branch-commit to bypass (and accept risk)\n\
             bypass source: {}",
            safe_display_option(self.current_branch.as_deref()),
            safe_display_option(self.detected_default.as_deref()),
            self.reason.escape_default(),
            self.bypass_source,
        )
    }
}

#[derive(Debug, Clone)]
pub(crate) struct BranchGuardRuntime {
    pub(crate) cli_bypass: bool,
    pub(crate) trusted_config_bypass: bool,
    pub(crate) project_config_requested_bypass: bool,
    pub(crate) read_only_mode: bool,
}

impl BranchGuardRuntime {
    pub(crate) fn for_run(
        cli_bypass: bool,
        project_config: Option<&ProjectConfig>,
        global_config: &GlobalConfig,
    ) -> Self {
        Self::from_sources(cli_bypass, project_config, global_config, false)
    }

    pub(crate) fn from_sources(
        cli_bypass: bool,
        project_config: Option<&ProjectConfig>,
        global_config: &GlobalConfig,
        read_only_mode: bool,
    ) -> Self {
        let trusted_config_bypass = global_config.run.allow_base_branch_commit;
        let project_config_requested_bypass = project_config
            .is_some_and(|config| config.run.allow_base_branch_commit)
            && !trusted_config_bypass;
        Self {
            cli_bypass,
            trusted_config_bypass,
            project_config_requested_bypass,
            read_only_mode,
        }
    }

    pub(crate) fn request(&self, branch_state: VcsBranchState) -> BranchGuardRequest {
        BranchGuardRequest {
            cli_bypass: self.cli_bypass,
            trusted_config_bypass: self.trusted_config_bypass,
            project_config_requested_bypass: self.project_config_requested_bypass,
            read_only_mode: self.read_only_mode,
            branch_state,
        }
    }
}

pub(crate) fn evaluate_and_emit_refusal(
    runtime: &BranchGuardRuntime,
    branch_state: VcsBranchState,
) -> Option<i32> {
    if let BranchGuardDecision::Refuse(refusal) =
        evaluate_branch_guard(runtime.request(branch_state))
    {
        emit_branch_guard_refusal(&refusal);
        return Some(BRANCH_GUARD_EXIT_CODE);
    }
    None
}

pub(crate) fn evaluate_branch_guard(request: BranchGuardRequest) -> BranchGuardDecision {
    if request.cli_bypass {
        return BranchGuardDecision::Allow {
            source: Some(BranchGuardBypassSource::CliFlag),
        };
    }
    if request.trusted_config_bypass {
        return BranchGuardDecision::Allow {
            source: Some(BranchGuardBypassSource::TrustedConfig),
        };
    }
    if request.read_only_mode {
        return BranchGuardDecision::Allow {
            source: Some(BranchGuardBypassSource::ReadOnly),
        };
    }

    match request.branch_state {
        VcsBranchState::OnBranch {
            current,
            detected_default,
        } if is_protected_branch(&current, detected_default.as_deref()) => {
            BranchGuardDecision::Refuse(BranchGuardRefusal {
                current_branch: Some(current),
                detected_default,
                reason: "protected branch matched hardcoded base branch or detected default branch"
                    .to_string(),
                bypass_source: bypass_source_diagnostic(request.project_config_requested_bypass),
            })
        }
        VcsBranchState::OnBranch { .. } => BranchGuardDecision::Allow { source: None },
        VcsBranchState::Indeterminate {
            current,
            detected_default,
            reason,
        } => BranchGuardDecision::Refuse(BranchGuardRefusal {
            current_branch: current,
            detected_default,
            reason,
            bypass_source: bypass_source_diagnostic(request.project_config_requested_bypass),
        }),
    }
}

pub(crate) fn observe_branch_state(
    project_root: &Path,
    project_config: Option<&ProjectConfig>,
) -> VcsBranchState {
    let vcs_config = project_config.map(|config| &config.vcs);
    let backend = csa_session::vcs_backends::create_vcs_backend_with_config(
        project_root,
        vcs_config.and_then(|config| config.backend),
        vcs_config.and_then(|config| config.colocated_default),
    );
    let current = match backend.current_branch(project_root) {
        Ok(current) => current,
        Err(err) => {
            return VcsBranchState::Indeterminate {
                current: None,
                detected_default: None,
                reason: format!("current branch probe failed: {err}"),
            };
        }
    };
    let detected_default = match backend.default_branch(project_root) {
        Ok(default_branch) => default_branch,
        Err(err) => {
            return VcsBranchState::Indeterminate {
                current,
                detected_default: None,
                reason: format!("default branch probe failed: {err}"),
            };
        }
    };

    match current {
        Some(current) => VcsBranchState::OnBranch {
            current,
            detected_default,
        },
        None => VcsBranchState::Indeterminate {
            current: None,
            detected_default,
            reason: "current branch is unknown or detached".to_string(),
        },
    }
}

pub(crate) fn emit_branch_guard_refusal(refusal: &BranchGuardRefusal) {
    eprintln!("{}", refusal.render_stderr());
}

fn is_protected_branch(current: &str, detected_default: Option<&str>) -> bool {
    HARDCODED_PROTECTED_BRANCHES.contains(&current) || detected_default == Some(current)
}

fn bypass_source_diagnostic(project_config_requested_bypass: bool) -> String {
    if project_config_requested_bypass {
        "none (project-local allow_base_branch_commit is not trusted)".to_string()
    } else {
        "none".to_string()
    }
}

fn safe_display_option(value: Option<&str>) -> String {
    value
        .map(|value| format!("{value:?}"))
        .unwrap_or_else(|| "(unknown)".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(branch_state: VcsBranchState) -> BranchGuardRequest {
        BranchGuardRequest {
            cli_bypass: false,
            trusted_config_bypass: false,
            project_config_requested_bypass: false,
            read_only_mode: false,
            branch_state,
        }
    }

    #[test]
    fn root_refused_on_hardcoded_protected_branch() {
        let decision = evaluate_branch_guard(request(VcsBranchState::OnBranch {
            current: "main".to_string(),
            detected_default: Some("trunk".to_string()),
        }));
        assert!(matches!(decision, BranchGuardDecision::Refuse(_)));
    }

    #[test]
    fn detected_default_adds_to_hardcoded_protected_set() {
        let decision = evaluate_branch_guard(request(VcsBranchState::OnBranch {
            current: "release".to_string(),
            detected_default: Some("release".to_string()),
        }));
        assert!(matches!(decision, BranchGuardDecision::Refuse(_)));
    }

    #[test]
    fn feature_branch_allowed_when_not_detected_default() {
        let decision = evaluate_branch_guard(request(VcsBranchState::OnBranch {
            current: "feat/work".to_string(),
            detected_default: Some("main".to_string()),
        }));
        assert_eq!(decision, BranchGuardDecision::Allow { source: None });
    }

    #[test]
    fn cli_bypass_has_highest_precedence() {
        let mut req = request(VcsBranchState::OnBranch {
            current: "main".to_string(),
            detected_default: Some("main".to_string()),
        });
        req.cli_bypass = true;
        assert_eq!(
            evaluate_branch_guard(req),
            BranchGuardDecision::Allow {
                source: Some(BranchGuardBypassSource::CliFlag)
            }
        );
    }

    #[test]
    fn trusted_config_bypass_allows_protected_branch() {
        let mut req = request(VcsBranchState::OnBranch {
            current: "dev".to_string(),
            detected_default: Some("main".to_string()),
        });
        req.trusted_config_bypass = true;
        assert_eq!(
            evaluate_branch_guard(req),
            BranchGuardDecision::Allow {
                source: Some(BranchGuardBypassSource::TrustedConfig)
            }
        );
    }

    #[test]
    fn project_config_bypass_is_rejected() {
        let mut req = request(VcsBranchState::OnBranch {
            current: "main".to_string(),
            detected_default: Some("main".to_string()),
        });
        req.project_config_requested_bypass = true;
        let decision = evaluate_branch_guard(req);
        let BranchGuardDecision::Refuse(refusal) = decision else {
            panic!("project-local bypass must not allow protected branch");
        };
        assert!(refusal.bypass_source.contains("project-local"));
    }

    #[test]
    fn child_session_env_does_not_bypass_protected_branch_guard() {
        let spoofed = request(VcsBranchState::OnBranch {
            current: "main".to_string(),
            detected_default: Some("main".to_string()),
        });
        assert!(matches!(
            evaluate_branch_guard(spoofed),
            BranchGuardDecision::Refuse(_)
        ));
    }

    #[test]
    fn proven_read_only_mode_allows_but_ephemeral_is_not_an_input() {
        let mut req = request(VcsBranchState::OnBranch {
            current: "master".to_string(),
            detected_default: Some("master".to_string()),
        });
        req.read_only_mode = true;
        assert_eq!(
            evaluate_branch_guard(req),
            BranchGuardDecision::Allow {
                source: Some(BranchGuardBypassSource::ReadOnly)
            }
        );
    }

    #[test]
    fn detached_or_unknown_branch_fails_closed() {
        let decision = evaluate_branch_guard(request(VcsBranchState::Indeterminate {
            current: None,
            detected_default: Some("main".to_string()),
            reason: "current branch is unknown or detached".to_string(),
        }));
        assert!(matches!(decision, BranchGuardDecision::Refuse(_)));
    }

    #[test]
    fn repeated_pre_spawn_decision_refuses_after_branch_changes_to_protected() {
        let first = evaluate_branch_guard(request(VcsBranchState::OnBranch {
            current: "feat/work".to_string(),
            detected_default: Some("main".to_string()),
        }));
        assert_eq!(first, BranchGuardDecision::Allow { source: None });

        let second = evaluate_branch_guard(request(VcsBranchState::OnBranch {
            current: "main".to_string(),
            detected_default: Some("main".to_string()),
        }));
        assert!(matches!(second, BranchGuardDecision::Refuse(_)));
    }

    #[test]
    fn refusal_escapes_control_characters_in_branch_names() {
        let refusal = BranchGuardRefusal {
            current_branch: Some("main\n\u{1b}[31mspoof".to_string()),
            detected_default: Some("main\rdefault".to_string()),
            reason: "protected".to_string(),
            bypass_source: "none".to_string(),
        };
        let rendered = refusal.render_stderr();
        assert!(!rendered.contains("\u{1b}[31m"));
        assert!(!rendered.contains("main\n"));
        assert!(rendered.contains("\\n"));
        assert!(rendered.contains("\\u{1b}"));
    }
}
