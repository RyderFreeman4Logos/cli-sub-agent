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
    convergence::{
        AdmittedModelIdentity, ArtifactEvidenceRef, CommandAuthoritySnapshot,
        ProviderEvidenceBundle,
    },
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
use super::schema::parse_discovery_page;

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
        let changed_paths = self.git(&[
            "diff",
            "--name-only",
            "-z",
            "--no-ext-diff",
            &base_oid,
            &head_oid,
            "--",
        ])?;
        let changed_paths = changed_paths
            .stdout
            .split(|byte| *byte == b'\0')
            .filter(|path| !path.is_empty())
            .map(|path| String::from_utf8(path.to_vec()).context("changed path was not UTF-8"))
            .collect::<Result<Vec<_>>>()?;
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
            changed_paths,
        )
    }
}

pub(crate) struct ProductionRunnerContext<'a> {
    pub(crate) command_authority: CommandAuthoritySnapshot,
    pub(crate) project_config: Option<&'a ProjectConfig>,
    pub(crate) global_config: &'a GlobalConfig,
    pub(crate) model_catalog: &'a EffectiveModelCatalog,
    pub(crate) pre_session_hook: Option<csa_hooks::PreSessionHookInvocation>,
    pub(crate) review_routing: ReviewRoutingMetadata,
    pub(crate) stream_mode: csa_process::StreamMode,
    pub(crate) idle_timeout_seconds: u64,
    pub(crate) initial_response_timeout_seconds: Option<u64>,
    pub(crate) force_override_user_config: bool,

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
    pub(crate) current_depth: u32,
    pub(crate) startup_env: &'a StartupSubtreeEnv,
}

impl ResolvedCommandContext<'_> {
    pub(crate) fn runner_context(
        &self,
        provider_bundle: ProviderEvidenceBundle,
        command_authority: CommandAuthoritySnapshot,
    ) -> ProductionRunnerContext<'_> {
        ProductionRunnerContext {
            command_authority,
            project_config: self.project_config,
            global_config: self.global_config,
            model_catalog: self.model_catalog,
            pre_session_hook: self.pre_session_hook.clone(),
            review_routing: self.review_routing.clone(),
            stream_mode: self.stream_mode,
            idle_timeout_seconds: self.idle_timeout_seconds,
            initial_response_timeout_seconds: self.initial_response_timeout_seconds,
            force_override_user_config: self.args.force_override_user_config,

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

    /// Transfer immutable command context after discovery has finished.
    #[must_use]
    pub(crate) fn into_context(self) -> ProductionRunnerContext<'a> {
        self.context
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
        let frozen_identity = context
            .command_authority
            .ordered_admitted()
            .first()
            .context("frozen command authority has no admitted executor")?;
        let frozen_tool = match frozen_identity.tool() {
            "gemini-cli" => ToolName::GeminiCli,
            "opencode" => ToolName::Opencode,
            "codex" => ToolName::Codex,
            "claude-code" => ToolName::ClaudeCode,
            "openai-compat" => ToolName::OpenaiCompat,
            "hermes" => ToolName::Hermes,
            "antigravity-cli" => ToolName::AntigravityCli,
            tool => bail!("unknown tool {tool} in frozen command authority"),
        };
        let frozen_spec = format!(
            "{}/{}/{}/{}",
            frozen_identity.tool(),
            frozen_identity.provider(),
            frozen_identity.model(),
            frozen_identity.reasoning()
        );
        let future = crate::review_cmd::execute::execute_review_with_tier_filter(
            frozen_tool,
            prompt,
            None,
            None,
            Some(frozen_spec.clone()),
            None,
            false,
            Vec::new(),
            None,
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
            context.command_authority.policy().force_ignore(),
            true,
            None,
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
            .or(Some(frozen_spec.as_str()))
            .context("review adapter did not expose a full admitted model spec")?;
        let identity = finalize_frozen_identity(
            &context.command_authority,
            admitted_spec,
            outcome.executed_tool.as_str(),
        )?;
        let session_id = outcome.execution.meta_session_id;
        let raw = outcome.execution.execution.output;
        let page = parse_discovery_page(&raw)
            .context("parse convergence discovery page before durable artifact publication")?;
        let session_dir = get_session_dir(&provider_root, &session_id)?;
        let artifact = encode_discovery_page_artifact(
            &raw,
            &page,
            &request.frozen.provider_evidence.identity,
        )?;
        publish_session_output_artifact(&session_dir, PAGE_ARTIFACT_FILE, &artifact)?;
        DiscoveryRunOutput::new_with_artifact_digest(
            raw,
            page,
            &session_id,
            completion,
            identity,
            PAGE_ARTIFACT_PATH,
            csa_session::convergence::Sha256Digest::compute(&artifact),
        )
    }
}

