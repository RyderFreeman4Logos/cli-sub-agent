// NOTE #1858: path-included; avoid crate:: and binary-only methods.
use super::super::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};
use super::*;
use csa_config::{GlobalConfig, ProjectMeta, ResourcesConfig, ToolConfig};
use csa_core::env::{CSA_PARENT_SESSION_DIR_ENV_KEY, CSA_SESSION_DIR_ENV_KEY};
use csa_session::review_artifact::{Finding, ReviewArtifact, Severity, SeveritySummary};
use proptest::prelude::*;
use std::path::PathBuf;
use tempfile::tempdir;
fn project_config_with_enabled_tools(tools: &[&str]) -> ProjectConfig {
    let mut tool_map = HashMap::new();
    for tool in csa_config::global::all_known_tools() {
        tool_map.insert(
            tool.as_str().to_string(),
            ToolConfig {
                enabled: false,
                restrictions: None,
                suppress_notify: true,
                ..Default::default()
            },
        );
    }
    for tool in tools {
        tool_map.insert(
            (*tool).to_string(),
            ToolConfig {
                enabled: true,
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
            memory_max_mb: Some(1024),
            min_free_memory_mb: 1,
            ..Default::default()
        },
        acp: Default::default(),
        tools: tool_map,
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        tool_aliases: HashMap::new(),
        preferences: None,
        github: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        tool_state_dirs: HashMap::new(),
        filesystem_sandbox: Default::default(),
    }
}

fn response(agent: &str, verdict: &str, timed_out: bool) -> AgentResponse {
    AgentResponse {
        agent: agent.to_string(),
        content: verdict.to_string(),
        weight: 1.0,
        timed_out,
    }
}

fn verdict_to_exit_code(verdict: &str) -> i32 {
    if verdict == CLEAN { 0 } else { 1 }
}

#[derive(Clone, Copy, Debug)]
enum ReviewerState {
    Pass,
    Fail,
    Unavailable,
}

impl ReviewerState {
    fn verdict(self) -> &'static str {
        match self {
            Self::Pass => CLEAN,
            Self::Fail => HAS_ISSUES,
            Self::Unavailable => UNAVAILABLE,
        }
    }
}

fn finding_with_location(
    fid: &str,
    severity: Severity,
    file: &str,
    rule_id: &str,
    line: Option<u32>,
) -> Finding {
    Finding {
        severity,
        fid: fid.to_string(),
        file: file.to_string(),
        line,
        rule_id: rule_id.to_string(),
        summary: format!("finding-{fid}"),
        engine: "reviewer".to_string(),
    }
}

fn finding(fid: &str, severity: Severity) -> Finding {
    finding_with_location(
        fid,
        severity,
        "src/lib.rs",
        &format!("rule.sample.{fid}"),
        Some(1),
    )
}

fn artifact_with_findings(session_id: &str, findings: Vec<Finding>) -> ReviewArtifact {
    ReviewArtifact {
        severity_summary: SeveritySummary::from_findings(&findings),
        findings,
        review_mode: None,
        schema_version: "1.0".to_string(),
        session_id: session_id.to_string(),
        timestamp: chrono::Utc::now(),
    }
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf()
}

#[test]
fn reviewer_prompt_documents_unavailable_state() {
    for relative_path in [
        "patterns/csa-review/PATTERN.md",
        "patterns/csa-review/workflow.toml",
        "patterns/csa-review/skills/csa-review/SKILL.md",
        "patterns/csa-review/skills/csa-review/references/output-schema.md",
    ] {
        let content =
            std::fs::read_to_string(workspace_root().join(relative_path)).expect("read doc");
        let lower = content.to_ascii_lowercase();
        assert!(
            lower.contains("unavailable"),
            "{relative_path} must document the unavailable decision state"
        );
    }

    let pattern = std::fs::read_to_string(workspace_root().join("patterns/csa-review/PATTERN.md"))
        .expect("read pattern");
    let pattern_lower = pattern.to_ascii_lowercase();
    assert!(
        pattern_lower.contains("quota/auth/network"),
        "pattern must explain unavailable as an infrastructure failure"
    );
    assert!(
        pattern_lower.contains("lacks confidence"),
        "pattern must distinguish unavailable from uncertain"
    );
}

