//! Caller-confirmed single-finding fix path for failed `csa review` sessions.

use std::path::Path;

use anyhow::{Context, Result};
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::{ReviewDecision, ToolName};
use serde::Deserialize;

use crate::cli::ReviewArgs;
use crate::startup_env::StartupSubtreeEnv;

use super::output::sanitize_review_output;
use super::post_review::CONFIRM_THEN_FIX_FINDING_ACTION;

#[path = "review_cmd_fix_finding_prompt.rs"]
mod prompt;
use prompt::{build_fix_finding_prompt, resolve_fix_finding_prompt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FixFindingRoute {
    pub(super) session_id: String,
    pub(super) tool: ToolName,
    pub(super) model_spec: Option<String>,
    pub(super) model: Option<String>,
    pub(super) thinking: Option<String>,
    pub(super) provider_session_id: String,
}

#[derive(Debug, Deserialize)]
struct SuggestionEnvelope {
    suggestion: SuggestionFile,
}

#[derive(Debug, Deserialize)]
struct SuggestionFile {
    action: String,
    session_id: Option<String>,
    tool: Option<String>,
    model_spec: Option<String>,
    model: Option<String>,
    thinking: Option<String>,
    provider_session_id: Option<String>,
}

pub(crate) fn validate_fix_finding_before_daemon(args: &ReviewArgs) -> Result<()> {
    if !args.fix_finding {
        return Ok(());
    }

    let project_root = crate::pipeline::determine_project_root(args.cd.as_deref())?;
    let project_config = ProjectConfig::load(&project_root)?;
    let global_config = GlobalConfig::load()?;
    let session_ref = args
        .session
        .as_deref()
        .context("--fix-finding requires --session <failed-review-session-id>")?;
    let route = load_fix_finding_route(&project_root, session_ref)?;
    validate_fix_finding_route(
        &project_root,
        &route,
        project_config.as_ref(),
        &global_config,
    )?;
    Ok(())
}

pub(crate) async fn handle_fix_finding(
    args: ReviewArgs,
    project_root: &Path,
    current_depth: u32,
    startup_env: &StartupSubtreeEnv,
) -> Result<i32> {
    let Some((config, global_config)) =
        crate::pipeline::load_and_validate(project_root, current_depth)?
    else {
        return Ok(1);
    };
    let session_ref = args
        .session
        .as_deref()
        .context("--fix-finding requires --session <failed-review-session-id>")?;
    let route = load_fix_finding_route(project_root, session_ref)?;
    validate_fix_finding_route(project_root, &route, config.as_ref(), &global_config)?;

    let caller_prompt = resolve_fix_finding_prompt(&args)?;
    let prompt = build_fix_finding_prompt(&caller_prompt);
    let stream_mode =
        super::resolve::resolve_review_stream_mode(args.stream_stdout, args.no_stream_stdout);
    let idle_timeout_seconds = crate::pipeline::resolve_effective_idle_timeout_seconds(
        config.as_ref(),
        args.idle_timeout,
        args.timeout,
    );
    let initial_response_timeout_seconds =
        crate::pipeline::resolve_effective_initial_response_timeout_for_tool(
            config.as_ref(),
            args.initial_response_timeout,
            args.idle_timeout,
            args.timeout,
            route.tool.as_str(),
        );
    let review_routing =
        crate::review_routing::detect_review_routing_metadata(project_root, config.as_ref());

    let fix_future = super::execute::execute_review_with_tier_filter(
        route.tool,
        prompt,
        Some(route.session_id.clone()),
        route.model.clone(),
        route.model_spec.clone(),
        None,
        false,
        Vec::new(),
        route.thinking.clone(),
        format!("fix finding from review {}", route.session_id),
        project_root,
        config.as_ref(),
        &global_config,
        None,
        review_routing,
        stream_mode,
        idle_timeout_seconds,
        initial_response_timeout_seconds,
        args.force_override_user_config,
        true,
        true,
        None,
        args.build_jobs,
        args.fast_but_more_cost,
        false,
        args.no_fs_sandbox,
        false,
        &args.extra_writable,
        &args.extra_readable,
        args.error_marker_scan_override(),
        args.resource_overrides(),
        current_depth,
        startup_env,
    );

    let result = if let Some(timeout_secs) = args.timeout {
        match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), fix_future).await {
            Ok(inner) => inner?,
            Err(_) => {
                anyhow::bail!("Fix-finding pass aborted: --timeout {timeout_secs}s exceeded.");
            }
        }
    } else {
        fix_future.await?
    };

    print!(
        "{}",
        sanitize_review_output(&result.execution.execution.output)
    );
    println!(
        "<!-- CSA:CALLER_HINT action=\"review_next_round\" next_review=\"fresh_session\" \
         reason=\"fix-finding resumed reviewer session; the next review round must be independent\" -->"
    );
    Ok(result.execution.execution.exit_code)
}

