//! Production adapters for observe-only review convergence.
//!
//! ADK-Rust capability decision (Rust rule 020): this workspace supports Rust 1.88,
//! while adk-rust 1.0.0 requires 1.94. Its model/graph adapters also cannot preserve
//! CSA's admitted-executor identity, session/failover, read-only sandbox, and artifact
//! contracts. This slice therefore reuses CSA's executor instead of adding a second
//! runtime or raising the public MSRV.
//! The persisted page envelope labels this single observation cell as a non-exhaustive
//! walking skeleton.

use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Command, Output};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use csa_config::{EffectiveModelCatalog, GlobalConfig, ProjectConfig};
use csa_core::types::ToolName;
use csa_executor::ModelSpec;
use csa_process::{ProviderTurnCompletion, StreamMode};
use csa_session::{convergence::AdmittedModelIdentity, get_session_dir};

use crate::cli::ReviewArgs;
use crate::pipeline::SessionCreationMode;
use crate::review_routing::ReviewRoutingMetadata;
use crate::run_resource_overrides::RunResourceOverrides;
use crate::startup_env::StartupSubtreeEnv;

use super::engine::{
    DiscoveryRequest, DiscoveryRunOutput, DiscoveryRunner, FrozenWorkspace, WorkspaceProbe,
};

const PAGE_ARTIFACT_PATH: &str = "output/convergence-discovery-page.json";

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExecutionPolicy {
    pub(crate) fresh_session: bool,
    pub(crate) readonly_project_root: bool,
    pub(crate) extra_writable: Vec<PathBuf>,
    pub(crate) no_fs_sandbox: bool,
}

#[cfg(test)]
pub(crate) fn execution_policy() -> ExecutionPolicy {
    ExecutionPolicy {
        fresh_session: true,
        readonly_project_root: true,
        extra_writable: Vec::new(),
        no_fs_sandbox: false,
    }
}

pub(crate) struct GitWorkspaceProbe {
    project_root: PathBuf,
}

impl GitWorkspaceProbe {
    pub(crate) fn new(project_root: &Path) -> Self {
        Self {
            project_root: project_root.to_path_buf(),
        }
    }

    fn git(&self, args: &[&str]) -> Result<Output> {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.project_root)
            .args(args)
            .output()
            .with_context(|| format!("run git {}", args.join(" ")))?;
        if !output.status.success() {
            bail!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(output)
    }
}

impl WorkspaceProbe for GitWorkspaceProbe {
    fn capture(&mut self, range: &str) -> Result<FrozenWorkspace> {
        let base_ref = range
            .strip_suffix("...HEAD")
            .filter(|base| !base.is_empty() && !base.contains(".."))
            .context("range must be the exact form <base>...HEAD")?;
        let merge_base = self.git(&["merge-base", base_ref, "HEAD"])?;
        let base_oid = String::from_utf8(merge_base.stdout)
            .context("merge-base was not UTF-8")?
            .trim()
            .to_string();
        let head = self.git(&["rev-parse", "HEAD"])?;
        let head_oid = String::from_utf8(head.stdout)
            .context("HEAD was not UTF-8")?
            .trim()
            .to_string();
        let diff_range = format!("{base_oid}..{head_oid}");
        let diff = self.git(&[
            "diff",
            "--binary",
            "--full-index",
            "--no-ext-diff",
            &diff_range,
            "--",
        ])?;
        let status = self.git(&["status", "--porcelain=v1", "--untracked-files=normal"])?;
        let mut index_clean = true;
        let mut worktree_clean = true;
        for line in status.stdout.split(|byte| *byte == b'\n') {
            if line.len() < 2 {
                continue;
            }
            if line[0] == b'?' && line[1] == b'?' {
                worktree_clean = false;
                continue;
            }
            index_clean &= line[0] == b' ';
            worktree_clean &= line[1] == b' ';
        }
        FrozenWorkspace::new(
            &base_oid,
            &head_oid,
            csa_session::convergence::Sha256Digest::compute(&diff.stdout),
            index_clean,
            worktree_clean,
        )
    }
}

pub(crate) struct ProductionRunnerContext<'a> {
    pub(crate) tool: ToolName,
    pub(crate) model: Option<String>,
    pub(crate) tier_model_spec: Option<String>,
    pub(crate) tier_name: Option<String>,
    pub(crate) tier_fallback_enabled: bool,
    pub(crate) tier_preference_order: Vec<String>,
    pub(crate) thinking: Option<String>,
    pub(crate) project_root: &'a Path,
    pub(crate) project_config: Option<&'a ProjectConfig>,
    pub(crate) global_config: &'a GlobalConfig,
    pub(crate) model_catalog: &'a EffectiveModelCatalog,
    pub(crate) pre_session_hook: Option<csa_hooks::PreSessionHookInvocation>,
    pub(crate) review_routing: ReviewRoutingMetadata,
    pub(crate) stream_mode: csa_process::StreamMode,
    pub(crate) idle_timeout_seconds: u64,
    pub(crate) initial_response_timeout_seconds: Option<u64>,
    pub(crate) force_override_user_config: bool,
    pub(crate) force_ignore_tier_setting: bool,
    pub(crate) no_failover: bool,
    pub(crate) explicit_tool_with_failover: Option<ToolName>,
    pub(crate) build_jobs: Option<u32>,
    pub(crate) fast_but_more_cost: bool,
    pub(crate) warn_no_codex_fast_mode: bool,
    pub(crate) extra_readable: &'a [PathBuf],
    pub(crate) error_marker_scan_override: Option<bool>,
    pub(crate) resource_overrides: RunResourceOverrides,
    pub(crate) current_depth: u32,
    pub(crate) startup_env: &'a StartupSubtreeEnv,
    pub(crate) timeout_seconds: Option<u64>,
}

