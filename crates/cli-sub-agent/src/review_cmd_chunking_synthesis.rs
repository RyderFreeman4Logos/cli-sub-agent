use super::*;

pub(super) async fn run_synthesis_review(
    ctx: &ChunkedReviewContext<'_>,
    parent_startup_env: &StartupSubtreeEnv,
    diff_fingerprint: Option<&str>,
    outcomes: &[ReviewerOutcome],
) -> Result<ReviewerOutcome> {
    let reviewer_index = ctx.plan.chunk_count();
    let consolidated = load_consolidated_chunk_artifact(
        ctx.project_root,
        parent_startup_env.session_dir().map(Path::new),
        outcomes,
    )?;
    let prompt = build_synthesis_review_instruction(
        ctx.prompt,
        &ctx.plan,
        outcomes,
        &consolidated,
        reviewer_index + 1,
        diff_fingerprint,
    );
    let result = execute_review_with_tier_filter(
        ctx.tool,
        prompt,
        None,
        ctx.review_model.clone(),
        ctx.resolved_model_spec.clone(),
        ctx.resolved_tier_name.clone(),
        ctx.tier_active,
        ctx.tier_preference_order.clone(),
        ctx.review_thinking.clone(),
        format!(
            "review[synthesis]: {}",
            crate::run_helpers::truncate_prompt(ctx.scope, 80)
        ),
        ctx.project_root,
        ctx.config.as_ref(),
        ctx.global_config,
        ctx.model_catalog,
        ctx.pre_session_hook.clone(),
        ctx.review_routing.clone(),
        ctx.stream_mode,
        ctx.idle_timeout_seconds,
        ctx.initial_response_timeout_seconds,
        ctx.args.force_override_user_config,
        ctx.args.force_ignore_tier_setting,
        ctx.execution_no_failover,
        ctx.explicit_tool_with_failover,
        ctx.build_jobs,
        ctx.args.fast_but_more_cost,
        false,
        ctx.args.no_fs_sandbox,
        ctx.allow_user_daemon_ipc,
        ctx.readonly_project_root,
        &ctx.args.extra_writable,
        &ctx.args.extra_readable,
        ctx.args.error_marker_scan_override(),
        ctx.args.resource_overrides(),
        ctx.current_depth,
        parent_startup_env,
    )
    .await;

    match result {
        Ok(result) => build_reviewer_outcome(reviewer_index, ctx.tool, &result),
        Err(err) => {
            let reason = reviewer_unavailable_error_reason(&err, ctx.tool)
                .unwrap_or_else(|| format!("synthesis review failed: {err:#}"));
            Ok(build_unavailable_reviewer_outcome(
                reviewer_index,
                ctx.tool,
                reason,
            ))
        }
    }
}

pub(super) fn build_chunk_review_instruction(
    base_prompt: &str,
    plan: &ReviewChunkPlan,
    chunk: &ReviewChunk,
    tool: ToolName,
    project_root: &Path,
    current_session_id: Option<&str>,
    diff_fingerprint: Option<&str>,
) -> String {
    let output_dir = reviewer_output_dir(chunk.id);
    let manifest = render_changed_file_manifest(&plan.chunks);
    let pathspecs = chunk.pathspecs.join("\n- ");
    let fingerprint = diff_fingerprint.unwrap_or("unavailable");
    let mut prompt = format!(
        "{base_prompt}\n\n\
## Chunked Review Scope\n\
You are chunk reviewer {id} of {total}. Review only findings whose concrete evidence is in this chunk.\n\
Original scope: {scope}\n\
Diff fingerprint: {fingerprint}\n\
Reviewer tool hint: {tool}\n\
Project root: {root}\n\
Parent session: {parent}\n\n\
Chunk group: {group}\n\
Chunk changed lines: {changed_lines}\n\
Chunk pathspecs:\n- {pathspecs}\n\n\
All changed files manifest:\n{manifest}\n\n\
Write review artifacts to {output_dir}/review-findings.json and {output_dir}/review-report.md.\n\
Report only findings with in-chunk file/line evidence. Do not report cross-chunk speculation.",
        id = chunk.id,
        total = plan.chunk_count(),
        scope = plan.scope,
        tool = tool.as_str(),
        root = project_root.display(),
        parent = current_session_id.unwrap_or("unknown"),
        group = chunk.group,
        changed_lines = chunk.changed_lines,
    );
    crate::review_design_anchor::append_design_anchor(&mut prompt);
    prompt
}

