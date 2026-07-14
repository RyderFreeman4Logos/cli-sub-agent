//! Production adapters for observe-only review convergence.
//!
//! ADK-Rust capability decision (Rust rule 020): this workspace supports Rust 1.88,
//! while adk-rust 1.0.0 requires 1.94. Its model/graph adapters also cannot preserve
//! CSA's admitted-executor identity, session/failover, read-only sandbox, and artifact
//! contracts. This slice therefore reuses CSA's executor instead of adding a second
//! runtime or raising the public MSRV.
//! The persisted page envelope labels this single observation cell as a non-exhaustive
//! walking skeleton.

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
use csa_session::{
    convergence::{AdmittedModelIdentity, ArtifactEvidenceRef, ProviderEvidenceBundle},
    get_session_dir, publish_session_output_artifact, read_session_output_artifact,
};

use crate::cli::ReviewArgs;
use crate::pipeline::SessionCreationMode;
use crate::review_routing::ReviewRoutingMetadata;
use crate::run_resource_overrides::RunResourceOverrides;
use crate::startup_env::StartupSubtreeEnv;

use super::bundle::ProviderEvidenceRef;
use super::engine::{
    DiscoveryRequest, DiscoveryRunOutput, DiscoveryRunner, FrozenWorkspace, WorkspaceProbe,
};
use super::output::encode_discovery_page_artifact;

const PAGE_ARTIFACT_PATH: &str = "output/convergence-discovery-page.json";
const PAGE_ARTIFACT_FILE: &str = "convergence-discovery-page.json";

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderInput {
    pub(crate) project_root: PathBuf,
    pub(crate) bundle_path: PathBuf,
    pub(crate) bundle_digest: csa_session::convergence::Sha256Digest,
    pub(crate) extra_readable: Vec<PathBuf>,
}

pub(crate) fn provider_input(request: &DiscoveryRequest) -> ProviderInput {
    ProviderInput {
        project_root: request.frozen.provider_evidence.root.clone(),
        bundle_path: request.frozen.provider_evidence.path.clone(),
        bundle_digest: request
            .frozen
            .provider_evidence
            .identity
            .bundle_digest
            .clone(),
        extra_readable: Vec::new(),
    }
}

pub(crate) struct GitWorkspaceProbe {
    project_root: PathBuf,
    provider_evidence: ProviderEvidenceRef,
}