#[test]
fn build_reviewer_tools_returns_empty_when_reviewer_count_is_zero() {
    let cfg = project_config_with_enabled_tools(&["codex", "opencode"]);
    let tools = build_reviewer_tools(None, ToolName::Codex, Some(&cfg), None, None, 0);
    assert!(tools.is_empty());
}

#[test]
fn build_reviewer_tools_round_robin_across_enabled_tools() {
    let cfg = project_config_with_enabled_tools(&["codex", "claude-code", "opencode"]);
    let tools = build_reviewer_tools(None, ToolName::Codex, Some(&cfg), None, None, 5);
    assert_eq!(
        tools,
        vec![
            ToolName::Codex,
            ToolName::Opencode,
            ToolName::ClaudeCode,
            ToolName::Codex,
            ToolName::Opencode
        ]
    );
}

#[test]
fn build_reviewer_tools_respects_explicit_tool_override() {
    let cfg = project_config_with_enabled_tools(&["codex", "claude-code", "opencode"]);
    let tools = build_reviewer_tools(
        Some(ToolName::Codex),
        ToolName::Codex,
        Some(&cfg),
        None,
        None,
        3,
    );
    assert_eq!(
        tools,
        vec![ToolName::Codex, ToolName::Codex, ToolName::Codex]
    );
}

#[test]
fn build_reviewer_tools_uses_tier_pool_when_present() {
    let tier_tools = [ToolName::GeminiCli, ToolName::Codex, ToolName::ClaudeCode];

    let tools = build_reviewer_tools(
        None,
        ToolName::Codex,
        None,
        Some(&GlobalConfig::default()),
        Some(&tier_tools),
        5,
    );
    assert_eq!(
        tools,
        vec![
            ToolName::Codex,
            ToolName::GeminiCli,
            ToolName::ClaudeCode,
            ToolName::Codex,
            ToolName::GeminiCli
        ]
    );
}

#[test]
fn validate_multi_reviewer_tier_pool_rejects_single_tool_consensus() {
    let error =
        validate_multi_reviewer_tier_pool("tier-review", 3, ToolName::Codex, &[ToolName::Codex])
            .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("only resolves to one available reviewer tool"),
        "unexpected error: {error}"
    );
}

#[test]
fn validate_multi_reviewer_tier_pool_reports_unique_tool_count() {
    let unique_tools = validate_multi_reviewer_tier_pool(
        "tier-review",
        3,
        ToolName::Codex,
        &[ToolName::GeminiCli, ToolName::ClaudeCode],
    )
    .unwrap();
    assert_eq!(unique_tools, 3);
}

#[test]
fn parse_review_verdict_prefers_has_issues_token() {
    let output = "result: CLEAN but escalation says HAS_ISSUES";
    assert_eq!(parse_review_verdict(output, 0), HAS_ISSUES);
}

#[test]
fn parse_review_verdict_falls_back_to_exit_code() {
    assert_eq!(parse_review_verdict("no explicit verdict", 0), HAS_ISSUES);
    assert_eq!(parse_review_verdict("no explicit verdict", 1), HAS_ISSUES);
}

#[test]
fn parse_review_verdict_does_not_treat_findings_as_clean_from_exit_zero() {
    let output = "<!-- CSA:SECTION:details -->\n1. P1 issue in workflow.toml\n<!-- CSA:SECTION:details:END -->";
    assert_eq!(parse_review_verdict(output, 0), HAS_ISSUES);
}

#[test]
fn parse_review_verdict_accepts_clean_phrase_without_explicit_token() {
    assert_eq!(
        parse_review_verdict("No issues found in this scope.", 0),
        CLEAN
    );
    assert_eq!(
        parse_review_verdict("No critical security or functional issues were found.", 0),
        CLEAN
    );
    assert_eq!(
        parse_review_verdict("\u{672a}\u{53d1}\u{73b0}\u{95ee}\u{9898}\u{3002}", 0),
        CLEAN
    );
}

#[test]
fn summary_and_verdict_paths_agree() {
    let output = r#"<!-- CSA:SECTION:summary -->
PASS
<!-- CSA:SECTION:summary:END -->
<!-- CSA:SECTION:details -->
No blocking findings.

Notes:
- I did not run the test suite because this CSA subprocess is read-only.
- Codegraph was unavailable because this checkout has no initialized index.
<!-- CSA:SECTION:details:END -->
"#;

    let summary_verdict = parse_review_verdict(output, 0);
    let structured_decision = parse_review_decision(output, 0);

    assert_eq!(summary_verdict, CLEAN);
    assert_eq!(structured_decision, csa_core::types::ReviewDecision::Pass);
}