pub(super) fn build_synthesis_review_instruction(
    base_prompt: &str,
    plan: &ReviewChunkPlan,
    outcomes: &[ReviewerOutcome],
    consolidated: &ReviewArtifact,
    reviewer_index: usize,
    diff_fingerprint: Option<&str>,
) -> String {
    let output_dir = reviewer_output_dir(reviewer_index);
    let findings =
        serde_json::to_string_pretty(&consolidated.findings).unwrap_or_else(|_| "[]".to_string());
    let outcome_summary = outcomes
        .iter()
        .map(|outcome| {
            format!(
                "- chunk {} => {} ({})",
                outcome.reviewer_index + 1,
                outcome.verdict,
                outcome.session_id
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let manifest = render_changed_file_manifest(&plan.chunks);
    let fingerprint = diff_fingerprint.unwrap_or("unavailable");
    format!(
        "{base_prompt}\n\n\
## Chunked Review Synthesis\n\
You are the synthesis reviewer for {chunks} chunk reviews.\n\
Original scope: {scope}\n\
Diff fingerprint: {fingerprint}\n\n\
Chunk outcomes:\n{outcome_summary}\n\n\
All changed files manifest:\n{manifest}\n\n\
Deterministically consolidated chunk findings before synthesis:\n```json\n{findings}\n```\n\n\
Write review artifacts to {output_dir}/review-findings.json and {output_dir}/review-report.md.\n\
Add only cross-file findings backed by concrete file/line evidence from the changed files. \
Do not repeat findings already present in the consolidated list.",
        chunks = plan.chunk_count(),
        scope = plan.scope,
    )
}

pub(super) fn render_changed_file_manifest(chunks: &[ReviewChunk]) -> String {
    chunks
        .iter()
        .flat_map(|chunk| {
            chunk.files.iter().map(move |file| {
                format!(
                    "- chunk {} [{} +{}] {}",
                    chunk.id, file.status, file.changed_lines, file.path
                )
            })
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn reviewer_output_dir(reviewer_index: usize) -> String {
    let reviewer_dir = format!("reviewer-{reviewer_index}");
    format!("${{{CSA_PARENT_SESSION_DIR_ENV_KEY}:-${CSA_SESSION_DIR_ENV_KEY}}}/{reviewer_dir}")
}

pub(super) fn all_changed_files(plan: &ReviewChunkPlan) -> Vec<String> {
    plan.chunks
        .iter()
        .flat_map(|chunk| chunk.files.iter().map(|file| file.path.clone()))
        .collect()
}

pub(super) fn parent_startup_env_for_chunked_review(
    daemon_child: bool,
    session_id: Option<&str>,
    startup_env: &StartupSubtreeEnv,
    project_root: &Path,
) -> Result<StartupSubtreeEnv> {
    if daemon_child && let Some(session_id) = session_id {
        let session_dir = csa_session::get_session_dir(project_root, session_id)?;
        return Ok(startup_env
            .clone()
            .with_current_session(session_id, session_dir.display().to_string()));
    }

    Ok(startup_env.clone())
}

pub(super) fn load_consolidated_chunk_artifact(
    project_root: &Path,
    parent_session_dir: Option<&Path>,
    outcomes: &[ReviewerOutcome],
) -> Result<ReviewArtifact> {
    let mut artifacts = Vec::new();
    for outcome in outcomes {
        match load_chunk_artifact(project_root, parent_session_dir, outcome) {
            Ok(artifact) => artifacts.push(artifact),
            Err(error) if outcome.verdict == HAS_ISSUES => {
                return Err(error).with_context(|| {
                    format!(
                        "issue-bearing chunk {} ({}) has no usable findings artifact",
                        outcome.reviewer_index + 1,
                        outcome.session_id
                    )
                });
            }
            Err(_) => {}
        }
    }
    Ok(build_consolidated_artifact(artifacts, "chunked-review"))
}

pub(super) fn load_chunk_artifact(
    project_root: &Path,
    parent_session_dir: Option<&Path>,
    outcome: &ReviewerOutcome,
) -> Result<ReviewArtifact> {
    let reviewer_dir = format!("reviewer-{}", outcome.reviewer_index + 1);
    let mut candidates = Vec::new();
    if let Some(parent_session_dir) = parent_session_dir {
        candidates.push(
            parent_session_dir
                .join(&reviewer_dir)
                .join("review-findings.json"),
        );
    }
    if let Ok(session_dir) = csa_session::get_session_dir(project_root, &outcome.session_id) {
        candidates.push(session_dir.join(&reviewer_dir).join("review-findings.json"));
        candidates.push(session_dir.join("output").join("review-findings.json"));
    }
    for path in candidates {
        if !path.exists() {
            continue;
        }
        let content = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        return crate::review_cmd::parent_artifacts::parse_reviewer_artifact(&path, &content);
    }
    anyhow::bail!("no review-findings.json for {}", outcome.session_id)
}

pub(super) fn persist_chunk_session_meta(
    project_root: &Path,
    parent_session_dir: Option<&Path>,
    reviewer_index: usize,
    outcome: &ReviewerOutcome,
    meta: ReviewChunkSessionMeta,
) -> Result<()> {
    let payload = serde_json::to_vec_pretty(&meta).context("failed to serialize chunk metadata")?;
    if let Ok(session_dir) = csa_session::get_session_dir(project_root, &outcome.session_id) {
        let output_dir = session_dir.join("output");
        fs::create_dir_all(&output_dir)
            .with_context(|| format!("failed to create {}", output_dir.display()))?;
        fs::write(output_dir.join("chunked-review-meta.json"), &payload)
            .context("failed to write child chunk metadata")?;
    }
    if let Some(parent_session_dir) = parent_session_dir {
        let reviewer_dir = parent_session_dir.join(format!("reviewer-{}", reviewer_index + 1));
        fs::create_dir_all(&reviewer_dir)
            .with_context(|| format!("failed to create {}", reviewer_dir.display()))?;
        fs::write(reviewer_dir.join("chunked-review-meta.json"), payload)
            .context("failed to write parent chunk metadata")?;
    }
    Ok(())
}

pub(super) fn final_chunked_verdict(
    outcomes: &[ReviewerOutcome],
    all_unavailable: bool,
) -> &'static str {
    if all_unavailable {
        return UNAVAILABLE;
    }
    if outcomes
        .iter()
        .any(|outcome| !outcome.produced_usable_verdict())
    {
        return HAS_ISSUES;
    }
    if outcomes.iter().any(|outcome| outcome.verdict == HAS_ISSUES) {
        HAS_ISSUES
    } else {
        CLEAN
    }
}

pub(super) fn write_chunked_review_audit(
    project_root: &Path,
    startup_env: &StartupSubtreeEnv,
    plan: &ReviewChunkPlan,
    outcomes: &[ReviewerOutcome],
    diff_fingerprint: Option<String>,
    final_verdict: &str,
    fail_closed: bool,
) -> Result<()> {
    let session_dir = audit_session_dir(project_root, startup_env, outcomes);
    let output_dir = session_dir.join("output");
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;
    let chunks = plan
        .chunks
        .iter()
        .filter_map(|chunk| {
            outcomes
                .iter()
                .find(|outcome| outcome.reviewer_index + 1 == chunk.id)
                .map(|outcome| ChunkAuditEntry {
                    chunk_id: chunk.id,
                    group: chunk.group.clone(),
                    pathspecs: chunk.pathspecs.clone(),
                    changed_lines: chunk.changed_lines,
                    estimated_tokens: chunk.estimated_tokens,
                    session_id: outcome.session_id.clone(),
                    verdict: outcome.verdict.to_string(),
                })
        })
        .collect::<Vec<_>>();
    let synthesis_session_id = outcomes
        .iter()
        .find(|outcome| outcome.reviewer_index == plan.chunk_count())
        .map(|outcome| outcome.session_id.clone());
    let audit = ChunkedReviewAudit {
        schema_version: 1,
        scope: plan.scope.clone(),
        diff_fingerprint,
        activation_reason: plan.activation_reason,
        final_verdict: final_verdict.to_string(),
        fail_closed,
        chunks,
        synthesis_session_id,
        timestamp: chrono::Utc::now(),
    };
    let payload =
        serde_json::to_vec_pretty(&audit).context("failed to serialize chunked-review audit")?;
    fs::write(output_dir.join("chunked-review.json"), payload)
        .context("failed to write output/chunked-review.json")?;
    Ok(())
}

pub(super) fn audit_session_dir(
    project_root: &Path,
    startup_env: &StartupSubtreeEnv,
    outcomes: &[ReviewerOutcome],
) -> PathBuf {
    if let Some(session_dir) = startup_env.session_dir() {
        return PathBuf::from(session_dir);
    }
    outcomes
        .first()
        .and_then(|outcome| csa_session::get_session_dir(project_root, &outcome.session_id).ok())
        .unwrap_or_else(|| project_root.to_path_buf())
}
