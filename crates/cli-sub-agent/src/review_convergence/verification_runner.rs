//! Production boundary for fresh, read-only candidate verifier sessions.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use csa_core::types::ToolName;
use csa_process::ProviderTurnCompletion;
use csa_session::convergence::{
    AdmittedModelIdentity, ArtifactEvidenceRef, CsaSessionId, SessionRelativeArtifactPath,
    Sha256Digest,
};
use csa_session::{get_session_dir, publish_session_output_artifact, read_session_output_artifact};

use crate::pipeline::SessionCreationMode;

use super::clustering::VerificationArtifactReader;
use super::runner::{ProductionRunnerContext, finalize_frozen_identity};
use super::verification::{
    CandidateVerificationOutput, CandidateVerificationRequest, CandidateVerifier,
    VERIFIER_ARTIFACT_FILE, VERIFIER_ARTIFACT_PATH, build_verifier_prompt,
    decode_verifier_artifact, encode_verifier_artifact, parse_verifier_page,
};

/// Executes isolated candidate verifiers without reusing discovery session state.
pub(crate) struct ProductionVerificationRunner<'a> {
    context: ProductionRunnerContext<'a>,
}

impl<'a> ProductionVerificationRunner<'a> {
    pub(crate) fn new(context: ProductionRunnerContext<'a>) -> Self {
        Self { context }
    }

    async fn execute(
        &mut self,
        request: CandidateVerificationRequest,
    ) -> Result<CandidateVerificationOutput> {
        if !request.policy.fresh_session
            || !request.policy.readonly_project_root
            || request.policy.resumes_discovery_state
            || request.policy.includes_discovery_transcript
        {
            bail!("candidate verifier request violates the independent read-only session policy");
        }
        let context = &self.context;
        let evidence = &request.frozen.provider_evidence;
        if context.provider_bundle.root() != evidence.root
            || context.provider_bundle.path() != evidence.path
            || context.provider_bundle.digest() != &evidence.identity.bundle_digest
        {
            bail!("candidate verifier request does not match the published immutable bundle");
        }
        context.provider_bundle.verify()?;
        let selected = &request.selected_verifier;
        let tool = tool_for_identity(selected)?;
        let model_spec = model_spec_for_identity(selected);
        let prompt = build_verifier_prompt(&request);
        let future = crate::review_cmd::execute::execute_review_with_tier_filter(
            tool,
            prompt,
            None,
            None,
            Some(model_spec.clone()),
            None,
            false,
            Vec::new(),
            None,
            format!(
                "convergence candidate verification for {}",
                request.candidate.id()
            ),
            &evidence.root,
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
            &[],
            context.error_marker_scan_override,
            context.resource_overrides,
            context.current_depth,
            SessionCreationMode::DaemonManaged,
            context.startup_env,
        );
        let outcome = if let Some(seconds) = context.timeout_seconds {
            tokio::time::timeout(Duration::from_secs(seconds), future)
                .await
                .context("candidate verifier timed out")??
        } else {
            future.await?
        };
        context.provider_bundle.verify()?;
        if outcome.execution.execution.exit_code != 0 || outcome.forced_decision.is_some() {
            bail!("candidate verifier did not complete successfully");
        }
        let completion = outcome.execution.execution.provider_turn_completion();
        if completion != ProviderTurnCompletion::Natural {
            bail!("candidate verifier provider turn was not naturally complete: {completion:?}");
        }
        let admitted_spec = outcome
            .routed_to
            .as_deref()
            .or(Some(model_spec.as_str()))
            .context("candidate verifier did not expose an admitted model spec")?;
        let identity = finalize_frozen_identity(
            &context.command_authority,
            admitted_spec,
            outcome.executed_tool.as_str(),
        )?;
        if identity != *selected {
            bail!("candidate verifier actual executor differs from the frozen selected identity");
        }
        let raw = outcome.execution.execution.output;
        let page = parse_verifier_page(&raw)
            .context("parse candidate verifier response before artifact publication")?;
        let session_id = outcome.execution.meta_session_id;
        let session_dir = get_session_dir(&evidence.root, &session_id)?;
        let artifact = encode_verifier_artifact(&raw, &page, &evidence.identity)?;
        let artifact_digest = Sha256Digest::compute(&artifact);
        if decode_verifier_artifact(&artifact, &artifact_digest, &evidence.identity)? != page {
            bail!("candidate verifier artifact round-trip changed the strict parsed response");
        }
        publish_session_output_artifact(&session_dir, VERIFIER_ARTIFACT_FILE, &artifact)?;
        Ok(CandidateVerificationOutput {
            page,
            actual_executor: identity,
            artifact: ArtifactEvidenceRef::new(
                CsaSessionId::parse(&session_id)?,
                SessionRelativeArtifactPath::new(VERIFIER_ARTIFACT_PATH)?,
                artifact_digest,
            ),
        })
    }
}

impl CandidateVerifier for ProductionVerificationRunner<'_> {
    fn verify<'a>(
        &'a mut self,
        request: CandidateVerificationRequest,
    ) -> Pin<Box<dyn Future<Output = Result<CandidateVerificationOutput>> + 'a>> {
        Box::pin(self.execute(request))
    }
}

impl VerificationArtifactReader for ProductionVerificationRunner<'_> {
    fn read_verifier_artifact<'a>(
        &'a mut self,
        artifact: &'a ArtifactEvidenceRef,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>>> + 'a>> {
        let result = (|| {
            self.context.provider_bundle.verify()?;
            let session_dir = get_session_dir(
                &self.context.provider_bundle.root(),
                artifact.csa_session_id().as_str(),
            )?;
            let file_name = artifact
                .path()
                .as_str()
                .strip_prefix("output/")
                .context("candidate verifier artifact is outside the output directory")?;
            read_session_output_artifact(&session_dir, file_name)
        })();
        Box::pin(async move { result })
    }
}

fn tool_for_identity(identity: &AdmittedModelIdentity) -> Result<ToolName> {
    match identity.tool() {
        "gemini-cli" => Ok(ToolName::GeminiCli),
        "opencode" => Ok(ToolName::Opencode),
        "codex" => Ok(ToolName::Codex),
        "claude-code" => Ok(ToolName::ClaudeCode),
        "openai-compat" => Ok(ToolName::OpenaiCompat),
        "hermes" => Ok(ToolName::Hermes),
        "antigravity-cli" => Ok(ToolName::AntigravityCli),
        tool => bail!("unknown tool {tool} in frozen command authority"),
    }
}

fn model_spec_for_identity(identity: &AdmittedModelIdentity) -> String {
    format!(
        "{}/{}/{}/{}",
        identity.tool(),
        identity.provider(),
        identity.model(),
        identity.reasoning()
    )
}
