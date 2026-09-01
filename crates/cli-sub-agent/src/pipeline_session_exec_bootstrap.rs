use std::path::Path;

use anyhow::{Context, Result};
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::ToolName;
use csa_session::{
    MetaSessionState, PhaseEvent, SessionPhase, compute_cooldown_wait, create_session,
    create_session_fresh,
};
use tracing::{info, warn};

use crate::pipeline::{ParentSessionSource, SessionCreationMode};
use crate::run_helpers::truncate_prompt;
use crate::startup_env::StartupSubtreeEnv;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(test)]
pub(super) enum BootstrapPlan {
    Legacy,
    CleanRoom,
}

#[cfg(test)]
impl BootstrapPlan {
    pub(super) const fn effect_names(self) -> &'static [&'static str] {
        match self {
            Self::Legacy => &[
                "setup-recovery",
                "resume-or-create",
                "parent-lineage",
                "continuation-recall",
            ],
            Self::CleanRoom => &[
                "symlink-preflight",
                "fresh-session",
                "budget-init",
                "state-cap",
                "resource-admission",
                "slot-reservation",
            ],
        }
    }
}

pub(super) struct SessionBootstrap {
    pub(super) session: MetaSessionState,
    pub(super) resolved_provider_session_id: Option<String>,
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn bootstrap_session(
    tool: &ToolName,
    prompt: &str,
    session_arg: Option<&str>,
    fresh_spawn_preflight_override: bool,
    description: Option<String>,
    parent: Option<String>,
    project_root: &Path,
    config: Option<&ProjectConfig>,
    global_config: Option<&GlobalConfig>,
    task_type: Option<&str>,
    tier_name: Option<&str>,
    parent_session_source: ParentSessionSource,
    session_creation_mode: SessionCreationMode,
    startup_env: &StartupSubtreeEnv,
) -> Result<SessionBootstrap> {
    // Check for parent session violation: a child process must not operate on its own session
    if let Some(session_id) = session_arg
        && startup_env
            .session_id()
            .is_some_and(|env_session| env_session == session_id)
    {
        return Err(csa_core::error::AppError::ParentSessionViolation.into());
    }

    if session_arg.is_none() || fresh_spawn_preflight_override {
        let preflight_check_config = config
            .map(|cfg| &cfg.preflight.ai_config_symlink_check)
            .or_else(|| global_config.map(|cfg| &cfg.preflight.ai_config_symlink_check));
        if let Some(preflight_check_config) = preflight_check_config {
            crate::preflight_symlink::run_ai_config_symlink_check(
                project_root,
                preflight_check_config,
            )?;
        }
    }

    // Spawn background lefthook auto-install task (non-blocking, rate-limited).
    crate::pipeline::lefthook_auto_install::spawn_lefthook_setup_if_needed(project_root);
    // Spawn background review-gate auto-setup if configured (non-blocking, rate-limited).
    crate::setup_cmds::spawn_review_gate_setup_if_needed(project_root, global_config);

    let cd = crate::pipeline_env::resolve_cooldown_seconds(config);
    let depth = startup_env.current_depth();
    if let Some(wait) = compute_cooldown_wait(
        project_root,
        cd,
        &session_arg.map(str::to_string),
        &parent,
        depth,
    ) {
        info!("Cooldown: sleeping {wait:?} before new session");
        tokio::time::sleep(wait).await;
        tokio::time::sleep(wait).await;
    }

    let mut resolved_provider_session_id: Option<String> = None;
    let mut session = if let Some(session_id) = session_arg {
        let resolution =
            csa_session::resolve_resume_session(project_root, session_id, tool.as_str())?;
        resolved_provider_session_id = resolution.provider_session_id;
        if resolved_provider_session_id.is_some() {
            info!(
                session = %resolution.meta_session_id,
                tool = %tool,
                "Resolved provider session ID from state.toml"
            );
        }
        csa_session::load_session(project_root, &resolution.meta_session_id)?
    } else {
        // Auto-generate description from prompt when not provided
        let effective_description = description.or_else(|| Some(truncate_prompt(prompt, 80)));
        let parent_id = match parent_session_source {
            ParentSessionSource::ExplicitOrEnv => parent.or_else(|| {
                inherited_parent_session_id_for_new_session(startup_env).map(ToOwned::to_owned)
            }),
            ParentSessionSource::ExplicitOnly => parent,
        };
        let mut new_session = match session_creation_mode {
            SessionCreationMode::DaemonManaged => create_session(
                project_root,
                effective_description.as_deref(),
                parent_id.as_deref(),
                Some(tool.as_str()),
            )?,
            SessionCreationMode::FreshChild => create_session_fresh(
                project_root,
                effective_description.as_deref(),
                parent_id.as_deref(),
                Some(tool.as_str()),
            )?,
        };
        crate::recall_cmd::spawn_recall_record_if_needed(project_root, startup_env.current_depth());
        new_session.task_context = csa_session::TaskContext {
            task_type: task_type.map(|s| s.to_string()),
            tier_name: tier_name.map(|s| s.to_string()),
        };
        let tier_budget = tier_token_budget(config, tier_name);
        let max_turns = tier_max_turns(config, tier_name);
        let issue_budget = global_config.map(|cfg| cfg.budget.resolved_max_tokens_per_issue());
        let allocated_budget = match (tier_budget, issue_budget) {
            (Some(tier), Some(issue)) => Some(tier.min(issue)),
            (Some(tier), None) => Some(tier),
            (None, Some(issue)) => Some(issue),
            (None, None) => None,
        };
        if allocated_budget.is_some() || max_turns.is_some() {
            let allocated = allocated_budget.unwrap_or(u64::MAX);
            let mut budget = csa_session::state::TokenBudget::new(allocated);
            budget.max_turns = max_turns;
            new_session.token_budget = Some(budget);
            info!(
                session = %new_session.meta_session_id,
                allocated = allocated,
                tier_budget = ?tier_budget,
                issue_budget = ?issue_budget,
                max_turns = ?max_turns,
                "Initialized token budget"
            );
        }
        new_session
    };

    if session_arg.is_some() && session.phase == SessionPhase::Available {
        if let Err(e) = session.apply_phase_event(PhaseEvent::Resumed) {
            warn!(session = %session.meta_session_id, error = %e, "Skipping phase transition on resume");
        } else {
            csa_session::save_session(&session).with_context(|| {
                format!(
                    "failed to persist resumed Active phase for session {}",
                    session.meta_session_id
                )
            })?;
            info!(session = %session.meta_session_id, "Session resumed and marked Active");
        }
    }

    if session_arg.is_some()
        && let Some(wrapper_session_id) = startup_env.session_id()
        && std::env::var("CSA_DAEMON_SESSION_ID").ok().as_deref() == Some(wrapper_session_id)
        && wrapper_session_id != session.meta_session_id
    {
        // The alias makes target artifacts visible to wrapper waiters, so the
        // previous attempt's terminal result must be invalidated first.
        let target_session_dir =
            csa_session::get_session_dir(project_root, &session.meta_session_id)?;
        let stale_result_path = target_session_dir.join(csa_session::result::RESULT_FILE_NAME);
        match std::fs::remove_file(&stale_result_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to invalidate stale resume result before publishing alias: {}",
                        stale_result_path.display()
                    )
                });
            }
        }
        csa_session::write_resume_target(
            project_root,
            wrapper_session_id,
            &session.meta_session_id,
        )
        .with_context(|| {
            format!(
                "failed to persist resume wrapper alias {wrapper_session_id} -> {}",
                session.meta_session_id
            )
        })?;
        info!(
            wrapper_session = %wrapper_session_id,
            target_session = %session.meta_session_id,
            "Persisted resume wrapper target"
        );
    }

    Ok(SessionBootstrap {
        session,
        resolved_provider_session_id,
    })
}

