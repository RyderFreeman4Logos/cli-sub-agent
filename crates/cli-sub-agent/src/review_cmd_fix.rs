//! Fix loop for `csa review --fix`: resumes the review session to apply fixes,
//! then re-gates via quality pipeline after each round.

use std::path::{Path, PathBuf};

use anyhow::Result;
use tracing::{error, info, warn};

use crate::review_routing::ReviewRoutingMetadata;
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::ToolName;
use csa_session::{FindingsFile, state::ReviewSessionMeta, write_findings_toml};

use super::CLEAN;
use super::output::{
    is_review_output_empty, persist_review_meta, persist_review_verdict, sanitize_review_output,
};
use super::resolve::ANTI_RECURSION_PREAMBLE;

/// All context needed to run the fix loop after a review finds issues.
pub(crate) struct FixLoopContext<'a> {
    pub effective_tool: ToolName,
    pub config: Option<&'a ProjectConfig>,
    pub global_config: &'a GlobalConfig,
    pub review_model: Option<String>,
    pub effective_tier_model_spec: Option<String>,
    pub review_thinking: Option<String>,
    pub review_routing: ReviewRoutingMetadata,
    pub stream_mode: csa_process::StreamMode,
    pub idle_timeout_seconds: u64,
    pub initial_response_timeout_seconds: Option<u64>,
    pub force_override_user_config: bool,
    pub force_ignore_tier_setting: bool,
    pub no_failover: bool,
    pub no_fs_sandbox: bool,
    pub extra_writable: &'a [PathBuf],
    pub extra_readable: &'a [PathBuf],
    pub timeout: Option<u64>,
    pub project_root: &'a Path,
    pub scope: String,
    pub decision: String,
    pub verdict: String,
    pub max_rounds: u8,
    pub initial_session_id: String,
    pub review_iterations: u32,
}