impl GitWorkspaceProbe {
    pub(crate) fn new(project_root: &Path, provider_evidence: ProviderEvidenceRef) -> Self {
        Self {
            project_root: project_root.to_path_buf(),
            provider_evidence,
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
        let head = self.git(&["rev-parse", "--verify", "HEAD^{commit}"])?;
        let head_oid = String::from_utf8(head.stdout)
            .context("HEAD was not UTF-8")?
            .trim()
            .to_string();
        let merge_base = self.git(&["merge-base", base_ref, &head_oid])?;
        let base_oid = String::from_utf8(merge_base.stdout)
            .context("merge-base was not UTF-8")?
            .trim()
            .to_string();
        let diff = self.git(&[
            "diff",
            "--binary",
            "--full-index",
            "--no-ext-diff",
            &base_oid,
            &head_oid,
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
        FrozenWorkspace::new_with_provider_evidence(
            &base_oid,
            &head_oid,
            csa_session::convergence::Sha256Digest::compute(&diff.stdout),
            index_clean,
            worktree_clean,
            self.provider_evidence.clone(),
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

    pub(crate) error_marker_scan_override: Option<bool>,
    pub(crate) resource_overrides: RunResourceOverrides,
    pub(crate) current_depth: u32,
    pub(crate) startup_env: &'a StartupSubtreeEnv,
    pub(crate) timeout_seconds: Option<u64>,
    pub(crate) provider_bundle: ProviderEvidenceBundle,
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
    pub(crate) fn runner_context(
        &self,
        provider_bundle: ProviderEvidenceBundle,
    ) -> ProductionRunnerContext<'_> {
        ProductionRunnerContext {
            tool: self.tool,
            model: self.model.clone(),
            tier_model_spec: self.tier_model_spec.clone(),
            tier_name: self.tier_name.clone(),
            tier_fallback_enabled: self.tier_fallback_enabled,
            tier_preference_order: self.tier_preference_order.clone(),
            thinking: self.thinking.clone(),
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

            error_marker_scan_override: self.args.error_marker_scan_override(),
            resource_overrides: self.args.resource_overrides(),
            current_depth: self.current_depth,
            startup_env: self.startup_env,
            timeout_seconds: self.args.timeout,
            provider_bundle,
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
        let provider_input = provider_input(&request);
        let context = &self.context;
        if context.provider_bundle.root() != provider_input.project_root
            || context.provider_bundle.path() != provider_input.bundle_path
            || context.provider_bundle.digest() != &provider_input.bundle_digest
        {
            bail!("provider evidence request does not match the published immutable bundle");
        }
        context.provider_bundle.verify()?;
        let provider_root = provider_input.project_root;
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
            &provider_root,
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
            &provider_input.extra_readable,
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
        context.provider_bundle.verify()?;
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
        let session_dir = get_session_dir(&provider_root, &session_id)?;
        let artifact =
            encode_discovery_page_artifact(&raw, &request.frozen.provider_evidence.identity)?;
        publish_session_output_artifact(&session_dir, PAGE_ARTIFACT_FILE, &artifact)?;
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

    fn read_artifact<'a>(
        &'a mut self,
        artifact: &'a ArtifactEvidenceRef,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>>> + 'a>> {
        let result = (|| {
            let session_id = artifact.csa_session_id().to_string();
            self.context.provider_bundle.verify()?;
            let session_dir = get_session_dir(&self.context.provider_bundle.root(), &session_id)?;
            let file_name = artifact
                .path()
                .as_str()
                .strip_prefix("output/")
                .context("convergence artifact is outside the output directory")?;
            read_session_output_artifact(&session_dir, file_name)
        })();
        Box::pin(async move { result })
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
        "Use the csa-review skill. Observe only; do not modify files. The only review evidence is the immutable bundle file ./{}, and the checkout that created it is not available to you. Before reading evidence and again after finishing, run `sha256sum {}` and require SHA-256 {}. Use read-only commands such as `tar -tf {}`, `tar -xOf {} manifest.json`, `tar -xOf {} diff.patch`, and `tar -xOf {} source.tar | tar -tf -`; do not extract files to disk. This is one Required whole-range broad-discovery walking-skeleton observation cell, not exhaustive semantic coverage.\nRange label: {}\nExact merge-base OID: {}\nExact HEAD OID: {}\nExact diff SHA-256: {}\nRun intent: {intent}; finalized attempts: {}.{existing}\nReturn exact JSON or one complete json fence and no prose. Use exactly this schema: {{\"schema_version\":1,\"kind\":\"convergence_discovery_page\",\"response_status\":\"complete|partial\",\"candidate_limit\":{},\"more_candidates_possible\":false,\"unscanned_items\":[],\"candidates\":[{{\"mechanism\":\"...\",\"affected_component\":\"...\",\"bug_class\":\"...\"}}]}}. The candidates array must not exceed candidate_limit. A complete page must have no continuation signals. A partial page must set more_candidates_possible or list at least one unscanned item.",
        request.frozen.provider_evidence.identity.bundle_file,
        request.frozen.provider_evidence.identity.bundle_file,
        request.frozen.provider_evidence.identity.bundle_digest,
        request.frozen.provider_evidence.identity.bundle_file,
        request.frozen.provider_evidence.identity.bundle_file,
        request.frozen.provider_evidence.identity.bundle_file,
        request.frozen.provider_evidence.identity.bundle_file,
        request.range,
        request.frozen.base_oid,
        request.frozen.head_oid,
        request.frozen.diff_digest,
        request.prior_finalized_attempt_count,
        request.candidate_limit,
    )
}
