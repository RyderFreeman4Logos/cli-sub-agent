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
    let session_dir =
        csa_session::get_session_dir(project_root, &session.meta_session_id).expect("session dir");
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

    let route =
        load_fix_finding_route(project_dir.path(), &session_id).expect("exact route should load");

    assert_eq!(route.tool, ToolName::Codex);
    assert_eq!(
        route.model_spec.as_deref(),
        Some("codex/openai/gpt-5.5/xhigh")
    );
    assert_eq!(route.provider_session_id, "provider-123");
}

#[test]
fn fix_finding_creates_distinct_execution_session_with_provider_resume() {
    let project_dir = tempfile::tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&project_dir);
    let review_session_id = create_failed_review_session(project_dir.path(), Some("provider-123"));
    let route = FixFindingRoute {
        session_id: review_session_id.clone(),
        tool: ToolName::Codex,
        model_spec: Some("codex/openai/gpt-5.5/xhigh".to_string()),
        model: None,
        thinking: None,
        provider_session_id: "provider-123".to_string(),
    };

    let fix_session_id = create_fix_finding_session(project_dir.path(), &route)
        .expect("fix session should be pre-created");

    assert_ne!(
        fix_session_id, review_session_id,
        "fix-finding must not write its result into the original review session"
    );
    let fix_session =
        csa_session::load_session(project_dir.path(), &fix_session_id).expect("fix session");
    assert_eq!(
        fix_session.genealogy.parent_session_id.as_deref(),
        Some(review_session_id.as_str())
    );
    assert_eq!(
        fix_session.task_context.task_type.as_deref(),
        Some(FIX_FINDING_TASK_TYPE)
    );
    assert_eq!(
        fix_session
            .tools
            .get("codex")
            .and_then(|state| state.provider_session_id.as_deref()),
        Some("provider-123")
    );
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