/// Run fix rounds, returning the final exit code.
///
/// Each round resumes the review session with a fix prompt, then re-runs
/// the quality gate.  Returns `Ok(0)` on first passing round, or
/// `Ok(1)` when all rounds are exhausted.
pub(crate) async fn run_fix_loop(ctx: FixLoopContext<'_>) -> Result<i32> {
    let mut session_id = ctx.initial_session_id;

    for round in 1..=ctx.max_rounds {
        info!(round, max_rounds = ctx.max_rounds, session_id = %session_id, "Fix round starting");

        let fix_prompt = format!(
            "{ANTI_RECURSION_PREAMBLE}\
             Fix round {round}/{}.\n\
             Fix all issues found in the review. Run formatting and linting commands as needed.\n\
             After applying fixes, verify the changes compile and pass basic checks.\n\
             If no issues remain, emit verdict: CLEAN.",
            ctx.max_rounds,
        );

        let fix_future = super::execute_review(
            ctx.effective_tool,
            fix_prompt,
            Some(session_id.clone()),
            ctx.review_model.clone(),
            ctx.effective_tier_model_spec.clone(),
            None,
            false,
            ctx.review_thinking.clone(),
            format!("fix round {round}/{}", ctx.max_rounds),
            ctx.project_root,
            ctx.config,
            ctx.global_config,
            ctx.review_routing.clone(),
            ctx.stream_mode,
            ctx.idle_timeout_seconds,
            ctx.initial_response_timeout_seconds,
            ctx.force_override_user_config,
            ctx.force_ignore_tier_setting,
            ctx.no_failover,
            ctx.no_fs_sandbox,
            false, // fix pass must write — override readonly_project_root
            ctx.extra_writable,
            ctx.extra_readable,
        );

        let fix_result = if let Some(timeout_secs) = ctx.timeout {
            match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), fix_future)
                .await
            {
                Ok(inner) => inner?,
                Err(_) => {
                    error!(
                        timeout_secs = timeout_secs,
                        round, "Fix round aborted: wall-clock timeout exceeded"
                    );
                    anyhow::bail!(
                        "Fix round {round}/{} aborted: --timeout {timeout_secs}s exceeded.",
                        ctx.max_rounds,
                    );
                }
            }
        } else {
            fix_future.await?
        };

        print!(
            "{}",
            sanitize_review_output(&fix_result.execution.execution.output)
        );
        let fix_empty = is_review_output_empty(&fix_result.execution.execution.output);
        if fix_empty {
            warn!(
                round,
                "Fix round produced no substantive output — treating as failed"
            );
        }
        session_id = fix_result.execution.meta_session_id.clone();

        // Run the quality gate after fix.
        let fix_gate_steps = ctx.global_config.review.effective_gate_steps();
        let fix_gate_timeout = ctx
            .config
            .and_then(|c| c.review.as_ref())
            .map(|r| r.gate_timeout_secs)
            .unwrap_or_else(csa_config::ReviewConfig::default_gate_timeout);
        let fix_gate_mode = &ctx.global_config.review.gate_mode;

        let gate_passed = if fix_gate_steps.is_empty() {
            let gate_command = ctx
                .config
                .and_then(|c| c.review.as_ref())
                .and_then(|r| r.gate_command.as_deref());
            let gate_result = crate::pipeline::gate::evaluate_quality_gate(
                ctx.project_root,
                gate_command,
                fix_gate_timeout,
                fix_gate_mode,
            )
            .await?;

            if !gate_result.passed() {
                warn!(
                    round,
                    max_rounds = ctx.max_rounds,
                    command = %gate_result.command,
                    exit_code = ?gate_result.exit_code,
                    "Quality gate still failing after fix round"
                );
            }
            gate_result.passed()
        } else {
            let pipeline_result = crate::pipeline::gate::evaluate_quality_gates(
                ctx.project_root,
                &fix_gate_steps,
                fix_gate_timeout,
                fix_gate_mode,
            )
            .await?;

            if !pipeline_result.passed {
                warn!(
                    round,
                    max_rounds = ctx.max_rounds,
                    failed_step = ?pipeline_result.failed_step,
                    "Quality gate pipeline still failing after fix round"
                );
            }
            pipeline_result.passed
        };

        if gate_passed && !fix_empty {
            info!(round, "Fix round succeeded — quality gate passed");
            let review_meta = ReviewSessionMeta {
                session_id: session_id.clone(),
                head_sha: csa_session::detect_git_head(ctx.project_root).unwrap_or_default(),
                decision: "pass".to_string(),
                verdict: CLEAN.to_string(),
                status_reason: None,
                routed_to: None,
                primary_failure: None,
                failure_reason: None,
                tool: ctx.effective_tool.to_string(),
                scope: ctx.scope.clone(),
                exit_code: 0,
                fix_attempted: true,
                fix_rounds: u32::from(round),
                review_iterations: ctx.review_iterations,
                timestamp: chrono::Utc::now(),
                diff_fingerprint: None,
            };
            persist_fix_final_artifacts(ctx.project_root, &review_meta, true);
            return Ok(0);
        }
    }

    // All fix rounds exhausted; gate still fails.
    let review_meta = ReviewSessionMeta {
        session_id,
        head_sha: csa_session::detect_git_head(ctx.project_root).unwrap_or_default(),
        decision: ctx.decision,
        verdict: ctx.verdict,
        status_reason: None,
        routed_to: None,
        primary_failure: None,
        failure_reason: None,
        tool: ctx.effective_tool.to_string(),
        scope: ctx.scope,
        exit_code: 1,
        fix_attempted: true,
        fix_rounds: u32::from(ctx.max_rounds),
        review_iterations: ctx.review_iterations,
        timestamp: chrono::Utc::now(),
        diff_fingerprint: None,
    };
    persist_fix_final_artifacts(ctx.project_root, &review_meta, false);
    error!(
        max_rounds = ctx.max_rounds,
        "All fix rounds exhausted — quality gate still failing"
    );
    Ok(1)
}

fn persist_fix_final_artifacts(
    project_root: &Path,
    review_meta: &ReviewSessionMeta,
    converged_clean: bool,
) {
    persist_review_meta(project_root, review_meta);
    if converged_clean {
        match csa_session::get_session_dir(project_root, &review_meta.session_id) {
            Ok(session_dir) => {
                if let Err(error) = write_findings_toml(&session_dir, &FindingsFile::default()) {
                    warn!(
                        session_id = %review_meta.session_id,
                        error = %error,
                        "Failed to write output/findings.toml after CLEAN convergence"
                    );
                }
                // Clear stale review-findings.json so the cross-check in
                // derive_review_verdict_artifact does not override the
                // cleanly-converged empty findings.toml (#1045 round 4).
                let stale_json = session_dir.join("review-findings.json");
                if stale_json.exists()
                    && let Err(error) = std::fs::remove_file(&stale_json)
                {
                    warn!(
                        session_id = %review_meta.session_id,
                        error = %error,
                        "Failed to remove stale review-findings.json after CLEAN convergence"
                    );
                }
                // Clear the synthetic-empty sidecar marker so
                // derive_review_verdict_artifact does not fall through to
                // full.md after clean convergence (#1048 M3).
                let synthetic_marker = session_dir
                    .join("output")
                    .join(super::findings_toml::FINDINGS_TOML_SYNTHETIC_MARKER);
                if let Err(error) = std::fs::remove_file(&synthetic_marker)
                    && error.kind() != std::io::ErrorKind::NotFound
                {
                    warn!(
                        session_id = %review_meta.session_id,
                        error = %error,
                        "Failed to remove synthetic marker after CLEAN convergence"
                    );
                }
            }
            Err(error) => {
                warn!(
                    session_id = %review_meta.session_id,
                    error = %error,
                    "Cannot resolve session dir for final fix artifacts"
                );
            }
        }
    }
    persist_review_verdict(project_root, review_meta, &[], Vec::new());
}