pub(super) fn load_fix_finding_route(
    project_root: &Path,
    session_ref: &str,
) -> Result<FixFindingRoute> {
    let resolution = csa_session::resolve_fork_source(project_root, session_ref)
        .with_context(|| format!("failed to resolve --session {session_ref} for --fix-finding"))?;
    let session_id = resolution.meta_session_id;
    ensure_failed_review(project_root, &session_id)?;

    let session = csa_session::load_session(project_root, &session_id)
        .with_context(|| format!("failed to load review session {session_id}"))?;
    let session_dir = csa_session::get_session_dir(project_root, &session_id)
        .with_context(|| format!("failed to resolve review session dir for {session_id}"))?;
    let suggestion_path = session_dir.join("output").join("suggestion.toml");
    let suggestion = load_fix_finding_suggestion(&suggestion_path, &session_id)?;
    let tool = parse_suggestion_tool(&suggestion, &session_id)?;
    let provider_session_id = session
        .tools
        .get(tool.as_str())
        .and_then(|state| state.provider_session_id.as_deref())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Cannot run `csa review --fix-finding --session {session_id}`: \
                 state.toml has no provider_session_id for tool '{tool}'. \
                 Refusing to cold-start while claiming reviewer KV-cache reuse."
            )
        })?
        .to_string();

    if let Some(recorded_provider) = suggestion.provider_session_id.as_deref()
        && recorded_provider != provider_session_id
    {
        anyhow::bail!(
            "Cannot preserve exact review session for --fix-finding: \
             suggestion.toml recorded provider_session_id '{recorded_provider}', \
             but state.toml has '{provider_session_id}'."
        );
    }

    let route = FixFindingRoute {
        session_id,
        tool,
        model_spec: suggestion.model_spec,
        model: suggestion.model,
        thinking: suggestion.thinking,
        provider_session_id,
    };
    validate_route_has_exact_model(&route)?;
    Ok(route)
}

fn load_fix_finding_suggestion(path: &Path, session_id: &str) -> Result<SuggestionFile> {
    let contents = std::fs::read_to_string(path).with_context(|| {
        format!(
            "Cannot run --fix-finding for session {session_id}: missing exact-route sidecar {}. \
             Rerun `csa review` to emit a confirm_then_fix_finding suggestion, \
             or use legacy `csa review --session {session_id} --fix`.",
            path.display()
        )
    })?;
    let envelope: SuggestionEnvelope =
        toml::from_str(&contents).with_context(|| format!("failed to parse {}", path.display()))?;
    if envelope.suggestion.action != CONFIRM_THEN_FIX_FINDING_ACTION {
        anyhow::bail!(
            "Cannot run --fix-finding for session {session_id}: suggestion action is '{}', \
             expected '{}'. Rerun `csa review` to capture exact route metadata.",
            envelope.suggestion.action,
            CONFIRM_THEN_FIX_FINDING_ACTION
        );
    }
    if let Some(recorded_session_id) = envelope.suggestion.session_id.as_deref()
        && recorded_session_id != session_id
    {
        anyhow::bail!(
            "Cannot run --fix-finding: suggestion.toml session_id '{recorded_session_id}' \
             does not match resolved review session '{session_id}'."
        );
    }
    Ok(envelope.suggestion)
}