pub(super) fn bootstrap_clean_room_session(
    tool: &ToolName,
    project_root: &Path,
    config: Option<&ProjectConfig>,
    global_config: Option<&GlobalConfig>,
    tier_name: Option<&str>,
) -> Result<SessionBootstrap> {
    let preflight_check_config = config
        .map(|cfg| &cfg.preflight.ai_config_symlink_check)
        .or_else(|| global_config.map(|cfg| &cfg.preflight.ai_config_symlink_check));
    if let Some(preflight_check_config) = preflight_check_config {
        crate::preflight_symlink::run_ai_config_symlink_check(
            project_root,
            preflight_check_config,
        )?;
    }
    let mut session = create_session_fresh(
        project_root,
        Some("clean-room execution"),
        None,
        Some(tool.as_str()),
    )?;
    session.task_context = csa_session::TaskContext {
        task_type: None,
        tier_name: tier_name.map(str::to_string),
    };
    let tier_budget = tier_token_budget(config, tier_name);
    let max_turns = tier_max_turns(config, tier_name);
    let issue_budget = global_config.map(|cfg| cfg.budget.resolved_max_tokens_per_issue());
    let allocated_budget = match (tier_budget, issue_budget) {
        (Some(tier), Some(issue)) => Some(tier.min(issue)),
        (Some(tier), None) => Some(tier),
        (None, Some(issue)) => Some(issue),
        (None, None) => None,
    };
    if allocated_budget.is_some() || max_turns.is_some() {
        let mut budget = csa_session::state::TokenBudget::new(allocated_budget.unwrap_or(u64::MAX));
        budget.max_turns = max_turns;
        session.token_budget = Some(budget);
    }
    csa_session::save_session(&session).context("persist fresh clean-room session")?;
    Ok(SessionBootstrap {
        session,
        resolved_provider_session_id: None,
    })
}