pub(crate) fn finalize_frozen_identity(
    authority: &CommandAuthoritySnapshot,
    admitted_spec: &str,
    executed_tool: &str,
) -> Result<AdmittedModelIdentity> {
    let admitted = ModelSpec::parse(admitted_spec)
        .with_context(|| format!("parse admitted review model spec {admitted_spec}"))?;
    if admitted.tool != executed_tool {
        bail!(
            "admitted model tool {} differs from executed tool {}",
            admitted.tool,
            executed_tool
        );
    }
    let identity = AdmittedModelIdentity::new(
        &admitted.tool,
        &admitted.provider,
        &admitted.model,
        &thinking_budget_label(&admitted.thinking_budget),
    )?;
    if !authority.contains(&identity) {
        bail!(
            "actual executor {admitted_spec} is outside frozen command authority {}",
            authority.digest()
        );
    }
    Ok(identity)
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
    let known_findings = if request.continuation.findings.is_empty() {
        "none".to_string()
    } else {
        request
            .continuation
            .findings
            .iter()
            .map(|finding| {
                format!(
                    "stable_id={}; violated_invariant={}; trigger_failure_mode={}; primary_component={}; bug_class={}",
                    finding.stable_finding_id.as_str(),
                    finding.semantic_identity.violated_invariant(),
                    finding.semantic_identity.trigger_failure_mode(),
                    finding.semantic_identity.primary_component(),
                    finding.semantic_identity.bug_class(),
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let latest_unscanned = if request
        .continuation
        .latest_finalized_unscanned_items
        .is_empty()
    {
        "none".to_string()
    } else {
        request
            .continuation
            .latest_finalized_unscanned_items
            .join(", ")
    };
    let uncovered_cells = if request.continuation.uncovered_cells.is_empty() {
        "none".to_string()
    } else {
        request
            .continuation
            .uncovered_cells
            .iter()
            .map(|cell| {
                format!(
                    "cell_id={}; scope={}={}; lens={}",
                    cell.id(),
                    cell.scope().kind(),
                    cell.scope().key(),
                    cell.lens().as_str(),
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let uncovered_items = if request.continuation.uncovered_items.is_empty() {
        "none".to_string()
    } else {
        request.continuation.uncovered_items.join(", ")
    };
    format!(
        "Use the csa-review skill. Observe only; do not modify files. The only review evidence is the immutable bundle file ./{}, and the checkout that created it is not available to you. Before reading evidence and again after finishing, run `sha256sum {}` and require SHA-256 {}. Use read-only commands such as `tar -tf {}`, `tar -xOf {} manifest.json`, `tar -xOf {} diff.patch`, and `tar -xOf {} source.tar | tar -tf -`; do not extract files to disk. This request covers one Required cell in a deterministic scope-by-semantic-lens manifest. Discovery evidence completes only after every required manifest cell has a fresh zero-new saturation page.\nRange label: {}\nExact merge-base OID: {}\nExact HEAD OID: {}\nExact diff SHA-256: {}\nCurrent manifest cell: id={}; scope={}={}; lens={}.\nRun intent: {intent}; finalized attempts: {}.\nPreviously reported semantic findings (return only genuinely new candidates):\n{}\nLatest finalized attempt unscanned items:\n{}\nUncovered manifest cells:\n{}\nUncovered manifest items:\n{}\nReturn exact JSON or one complete json fence and no prose. Use exactly this schema: {{\"schema_version\":1,\"kind\":\"convergence_discovery_page\",\"response_status\":\"complete|partial\",\"candidate_limit\":{},\"more_candidates_possible\":false,\"unscanned_items\":[],\"candidates\":[{{\"violated_invariant\":\"...\",\"trigger_failure_mode\":\"...\",\"primary_component\":\"...\",\"bug_class\":\"...\"}}]}}. The candidates array must not exceed candidate_limit. A complete page must have no continuation signals. A partial page must set more_candidates_possible or list at least one unscanned item.",
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
        request.cell.id(),
        request.cell.scope().kind(),
        request.cell.scope().key(),
        request.cell.lens().as_str(),
        request.prior_finalized_attempt_count,
        known_findings,
        latest_unscanned,
        uncovered_cells,
        uncovered_items,
        request.candidate_limit,
    )
}