#[cfg(test)]
pub(crate) fn persist_fix_final_artifacts_for_tests(
    project_root: &Path,
    review_meta: &ReviewSessionMeta,
    converged_clean: bool,
) {
    persist_fix_final_artifacts(project_root, review_meta, converged_clean);
}

#[cfg(test)]
mod tests {
    use super::{CLEAN, persist_fix_final_artifacts};
    use crate::test_env_lock::ScopedTestEnvVar;
    use csa_core::types::ReviewDecision;
    use csa_session::state::ReviewSessionMeta;
    use csa_session::{
        FindingsFile, ReviewFinding, ReviewFindingFileRange, Severity, write_findings_toml,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn make_clean_review_meta(session_id: &str) -> ReviewSessionMeta {
        ReviewSessionMeta {
            session_id: session_id.to_string(),
            head_sha: String::new(),
            decision: ReviewDecision::Pass.as_str().to_string(),
            verdict: CLEAN.to_string(),
            status_reason: None,
            routed_to: None,
            primary_failure: None,
            failure_reason: None,
            tool: "codex".to_string(),
            scope: "diff".to_string(),
            exit_code: 0,
            fix_attempted: true,
            fix_rounds: 1,
            review_iterations: 1,
            timestamp: chrono::Utc::now(),
            diff_fingerprint: None,
        }
    }

    fn temp_project_root(test_name: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("csa-{test_name}-{suffix}"));
        fs::create_dir_all(&path).expect("create temp project root");
        path
    }

    fn unique_session_id(prefix: &str) -> String {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_nanos();
        format!("{prefix}-{suffix}")
    }

    fn create_session_dir(project_root: &Path, session_id: &str) -> PathBuf {
        let session_dir =
            csa_session::get_session_dir(project_root, session_id).expect("resolve session dir");
        fs::create_dir_all(session_dir.join("output")).expect("create session output dir");
        session_dir
    }

    fn sample_stale_finding() -> ReviewFinding {
        ReviewFinding {
            id: "stale-medium".to_string(),
            severity: Severity::Medium,
            file_ranges: vec![ReviewFindingFileRange {
                path: "src/lib.rs".to_string(),
                start: 42,
                end: Some(42),
            }],
            is_regression_of_commit: None,
            suggested_test_scenario: None,
            description: "Stale finding from a previous fix round.".to_string(),
        }
    }

    #[test]
    fn persist_fix_final_artifacts_rewrites_stale_findings_toml_to_empty_on_clean() {
        let project_root = temp_project_root("persist-fix-final-artifacts");
        let _state_home = ScopedTestEnvVar::set("XDG_STATE_HOME", project_root.join("state"));
        let session_id = unique_session_id("01FIXFINALARTIFACTS");
        let session_dir = create_session_dir(&project_root, &session_id);

        write_findings_toml(
            &session_dir,
            &FindingsFile {
                findings: vec![sample_stale_finding()],
            },
        )
        .expect("write stale findings.toml");

        persist_fix_final_artifacts(&project_root, &make_clean_review_meta(&session_id), true);

        let findings_path = session_dir.join("output").join("findings.toml");
        assert!(
            findings_path.exists(),
            "findings.toml should remain present"
        );

        let actual = fs::read_to_string(&findings_path).expect("read findings.toml");
        let parsed: FindingsFile = toml::from_str(&actual).expect("parse findings.toml");
        assert_eq!(parsed, FindingsFile::default());
    }