fn parse_suggestion_tool(suggestion: &SuggestionFile, session_id: &str) -> Result<ToolName> {
    let tool = suggestion.tool.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "Cannot run --fix-finding for session {session_id}: \
             suggestion.toml does not record the exact review tool."
        )
    })?;
    crate::run_helpers::parse_tool_name(tool).with_context(|| {
        format!("suggestion.toml for review session {session_id} records invalid tool '{tool}'")
    })
}

fn ensure_failed_review(project_root: &Path, session_id: &str) -> Result<()> {
    let Some(meta) = crate::review_consensus::review_iteration_resolver::load_review_meta(
        project_root,
        session_id,
    )?
    else {
        anyhow::bail!(
            "Cannot run --fix-finding for session {session_id}: review_meta.json is missing."
        );
    };
    let decision = meta
        .decision
        .parse::<ReviewDecision>()
        .unwrap_or(ReviewDecision::Uncertain);
    if decision != ReviewDecision::Fail {
        anyhow::bail!(
            "Cannot run --fix-finding for session {session_id}: review decision is '{}', not 'fail'.",
            meta.decision
        );
    }
    Ok(())
}

pub(super) fn validate_route_against_config(
    route: &FixFindingRoute,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
) -> Result<()> {
    validate_route_has_exact_model(route)?;
    let recorded_thinking = route_recorded_thinking(route)?;
    let tool_name = route.tool.as_str();
    if let Some(lock) = project_config
        .and_then(|config| config.thinking_lock(tool_name))
        .or_else(|| global_config.thinking_lock(tool_name))
        && Some(lock) != recorded_thinking.as_deref()
    {
        anyhow::bail!(
            "Cannot preserve exact review route for --fix-finding: \
             current thinking_lock for tool '{tool_name}' is '{lock}', \
             but the failed review route recorded thinking '{}'. \
             Remove or align the lock, then retry.",
            recorded_thinking.as_deref().unwrap_or("<missing>")
        );
    }
    Ok(())
}

fn validate_fix_finding_route(
    project_root: &Path,
    route: &FixFindingRoute,
    project_config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
) -> Result<()> {
    validate_route_against_config(route, project_config, global_config)?;
    if super::fix::should_skip_for_readonly_tool(
        project_config,
        route.tool,
        project_root,
        std::slice::from_ref(&route.session_id),
    ) {
        anyhow::bail!(
            "Cannot run --fix-finding for session {}: failed review route uses tool '{}', \
             but current project config sets allow_edit_existing_files=false for that tool. \
             Refusing to resume a write-intended fix pass.",
            route.session_id,
            route.tool
        );
    }
    Ok(())
}

fn validate_route_has_exact_model(route: &FixFindingRoute) -> Result<()> {
    if let Some(model_spec) = route.model_spec.as_deref() {
        validate_model_spec_matches_tool(model_spec, route.tool)?;
        return Ok(());
    }
    if route
        .model
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
        && route
            .thinking
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
    {
        return Ok(());
    }
    anyhow::bail!(
        "Cannot run --fix-finding for session {}: exact model/thinking route is missing. \
         Refusing to resume because a config default or fallback could change the reviewer route.",
        route.session_id
    )
}

fn validate_model_spec_matches_tool(model_spec: &str, tool: ToolName) -> Result<()> {
    let spec_tool = model_spec.split('/').next().unwrap_or_default();
    if spec_tool != tool.as_str() {
        anyhow::bail!(
            "Cannot preserve exact review route for --fix-finding: \
             suggestion tool is '{tool}', but model_spec selects '{spec_tool}'."
        );
    }
    if model_spec.split('/').count() != 4 {
        anyhow::bail!(
            "Cannot preserve exact review route for --fix-finding: \
             model_spec '{model_spec}' is not in tool/provider/model/thinking format."
        );
    }
    Ok(())
}