pub(crate) struct ResolvedCommandContext<'a> {
    pub(crate) project_root: &'a Path,
    pub(crate) args: &'a ReviewArgs,
    pub(crate) project_config: Option<&'a ProjectConfig>,
    pub(crate) global_config: &'a GlobalConfig,
    pub(crate) model_catalog: &'a EffectiveModelCatalog,
    pub(crate) pre_session_hook: Option<csa_hooks::PreSessionHookInvocation>,
    pub(crate) review_routing: ReviewRoutingMetadata,
    pub(crate) tool: ToolName,
    pub(crate) tier_model_spec: Option<String>,
    pub(crate) tier_name: Option<String>,
    pub(crate) tier_fallback_enabled: bool,
    pub(crate) tier_preference_order: Vec<String>,
    pub(crate) model: Option<String>,
    pub(crate) thinking: Option<String>,
    pub(crate) stream_mode: StreamMode,
    pub(crate) idle_timeout_seconds: u64,
    pub(crate) initial_response_timeout_seconds: Option<u64>,
    pub(crate) no_failover: bool,
    pub(crate) explicit_tool_with_failover: Option<ToolName>,
    pub(crate) current_depth: u32,
    pub(crate) startup_env: &'a StartupSubtreeEnv,
}

impl ResolvedCommandContext<'_> {
    pub(crate) fn runner_context(&self) -> ProductionRunnerContext<'_> {
        ProductionRunnerContext {
            tool: self.tool,
            model: self.model.clone(),
            tier_model_spec: self.tier_model_spec.clone(),
            tier_name: self.tier_name.clone(),
            tier_fallback_enabled: self.tier_fallback_enabled,
            tier_preference_order: self.tier_preference_order.clone(),
            thinking: self.thinking.clone(),
            project_root: self.project_root,
            project_config: self.project_config,
            global_config: self.global_config,
            model_catalog: self.model_catalog,
            pre_session_hook: self.pre_session_hook.clone(),
            review_routing: self.review_routing.clone(),
            stream_mode: self.stream_mode,
            idle_timeout_seconds: self.idle_timeout_seconds,
            initial_response_timeout_seconds: self.initial_response_timeout_seconds,
            force_override_user_config: self.args.force_override_user_config,
            force_ignore_tier_setting: self.args.force_ignore_tier_setting,
            no_failover: self.no_failover,
            explicit_tool_with_failover: self.explicit_tool_with_failover,
            build_jobs: self.args.build_jobs,
            fast_but_more_cost: self.args.fast_but_more_cost,
            warn_no_codex_fast_mode: true,
            extra_readable: &self.args.extra_readable,
            error_marker_scan_override: self.args.error_marker_scan_override(),
            resource_overrides: self.args.resource_overrides(),
            current_depth: self.current_depth,
            startup_env: self.startup_env,
            timeout_seconds: self.args.timeout,
        }
    }
}

pub(crate) struct ProductionDiscoveryRunner<'a> {
    context: ProductionRunnerContext<'a>,
}

impl<'a> ProductionDiscoveryRunner<'a> {
    pub(crate) fn new(context: ProductionRunnerContext<'a>) -> Self {
        Self { context }
    }