    #[test]
    fn persist_fix_final_artifacts_refreshes_verdict_after_findings_normalized() {
        let project_root = temp_project_root("persist-fix-final-artifacts-verdict-refresh");
        let _state_home = ScopedTestEnvVar::set("XDG_STATE_HOME", project_root.join("state"));
        let session_id = unique_session_id("01FIXFINALVERDICT");
        let session_dir = create_session_dir(&project_root, &session_id);

        write_findings_toml(
            &session_dir,
            &FindingsFile {
                findings: vec![ReviewFinding {
                    id: "stale-high".to_string(),
                    severity: Severity::High,
                    file_ranges: vec![ReviewFindingFileRange {
                        path: "src/lib.rs".to_string(),
                        start: 7,
                        end: Some(7),
                    }],
                    is_regression_of_commit: None,
                    suggested_test_scenario: None,
                    description: "Stale high finding from a previous fix round.".to_string(),
                }],
            },
        )
        .expect("write stale findings.toml");

        fs::write(
            session_dir.join("output").join("full.md"),
            "<!-- CSA:SECTION:summary -->\nPASS\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:details -->\nNo blocking issues found in this scope.\nOverall risk: low\n<!-- CSA:SECTION:details:END -->",
        )
        .expect("write full output transcript");

        persist_fix_final_artifacts(&project_root, &make_clean_review_meta(&session_id), true);

        let verdict_path = session_dir.join("output").join("review-verdict.json");
        let artifact: csa_session::ReviewVerdictArtifact =
            serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
                .expect("parse verdict");
        assert_eq!(artifact.decision, ReviewDecision::Pass);
        assert_eq!(artifact.severity_counts.get(&Severity::High), Some(&0));
    }

    /// Round 4 repro (#1045): stale review-findings.json with a HIGH finding
    /// left over from the pre-fix review round. Fix loop converges clean →
    /// both findings.toml and review-findings.json must be cleared, and the
    /// final verdict must report decision=pass / severity_counts.high=0.
    #[test]
    fn persist_fix_final_artifacts_clears_stale_review_findings_json_on_clean() {
        let project_root = temp_project_root("persist-fix-stale-json");
        let _state_home = ScopedTestEnvVar::set("XDG_STATE_HOME", project_root.join("state"));
        let session_id = unique_session_id("01FIXSTALEJSON");
        let session_dir = create_session_dir(&project_root, &session_id);

        // Seed stale review-findings.json with a HIGH finding (from pre-fix round).
        let stale_json = serde_json::json!({
            "findings": [{
                "severity": "high",
                "fid": "stale-high",
                "file": "src/lib.rs",
                "line": 42,
                "rule_id": "rule.stale",
                "summary": "Stale high finding from pre-fix review",
                "engine": "reviewer"
            }],
            "severity_summary": { "critical": 0, "high": 1, "medium": 0, "low": 0 },
            "overall_risk": "high"
        });
        fs::write(
            session_dir.join("review-findings.json"),
            serde_json::to_vec_pretty(&stale_json).expect("serialize stale json"),
        )
        .expect("write stale review-findings.json");

        // No stale findings.toml — persist_fix_final_artifacts will create a clean one.
        persist_fix_final_artifacts(&project_root, &make_clean_review_meta(&session_id), true);

        // findings.toml must be empty.
        let findings_path = session_dir.join("output").join("findings.toml");
        let parsed: FindingsFile =
            toml::from_str(&fs::read_to_string(&findings_path).expect("read findings.toml"))
                .expect("parse findings.toml");
        assert_eq!(parsed, FindingsFile::default());

        // review-findings.json must be removed.
        assert!(
            !session_dir.join("review-findings.json").exists(),
            "review-findings.json should be removed after clean convergence"
        );

        // Verdict must report pass.
        let verdict_path = session_dir.join("output").join("review-verdict.json");
        let artifact: csa_session::ReviewVerdictArtifact =
            serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
                .expect("parse verdict");
        assert_eq!(artifact.decision, ReviewDecision::Pass);
        assert_eq!(artifact.severity_counts.get(&Severity::High), Some(&0));
    }