#[test]
fn build_multi_reviewer_instruction_keeps_session_dir_deferred_when_env_exists() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let _parent_guard =
        ScopedEnvVarRestore::set(CSA_PARENT_SESSION_DIR_ENV_KEY, "/tmp/parent-session");
    let _session_guard = ScopedEnvVarRestore::set(CSA_SESSION_DIR_ENV_KEY, "/tmp/child-session");
    let project_dir = tempdir().expect("tempdir should be created");

    let prompt = build_multi_reviewer_instruction(
        "Base prompt",
        2,
        ToolName::Codex,
        project_dir.path(),
        None,
        None,
    );

    assert!(
        prompt.contains(
            "${CSA_PARENT_SESSION_DIR:-$CSA_SESSION_DIR}/reviewer-2/review-findings.json"
        ),
        "review prompt should prefer the parent session dir while deferring shell resolution"
    );
    assert!(
        !prompt.contains("/tmp/child-session") && !prompt.contains("/tmp/parent-session"),
        "review prompt must not capture the parent process session dirs"
    );
}

#[test]
fn build_multi_reviewer_instruction_does_not_capture_parent_session_dir_env() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let _parent_guard =
        ScopedEnvVarRestore::set(CSA_PARENT_SESSION_DIR_ENV_KEY, "/tmp/parent-session");
    let _session_guard = ScopedEnvVarRestore::unset(CSA_SESSION_DIR_ENV_KEY);
    let project_dir = tempdir().expect("tempdir should be created");

    let prompt = build_multi_reviewer_instruction(
        "Base prompt",
        3,
        ToolName::Codex,
        project_dir.path(),
        None,
        None,
    );

    assert!(
        prompt.contains(
            "${CSA_PARENT_SESSION_DIR:-$CSA_SESSION_DIR}/reviewer-3/review-findings.json"
        )
    );
    assert!(
        !prompt.contains("/tmp/parent-session"),
        "review prompt must not inherit the outer CSA session dir"
    );
}

#[test]
fn build_multi_reviewer_instruction_uses_parent_first_shell_fallback_when_env_missing() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let _parent_guard = ScopedEnvVarRestore::unset(CSA_PARENT_SESSION_DIR_ENV_KEY);
    let _session_guard = ScopedEnvVarRestore::unset(CSA_SESSION_DIR_ENV_KEY);
    let project_dir = tempdir().expect("tempdir should be created");

    let prompt = build_multi_reviewer_instruction(
        "Base prompt",
        4,
        ToolName::Codex,
        project_dir.path(),
        None,
        None,
    );

    assert!(
        prompt.contains(
            "${CSA_PARENT_SESSION_DIR:-$CSA_SESSION_DIR}/reviewer-4/review-findings.json"
        )
    );
}

#[test]
fn build_multi_reviewer_instruction_does_not_duplicate_findings_contract_from_base_prompt() {
    let base_prompt = format!("Base prompt\n\n{REVIEW_FINDINGS_TOML_INSTRUCTION}");
    let project_dir = tempdir().expect("tempdir should be created");

    let prompt = build_multi_reviewer_instruction(
        &base_prompt,
        4,
        ToolName::Codex,
        project_dir.path(),
        None,
        None,
    );

    assert_eq!(prompt.matches(REVIEW_FINDINGS_TOML_INSTRUCTION).count(), 1);
}

#[test]
fn parse_review_verdict_is_case_insensitive_and_token_aware() {
    assert_eq!(
        parse_review_verdict("final verdict: clean.", 1),
        CLEAN,
        "token matching should be case-insensitive"
    );
    assert_eq!(
        parse_review_verdict("status: unclean output", 1),
        HAS_ISSUES,
        "partial-word matches must not be treated as CLEAN"
    );
}

#[test]
fn parse_consensus_strategy_supports_all_cli_values() {
    assert_eq!(
        parse_consensus_strategy("majority").unwrap(),
        ConsensusStrategy::Majority
    );
    assert_eq!(
        parse_consensus_strategy("weighted").unwrap(),
        ConsensusStrategy::Weighted
    );
    assert_eq!(
        parse_consensus_strategy("unanimous").unwrap(),
        ConsensusStrategy::Unanimous
    );
    assert!(parse_consensus_strategy("invalid").is_err());
}

include!("review_consensus_tests_tail.rs");