    async fn execute(&mut self, request: DiscoveryRequest) -> Result<DiscoveryRunOutput> {
        let prompt = build_discovery_prompt(&request);
        let context = &self.context;
        let future = crate::review_cmd::execute::execute_review_with_tier_filter(
            context.tool,
            prompt,
            None,
            context.model.clone(),
            context.tier_model_spec.clone(),
            context.tier_name.clone(),
            context.tier_fallback_enabled,
            context.tier_preference_order.clone(),
            context.thinking.clone(),
            format!("convergence discovery observation for {}", request.range),
            context.project_root,
            context.project_config,
            context.global_config,
            context.model_catalog,
            context.pre_session_hook.clone(),
            context.review_routing.clone(),
            context.stream_mode,
            context.idle_timeout_seconds,
            context.initial_response_timeout_seconds,
            context.force_override_user_config,
            context.force_ignore_tier_setting,
            context.no_failover,
            context.explicit_tool_with_failover,
            context.build_jobs,
            context.fast_but_more_cost,
            context.warn_no_codex_fast_mode,
            false,
            false,
            true,
            &[],
            context.extra_readable,
            context.error_marker_scan_override,
            context.resource_overrides,
            context.current_depth,
            SessionCreationMode::DaemonManaged,
            context.startup_env,
        );
        let outcome = if let Some(seconds) = context.timeout_seconds {
            tokio::time::timeout(Duration::from_secs(seconds), future)
                .await
                .context("convergence discovery reviewer timed out")??
        } else {
            future.await?
        };
        if outcome.execution.execution.exit_code != 0 || outcome.forced_decision.is_some() {
            bail!(
                "review execution did not complete successfully: exit={} reason={}",
                outcome.execution.execution.exit_code,
                outcome
                    .failure_reason
                    .as_deref()
                    .or(outcome.status_reason.as_deref())
                    .unwrap_or("unknown")
            );
        }
        let completion = outcome.execution.execution.provider_turn_completion();
        if completion != ProviderTurnCompletion::Natural {
            bail!("review provider turn was not naturally complete: {completion:?}");
        }
        let admitted_spec = outcome
            .routed_to
            .as_deref()
            .or(context.tier_model_spec.as_deref())
            .context("review adapter did not expose a full admitted model spec")?;
        let admitted = ModelSpec::parse(admitted_spec)
            .with_context(|| format!("parse admitted review model spec {admitted_spec}"))?;
        if admitted.tool != outcome.executed_tool.as_str() {
            bail!(
                "admitted model tool {} differs from executed tool {}",
                admitted.tool,
                outcome.executed_tool
            );
        }
        let identity = AdmittedModelIdentity::new(
            &admitted.tool,
            &admitted.provider,
            &admitted.model,
            &thinking_budget_label(&admitted.thinking_budget),
        )?;
        let session_id = outcome.execution.meta_session_id;
        let raw = outcome.execution.execution.output;
        let session_dir = get_session_dir(context.project_root, &session_id)?;
        let artifact_path = session_dir.join(PAGE_ARTIFACT_PATH);
        let parent = artifact_path
            .parent()
            .context("convergence artifact path has no parent")?;
        fs::create_dir_all(parent)?;
        let artifact = serde_json::to_vec(&serde_json::json!({
            "kind": "convergence_discovery_observation_page",
            "semantic_coverage": "walking_skeleton_not_exhaustive",
            "provider_response_raw": raw,
        }))?;
        fs::write(&artifact_path, &artifact)?;
        DiscoveryRunOutput::new_with_artifact_digest(
            raw,
            &session_id,
            completion,
            identity,
            PAGE_ARTIFACT_PATH,
            csa_session::convergence::Sha256Digest::compute(&artifact),
        )
    }
}

fn thinking_budget_label(budget: &csa_executor::ThinkingBudget) -> String {
    match budget {
        csa_executor::ThinkingBudget::DefaultBudget => "default".to_string(),
        csa_executor::ThinkingBudget::Low => "low".to_string(),
        csa_executor::ThinkingBudget::Medium => "medium".to_string(),
        csa_executor::ThinkingBudget::High => "high".to_string(),
        csa_executor::ThinkingBudget::Xhigh => "xhigh".to_string(),
        csa_executor::ThinkingBudget::Max => "max".to_string(),
        csa_executor::ThinkingBudget::Custom(value) => value.to_string(),
    }
}

impl DiscoveryRunner for ProductionDiscoveryRunner<'_> {
    fn run<'a>(
        &'a mut self,
        request: DiscoveryRequest,
    ) -> Pin<Box<dyn Future<Output = Result<DiscoveryRunOutput>> + 'a>> {
        Box::pin(self.execute(request))
    }
}

pub(crate) fn build_discovery_prompt(request: &DiscoveryRequest) -> String {
    let intent = match request.intent {
        csa_session::convergence::DiscoveryRunIntent::Initial => "initial broad discovery",
        csa_session::convergence::DiscoveryRunIntent::Continuation => {
            "continuation for hidden or unscanned candidates"
        }
        csa_session::convergence::DiscoveryRunIntent::SaturationChallenge => {
            "zero-new saturation challenge"
        }
    };
    let existing = if request.existing_fingerprints.is_empty() {
        String::new()
    } else {
        format!(
            "\nExisting semantic fingerprints (return only new candidates): {}",
            request.existing_fingerprints.join(",")
        )
    };
    format!(
        "Use the csa-review skill. Observe only; do not modify files. This is one Required whole-range broad-discovery walking-skeleton observation cell, not exhaustive semantic coverage.\nRange: {}\nFrozen merge-base: {}\nFrozen HEAD: {}\nFrozen diff SHA-256: {}\nRun intent: {intent}; finalized attempts: {}.{existing}\nReturn exact JSON or one complete json fence and no prose. Use exactly this schema: {{\"response_status\":\"complete|incomplete\",\"completion\":\"natural\",\"candidate_limit\":{},\"candidate_count\":0,\"more_candidates_possible\":false,\"unscanned_items\":[],\"candidates\":[{{\"mechanism\":\"...\",\"affected_component\":\"...\",\"bug_class\":\"...\"}}]}}. candidate_count must equal candidates length and never exceed candidate_limit.",
        request.range,
        request.frozen.base_oid,
        request.frozen.head_oid,
        request.frozen.diff_digest,
        request.prior_finalized_attempt_count,
        request.candidate_limit,
    )
}
