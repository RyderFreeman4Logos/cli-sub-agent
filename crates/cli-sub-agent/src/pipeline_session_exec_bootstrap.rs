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
    use super::inherited_parent_session_id_for_new_session;
    use crate::startup_env::StartupSubtreeEnv;
    use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};
    use csa_core::env::{
        CSA_PARENT_SESSION_ENV_KEY, CSA_SESSION_DIR_ENV_KEY, CSA_SESSION_ID_ENV_KEY,
    };
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
}