    /// Mixed staleness (#1045 round 4): stale findings.toml with a MEDIUM finding
    /// + stale review-findings.json with a HIGH finding. Converge clean → both
    /// must be cleared, verdict must be pass.
    #[test]
    fn persist_fix_final_artifacts_clears_both_stale_artifacts_on_clean() {
        let project_root = temp_project_root("persist-fix-both-stale");
        let _state_home = ScopedTestEnvVar::set("XDG_STATE_HOME", project_root.join("state"));
        let session_id = unique_session_id("01FIXBOTHSTALE");
        let session_dir = create_session_dir(&project_root, &session_id);

        // Stale findings.toml with a MEDIUM finding.
        write_findings_toml(
            &session_dir,
            &FindingsFile {
                findings: vec![sample_stale_finding()],
            },
        )
        .expect("write stale findings.toml");

        // Stale review-findings.json with a HIGH finding.
        let stale_json = serde_json::json!({
            "findings": [{
                "severity": "high",
                "fid": "stale-high-json",
                "file": "src/main.rs",
                "line": 10,
                "rule_id": "rule.stale-json",
                "summary": "Stale high from JSON",
                "engine": "reviewer"
            }],
            "severity_summary": { "critical": 0, "high": 1, "medium": 0, "low": 0 },
            "overall_risk": "high"
        });
        fs::write(
            session_dir.join("review-findings.json"),
            serde_json::to_vec_pretty(&stale_json).expect("serialize stale json"),
        )
        .expect("write stale review-findings.json");

        persist_fix_final_artifacts(&project_root, &make_clean_review_meta(&session_id), true);

        // findings.toml must be empty.
        let findings_path = session_dir.join("output").join("findings.toml");
        let parsed: FindingsFile =
            toml::from_str(&fs::read_to_string(&findings_path).expect("read findings.toml"))
                .expect("parse findings.toml");
        assert_eq!(parsed, FindingsFile::default());

        // review-findings.json must be removed.
        assert!(
            !session_dir.join("review-findings.json").exists(),
            "review-findings.json should be removed after clean convergence"
        );

        // Verdict must report pass with zero blocking counts.
        let verdict_path = session_dir.join("output").join("review-verdict.json");
        let artifact: csa_session::ReviewVerdictArtifact =
            serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
                .expect("parse verdict");
        assert_eq!(artifact.decision, ReviewDecision::Pass);
        assert_eq!(artifact.severity_counts.get(&Severity::High), Some(&0));
        assert_eq!(artifact.severity_counts.get(&Severity::Medium), Some(&0));
    }

    /// Non-converged fix (exhausted rounds): stale artifacts must be PRESERVED
    /// — this is the existing contract per decision #820 Option A.
    #[test]
    fn fix_loop_exhausted_preserves_stale_review_findings_json() {
        let project_root = temp_project_root("persist-fix-exhausted-json");
        let _state_home = ScopedTestEnvVar::set("XDG_STATE_HOME", project_root.join("state"));
        let session_id = unique_session_id("01FIXEXHAUSTEDJSON");
        let session_dir = create_session_dir(&project_root, &session_id);

        // Stale review-findings.json with a HIGH finding.
        let stale_json = serde_json::json!({
            "findings": [{
                "severity": "high",
                "fid": "stale-high-exhausted",
                "file": "src/lib.rs",
                "line": 99,
                "rule_id": "rule.stale-exhausted",
                "summary": "Stale high finding persisted on exhaustion",
                "engine": "reviewer"
            }],
            "severity_summary": { "critical": 0, "high": 1, "medium": 0, "low": 0 },
            "overall_risk": "high"
        });
        fs::write(
            session_dir.join("review-findings.json"),
            serde_json::to_vec_pretty(&stale_json).expect("serialize stale json"),
        )
        .expect("write stale review-findings.json");

        // Stale findings.toml too.
        write_findings_toml(
            &session_dir,
            &FindingsFile {
                findings: vec![sample_stale_finding()],
            },
        )
        .expect("write stale findings.toml");

        let mut exhausted_meta = make_clean_review_meta(&session_id);
        exhausted_meta.decision = ReviewDecision::Fail.as_str().to_string();
        exhausted_meta.verdict = "HAS_ISSUES".to_string();
        exhausted_meta.exit_code = 1;
        exhausted_meta.fix_rounds = 3;

        persist_fix_final_artifacts(&project_root, &exhausted_meta, false);

        // review-findings.json must still exist (not cleaned on non-convergence).
        assert!(
            session_dir.join("review-findings.json").exists(),
            "review-findings.json should be preserved when fix loop is exhausted"
        );

        // findings.toml must still have the stale content.
        let findings_path = session_dir.join("output").join("findings.toml");
        let parsed: FindingsFile =
            toml::from_str(&fs::read_to_string(&findings_path).expect("read findings.toml"))
                .expect("parse findings.toml");
        assert_eq!(
            parsed,
            FindingsFile {
                findings: vec![sample_stale_finding()],
            }
        );
    }