fn route_recorded_thinking(route: &FixFindingRoute) -> Result<Option<String>> {
    if let Some(model_spec) = route.model_spec.as_deref() {
        let thinking = model_spec
            .rsplit('/')
            .next()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("model_spec '{model_spec}' has no thinking segment"))?;
        return Ok(Some(thinking.to_string()));
    }
    Ok(route.thinking.clone())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::Path;

    use csa_config::{ProjectConfig, ProjectMeta, ResourcesConfig, ToolConfig, ToolRestrictions};
    use csa_core::types::ReviewDecision;
    use csa_session::{ReviewSessionMeta, ToolState, write_review_meta};

    use super::*;
    use crate::test_session_sandbox::ScopedSessionSandbox;

    fn project_config_with_codex() -> ProjectConfig {
        let mut tools = HashMap::new();
        for tool in csa_config::global::all_known_tools() {
            tools.insert(
                tool.as_str().to_string(),
                ToolConfig {
                    enabled: tool.as_str() == "codex",
                    restrictions: None,
                    suppress_notify: true,
                    ..Default::default()
                },
            );
        }
        ProjectConfig {
            schema_version: 1,
            project: ProjectMeta::default(),
            resources: ResourcesConfig {
                min_free_memory_mb: 1,
                ..Default::default()
            },
            acp: Default::default(),
            session: Default::default(),
            memory: Default::default(),
            tools,
            review: None,
            debate: None,
            tiers: HashMap::new(),
            tier_mapping: HashMap::new(),
            aliases: HashMap::new(),
            tool_aliases: HashMap::new(),
            preferences: None,
            github: None,
            hooks: Default::default(),
            run: Default::default(),
            execution: Default::default(),
            session_wait: None,
            preflight: Default::default(),
            vcs: Default::default(),
            filesystem_sandbox: Default::default(),
        }
    }

    fn failed_review_meta(session_id: &str) -> ReviewSessionMeta {
        ReviewSessionMeta {
            session_id: session_id.to_string(),
            head_sha: "HEAD".to_string(),
            decision: ReviewDecision::Fail.as_str().to_string(),
            verdict: "HAS_ISSUES".to_string(),
            review_mode: Some("standard".to_string()),
            status_reason: None,
            routed_to: Some("codex/openai/gpt-5.5/xhigh".to_string()),
            primary_failure: None,
            failure_reason: None,
            tool: "codex".to_string(),
            scope: "range:main...HEAD".to_string(),
            exit_code: 1,
            fix_attempted: false,
            fix_rounds: 0,
            review_iterations: 1,
            timestamp: chrono::Utc::now(),
            diff_fingerprint: None,
            fix_convergence: None,
        }
    }

    fn write_suggestion(project_root: &Path, session_id: &str, extra: &str) -> std::path::PathBuf {
        let session_dir = csa_session::get_session_dir(project_root, session_id).unwrap();
        let path = session_dir.join("output").join("suggestion.toml");
        std::fs::write(
            &path,
            format!(
                "[suggestion]\n\
                 action = \"confirm_then_fix_finding\"\n\
                 session_id = \"{session_id}\"\n\
                 tool = \"codex\"\n\
                 model_spec = \"codex/openai/gpt-5.5/xhigh\"\n\
                 {extra}"
            ),
        )
        .unwrap();
        path
    }

    fn create_failed_review_session(project_root: &Path, provider: Option<&str>) -> String {
        let mut session =
            csa_session::create_session(project_root, Some("failed review"), None, Some("codex"))
                .unwrap();
        if let Some(provider) = provider {
            session.tools.insert(
                "codex".to_string(),
                ToolState {
                    provider_session_id: Some(provider.to_string()),
                    last_action_summary: "failed review".to_string(),
                    last_exit_code: 1,
                    updated_at: chrono::Utc::now(),
                    tool_version: None,
                    token_usage: None,
                },
            );
            csa_session::save_session(&session).unwrap();
        }
        let session_dir = csa_session::get_session_dir(project_root, &session.meta_session_id)
            .expect("session dir");
        write_review_meta(&session_dir, &failed_review_meta(&session.meta_session_id)).unwrap();
        session.meta_session_id
    }

    #[test]
    fn fix_finding_route_uses_actual_fallback_model_spec() {
        let project_dir = tempfile::tempdir().unwrap();
        let _sandbox = ScopedSessionSandbox::new_blocking(&project_dir);
        let session_id = create_failed_review_session(project_dir.path(), Some("provider-123"));
        write_suggestion(
            project_dir.path(),
            &session_id,
            "provider_session_id = \"provider-123\"\n",
        );

        let route = load_fix_finding_route(project_dir.path(), &session_id)
            .expect("exact route should load");

        assert_eq!(route.tool, ToolName::Codex);
        assert_eq!(
            route.model_spec.as_deref(),
            Some("codex/openai/gpt-5.5/xhigh")
        );
        assert_eq!(route.provider_session_id, "provider-123");
    }

    #[test]
    fn fix_finding_rejects_missing_provider_session_id() {
        let project_dir = tempfile::tempdir().unwrap();
        let _sandbox = ScopedSessionSandbox::new_blocking(&project_dir);
        let session_id = create_failed_review_session(project_dir.path(), None);
        write_suggestion(project_dir.path(), &session_id, "");

        let err = load_fix_finding_route(project_dir.path(), &session_id)
            .expect_err("missing provider session must fail closed");
        let msg = format!("{err:#}");

        assert!(msg.contains("provider_session_id"), "{msg}");
        assert!(msg.contains("Refusing to cold-start"), "{msg}");
    }

    #[test]
    fn fix_finding_rejects_legacy_resume_to_fix_suggestion() {
        let project_dir = tempfile::tempdir().unwrap();
        let _sandbox = ScopedSessionSandbox::new_blocking(&project_dir);
        let session_id = create_failed_review_session(project_dir.path(), Some("provider-123"));
        let session_dir = csa_session::get_session_dir(project_dir.path(), &session_id).unwrap();
        std::fs::write(
            session_dir.join("output").join("suggestion.toml"),
            format!("[suggestion]\naction = \"resume_to_fix\"\nsession_id = \"{session_id}\"\n"),
        )
        .unwrap();

        let err = load_fix_finding_route(project_dir.path(), &session_id)
            .expect_err("legacy suggestion lacks exact route");
        let msg = format!("{err:#}");

        assert!(msg.contains("expected 'confirm_then_fix_finding'"), "{msg}");
    }

    #[test]
    fn fix_finding_rejects_thinking_lock_drift() {
        let route = FixFindingRoute {
            session_id: "01TESTFIXFINDINGROUTE000".to_string(),
            tool: ToolName::Codex,
            model_spec: Some("codex/openai/gpt-5.5/xhigh".to_string()),
            model: None,
            thinking: None,
            provider_session_id: "provider-123".to_string(),
        };
        let mut config = project_config_with_codex();
        config
            .tools
            .get_mut("codex")
            .expect("codex tool")
            .thinking_lock = Some("high".to_string());

        let err = validate_route_against_config(&route, Some(&config), &GlobalConfig::default())
            .expect_err("thinking lock drift must fail closed");
        let msg = format!("{err:#}");

        assert!(msg.contains("thinking_lock"), "{msg}");
        assert!(msg.contains("xhigh"), "{msg}");
    }

    #[test]
    fn fix_finding_rejects_readonly_tool_route() {
        let project_dir = tempfile::tempdir().unwrap();
        let _sandbox = ScopedSessionSandbox::new_blocking(&project_dir);
        let session_id = create_failed_review_session(project_dir.path(), Some("provider-123"));
        let route = FixFindingRoute {
            session_id,
            tool: ToolName::Codex,
            model_spec: Some("codex/openai/gpt-5.5/xhigh".to_string()),
            model: None,
            thinking: None,
            provider_session_id: "provider-123".to_string(),
        };
        let mut config = project_config_with_codex();
        let codex = config.tools.get_mut("codex").expect("codex tool");
        codex.restrictions = Some(ToolRestrictions {
            allow_edit_existing_files: false,
            allow_write_new_files: true,
        });

        let err = validate_fix_finding_route(
            project_dir.path(),
            &route,
            Some(&config),
            &GlobalConfig::default(),
        )
        .expect_err("readonly tool route must fail before reviewer resume");
        let msg = format!("{err:#}");

        assert!(msg.contains("--fix-finding"), "{msg}");
        assert!(msg.contains("codex"), "{msg}");
        assert!(msg.contains("allow_edit_existing_files=false"), "{msg}");
    }
}
