//! Production clean-room provider adapter.

#![allow(
    dead_code,
    reason = "B5 Slice 3B1 exposes audited clean-room provider authority before orchestration dispatch"
)]

use anyhow::{Result, bail};
use csa_config::{GlobalConfig, ProjectConfig};

use crate::pipeline::{
    AdmittedExecutor, CleanRoomExecutionContract, CleanRoomExecutionLimits, ParentSessionSource,
    SessionCreationMode, execute_clean_room_session,
};

use super::clean_room::{
    ProviderSessionFactory, ProviderSessionFuture, ProviderSessionOutcome, ProviderSessionRequest,
};
use super::provider_command_authority::ProviderCommandAuthority;

/// Production adapter from audited authority to the pipeline clean-room API.
pub(crate) struct ProductionCleanRoomProvider<'a> {
    admitted: &'a AdmittedExecutor,
    authority: &'a ProviderCommandAuthority,
    project_config: Option<&'a ProjectConfig>,
    global_config: Option<&'a GlobalConfig>,
    limits: CleanRoomExecutionLimits,
}

impl<'a> ProductionCleanRoomProvider<'a> {
    pub(crate) fn new(
        admitted: &'a AdmittedExecutor,
        authority: &'a ProviderCommandAuthority,
        project_config: Option<&'a ProjectConfig>,
        global_config: Option<&'a GlobalConfig>,
        limits: CleanRoomExecutionLimits,
    ) -> Result<Self> {
        authority.validate_admitted_executor(admitted)?;
        Ok(Self {
            admitted,
            authority,
            project_config,
            global_config,
            limits,
        })
    }
}

impl ProviderSessionFactory for ProductionCleanRoomProvider<'_> {
    fn run<'a>(&'a mut self, request: &'a ProviderSessionRequest) -> ProviderSessionFuture<'a> {
        Box::pin(async move {
            self.authority.validate_request(self.admitted, request)?;
            validate_production_request_policy(request)?;
            let command = self
                .authority
                .clean_command_contract(self.admitted, request)?;
            let contract = CleanRoomExecutionContract::try_new(
                request.cwd(),
                request.evidence_bundle(),
                command,
            )?;
            let outcome = execute_clean_room_session(
                self.admitted,
                &self.authority.tool(),
                request.exact_prompt(),
                contract,
                self.project_config,
                self.global_config,
                self.limits.clone(),
            )
            .await?;
            if outcome.execution.exit_code != 0 {
                bail!(
                    "clean-room provider process exited with status {}",
                    outcome.execution.exit_code
                );
            }
            Ok(ProviderSessionOutcome::with_provider_turn_completion(
                &outcome.meta_session_id,
                outcome.execution.output.as_bytes(),
                outcome.execution.provider_turn_completion(),
            ))
        })
    }
}

fn validate_production_request_policy(request: &ProviderSessionRequest) -> Result<()> {
    if !request.readonly_project_root() {
        bail!("production clean-room provider requires a read-only project root");
    }
    if !request.extra_writable().is_empty() {
        bail!("production clean-room provider rejects extra writable paths");
    }
    if request.extra_readable() != [request.evidence_bundle().to_path_buf()] {
        bail!("production clean-room provider allows only the evidence bundle as extra readable");
    }
    if request.parent_session_source() != ParentSessionSource::ExplicitOnly
        || request.session_creation_mode() != SessionCreationMode::FreshChild
        || request.parent().is_some()
        || request.resume_session().is_some()
        || !request.startup_env().to_child_env_vars().is_empty()
    {
        bail!("production clean-room provider request violates fresh detached session policy");
    }
    Ok(())
}