    /// #1048 M3: --fix session that started from synthetic-empty initial
    /// review, converges clean → synthetic marker must be removed alongside
    /// findings.toml and review-findings.json.
    ///
    /// Bug: persist_fix_final_artifacts(converged_clean=true) cleared
    /// findings.toml + review-findings.json but left the synthetic sidecar
    /// marker in place, causing derive_review_verdict_artifact to fall
    /// through to full.md on subsequent reads.
    #[test]
    fn persist_fix_final_artifacts_clears_synthetic_marker_on_clean_convergence() {
        let project_root = temp_project_root("persist-fix-synthetic-marker-clean");
        let _state_home = ScopedTestEnvVar::set("XDG_STATE_HOME", project_root.join("state"));
        let session_id = unique_session_id("01FIXSYNTHETICMARKERCLEAN");
        let session_dir = create_session_dir(&project_root, &session_id);

        // Create the synthetic marker (simulates a fix session that started
        // from a synthetic-empty initial review).
        let marker_path = session_dir
            .join("output")
            .join(super::super::findings_toml::FINDINGS_TOML_SYNTHETIC_MARKER);
        fs::write(&marker_path, b"").expect("write synthetic marker");

        // Seed stale review-findings.json to verify it's also removed.
        let stale_json = serde_json::json!({
            "findings": [{
                "severity": "medium",
                "fid": "stale-medium-synth",
                "file": "src/lib.rs",
                "line": 10,
                "rule_id": "rule.stale",
                "summary": "Stale finding from pre-fix review",
                "engine": "reviewer"
            }],
            "severity_summary": { "critical": 0, "high": 0, "medium": 1, "low": 0 },
            "overall_risk": "medium"
        });
        fs::write(
            session_dir.join("review-findings.json"),
            serde_json::to_vec_pretty(&stale_json).expect("serialize stale json"),
        )
        .expect("write stale review-findings.json");

        persist_fix_final_artifacts(&project_root, &make_clean_review_meta(&session_id), true);

        // findings.toml must be empty.
        let findings_path = session_dir.join("output").join("findings.toml");
        let parsed: FindingsFile =
            toml::from_str(&fs::read_to_string(&findings_path).expect("read findings.toml"))
                .expect("parse findings.toml");
        assert_eq!(parsed, FindingsFile::default());

        // review-findings.json must be removed.
        assert!(
            !session_dir.join("review-findings.json").exists(),
            "review-findings.json should be removed after clean convergence"
        );

        // Synthetic marker must be removed (#1048 M3).
        assert!(
            !marker_path.exists(),
            "#1048 M3: synthetic marker must be removed after clean convergence"
        );

        // Verdict must report pass.
        let verdict_path = session_dir.join("output").join("review-verdict.json");
        let artifact: csa_session::ReviewVerdictArtifact =
            serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
                .expect("parse verdict");
        assert_eq!(artifact.decision, ReviewDecision::Pass);
    }

    #[test]
    fn fix_loop_exhausted_preserves_open_findings_in_findings_toml() {
        let project_root = temp_project_root("persist-fix-final-artifacts-exhausted");
        let _state_home = ScopedTestEnvVar::set("XDG_STATE_HOME", project_root.join("state"));
        let session_id = unique_session_id("01FIXEXHAUSTEDARTIFACTS");
        let session_dir = create_session_dir(&project_root, &session_id);
        let existing = FindingsFile {
            findings: vec![sample_stale_finding()],
        };

        write_findings_toml(&session_dir, &existing).expect("write last-round findings.toml");

        let mut exhausted_meta = make_clean_review_meta(&session_id);
        exhausted_meta.decision = ReviewDecision::Fail.as_str().to_string();
        exhausted_meta.verdict = "HAS_ISSUES".to_string();
        exhausted_meta.exit_code = 1;
        exhausted_meta.fix_rounds = 3;

        persist_fix_final_artifacts(&project_root, &exhausted_meta, false);

        let findings_path = session_dir.join("output").join("findings.toml");
        assert!(
            findings_path.exists(),
            "findings.toml should remain present after exhausted fix loop"
        );

        let actual = fs::read_to_string(&findings_path).expect("read preserved findings.toml");
        let parsed: FindingsFile = toml::from_str(&actual).expect("parse preserved findings.toml");
        assert_eq!(parsed, existing);
    }
}