fn tier_token_budget(config: Option<&ProjectConfig>, tier_name: Option<&str>) -> Option<u64> {
    config
        .zip(tier_name)
        .and_then(|(cfg, tier)| cfg.tiers.get(tier))
        .and_then(|tier| tier.token_budget)
}

fn tier_max_turns(config: Option<&ProjectConfig>, tier_name: Option<&str>) -> Option<u32> {
    config
        .zip(tier_name)
        .and_then(|(cfg, tier)| cfg.tiers.get(tier))
        .and_then(|tier| tier.max_turns)
}

fn inherited_parent_session_id_for_new_session(startup_env: &StartupSubtreeEnv) -> Option<&str> {
    let inherited_session = startup_env.session_id()?;
    if std::env::var("CSA_DAEMON_SESSION_ID").ok().as_deref() == Some(inherited_session) {
        return startup_env.parent_session();
    }
    Some(inherited_session)
}

#[cfg(test)]
mod tests {
    use super::{bootstrap_session, inherited_parent_session_id_for_new_session};
    use crate::pipeline::{ParentSessionSource, SessionCreationMode};
    use crate::session_cmds_daemon::{
        WaitBehavior, WaitLoopTiming, WaitReconciliationOutcome, handle_session_wait_with_hooks,
    };
    use crate::startup_env::StartupSubtreeEnv;
    use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};
    use crate::test_session_sandbox::ScopedSessionSandbox;
    use csa_core::env::{
        CSA_PARENT_SESSION_ENV_KEY, CSA_SESSION_DIR_ENV_KEY, CSA_SESSION_ID_ENV_KEY,
    };
    use csa_core::types::ToolName;
    use std::collections::HashMap;

    #[test]
    fn inherited_parent_session_uses_session_id_for_foreground_nested_run() {
        let _env_lock = TEST_ENV_LOCK.blocking_lock();
        let _daemon = ScopedEnvVarRestore::unset("CSA_DAEMON_SESSION_ID");
        let startup_env = StartupSubtreeEnv::from_values(HashMap::from([(
            CSA_SESSION_ID_ENV_KEY,
            "01PARENT".to_string(),
        )]));

        assert_eq!(
            inherited_parent_session_id_for_new_session(&startup_env),
            Some("01PARENT")
        );
    }

    #[test]
    fn inherited_parent_session_uses_parent_for_daemon_child_run() {
        let _env_lock = TEST_ENV_LOCK.blocking_lock();
        let _daemon = ScopedEnvVarRestore::set("CSA_DAEMON_SESSION_ID", "01CHILD");
        let startup_env = StartupSubtreeEnv::from_values(HashMap::from([
            (CSA_SESSION_ID_ENV_KEY, "01PARENT".to_string()),
            (CSA_SESSION_DIR_ENV_KEY, "/repo/parent".to_string()),
        ]))
        .with_current_session("01CHILD", "/repo/child");

        assert_eq!(
            inherited_parent_session_id_for_new_session(&startup_env),
            Some("01PARENT")
        );
    }

    #[test]
    fn inherited_parent_session_returns_none_for_top_level_daemon_child() {
        let _env_lock = TEST_ENV_LOCK.blocking_lock();
        let _daemon = ScopedEnvVarRestore::set("CSA_DAEMON_SESSION_ID", "01CHILD");
        let startup_env = StartupSubtreeEnv::from_values(HashMap::from([(
            CSA_SESSION_ID_ENV_KEY,
            "01CHILD".to_string(),
        )]));

        assert_eq!(
            inherited_parent_session_id_for_new_session(&startup_env),
            None
        );
    }

    #[test]
    fn inherited_parent_session_preserves_explicit_parent_snapshot_for_daemon_child() {
        let _env_lock = TEST_ENV_LOCK.blocking_lock();
        let _daemon = ScopedEnvVarRestore::set("CSA_DAEMON_SESSION_ID", "01CHILD");
        let startup_env = StartupSubtreeEnv::from_values(HashMap::from([
            (CSA_SESSION_ID_ENV_KEY, "01CHILD".to_string()),
            (CSA_PARENT_SESSION_ENV_KEY, "01PARENT".to_string()),
        ]));

        assert_eq!(
            inherited_parent_session_id_for_new_session(&startup_env),
            Some("01PARENT")
        );
    }

    #[tokio::test]
    async fn resume_alias_does_not_expose_previous_attempt_config_failure() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _sandbox = ScopedSessionSandbox::new(&temp).await;
        let project = temp.path();
        let target =
            csa_session::create_session_fresh(project, Some("resume target"), None, Some("codex"))
                .expect("create target");
        let target_id = target.meta_session_id.clone();
        let target_dir = csa_session::get_session_dir(project, &target_id).expect("target dir");
        let previous_result = csa_session::SessionResult {
            status: "failure".to_string(),
            exit_code: 1,
            summary: "Error loading config.toml: previous attempt".to_string(),
            tool: "codex".to_string(),
            ..Default::default()
        };
        csa_session::save_result(project, &target_id, &previous_result)
            .expect("save previous result");
        let _diagnostic_lock =
            csa_lock::acquire_lock(&target_dir, "codex", "previous attempt diagnostic")
                .expect("acquire live diagnostic lock");

        let wrapper =
            csa_session::create_session_fresh(project, Some("resume wrapper"), None, Some("codex"))
                .expect("create wrapper");
        let wrapper_id = wrapper.meta_session_id;
        // SAFETY: ScopedSessionSandbox owns TEST_ENV_LOCK for the test lifetime.
        unsafe { std::env::set_var("CSA_DAEMON_SESSION_ID", &wrapper_id) };
        let startup_env = StartupSubtreeEnv::from_values(HashMap::from([(
            CSA_SESSION_ID_ENV_KEY,
            wrapper_id.clone(),
        )]));

        bootstrap_session(
            &ToolName::Codex,
            "resume target",
            Some(&target_id),
            false,
            None,
            None,
            project,
            None,
            None,
            Some("run"),
            None,
            ParentSessionSource::ExplicitOrEnv,
            SessionCreationMode::DaemonManaged,
            &startup_env,
        )
        .await
        .expect("bootstrap resumed target");
        assert!(
            !target_dir.join("result.toml").exists(),
            "alias publication must invalidate the previous result first"
        );
        let mut completion = None;
        let wait_behavior = WaitBehavior {
            wait_timeout_secs: 0,
            memory_warn_mb: None,
            timing: WaitLoopTiming {
                poll_interval: std::time::Duration::from_millis(1),
                memory_sample_interval: std::time::Duration::from_secs(15),
            },
        };
        let first_exit = handle_session_wait_with_hooks(
            wrapper_id.clone(),
            Some(project.to_string_lossy().into_owned()),
            wait_behavior,
            |_project_root, _current_session_id, _trigger| {
                Ok(WaitReconciliationOutcome {
                    result_became_available: false,
                    synthetic: false,
                })
            },
            |_sid: &str, status: &str, exit_code, _synthetic, _mirror_to_stdout| {
                completion = Some((status.to_string(), exit_code));
            },
        )
        .expect("wait before current result");
        assert_eq!(first_exit, 0);
        assert_eq!(completion, None, "wait must reject the previous result");

        let current_result = csa_session::SessionResult {
            summary: "Error loading config.toml: current attempt".to_string(),
            ..previous_result
        };
        csa_session::save_result(project, &target_id, &current_result)
            .expect("save current result");

        let second_exit = handle_session_wait_with_hooks(
            wrapper_id,
            Some(project.to_string_lossy().into_owned()),
            wait_behavior,
            |_project_root, _current_session_id, _trigger| {
                panic!("current config failure must not reconcile")
            },
            |_sid: &str, status: &str, exit_code, _synthetic, _mirror_to_stdout| {
                completion = Some((status.to_string(), exit_code));
            },
        )
        .expect("wait should accept the current attempt result");
        assert_eq!(second_exit, 1);
        assert_eq!(completion, Some(("failure".to_string(), 1)));
        assert_eq!(
            csa_session::load_result(project, &target_id)
                .expect("load result")
                .expect("current result")
                .summary,
            "Error loading config.toml: current attempt"
        );
    }
}
