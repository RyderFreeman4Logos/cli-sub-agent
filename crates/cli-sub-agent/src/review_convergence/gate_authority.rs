//! High-trust authority and exact plans for host final gates.
//!
//! Project configuration can describe a proposed gate set, but it cannot add,
//! reorder, or alter commands: [`FinalGatePlan::from_authority`] accepts only
//! the exact sequence captured by the host authority.

#![expect(
    dead_code,
    reason = "Task 8 defines the authority contract before Task 10 wires production completion ports"
)]

use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use anyhow::{Result, bail};
use csa_session::convergence::{Sha256Digest, WorkspaceLeaseIdentity};
use serde::{Deserialize, Serialize};

const FINAL_GATE_PLAN_SCHEMA_VERSION: u32 = 1;
const MAX_GATE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const MAX_GATE_COMMANDS: usize = 16;
const MAX_GATE_ARGV_ITEMS: usize = 64;
const MAX_COMMAND_ID_BYTES: usize = 128;
const MAX_PROGRAM_BYTES: usize = 1_024;
const MAX_ARG_BYTES: usize = 4 * 1_024;
const MAX_AUTHORITY_VERSION_BYTES: usize = 128;

/// The network boundary the host driver must enforce for a gate command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GateNetworkPolicy {
    Denied,
    AllowedByAuthority,
}

/// The credential boundary the host driver must enforce for a gate command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GateCredentialPolicy {
    Denied,
}

/// One exact argv command admitted by the high-trust final-gate authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct GateCommandAuthority {
    command_id: String,
    program: String,
    argv: Vec<String>,
    network: GateNetworkPolicy,
    credentials: GateCredentialPolicy,
    timeout_seconds: u64,
}

impl GateCommandAuthority {
    pub(crate) fn new(
        command_id: &str,
        program: &str,
        argv: Vec<String>,
        network: GateNetworkPolicy,
        timeout: Duration,
    ) -> Result<Self> {
        validate_component("gate command ID", command_id, MAX_COMMAND_ID_BYTES)?;
        validate_component("gate program", program, MAX_PROGRAM_BYTES)?;
        reject_shell_program(program)?;
        if argv.len() > MAX_GATE_ARGV_ITEMS {
            bail!("gate argv exceeds its maximum of {MAX_GATE_ARGV_ITEMS} items");
        }
        for argument in &argv {
            validate_component("gate argv component", argument, MAX_ARG_BYTES)?;
        }
        let timeout_seconds = timeout.as_secs();
        if timeout_seconds == 0 || timeout > MAX_GATE_TIMEOUT {
            bail!(
                "gate timeout must be between one second and {} seconds",
                MAX_GATE_TIMEOUT.as_secs()
            );
        }
        Ok(Self {
            command_id: command_id.to_string(),
            program: program.to_string(),
            argv,
            network,
            credentials: GateCredentialPolicy::Denied,
            timeout_seconds,
        })
    }

    pub(crate) fn command_id(&self) -> &str {
        &self.command_id
    }

    pub(crate) fn program(&self) -> &str {
        &self.program
    }

    pub(crate) fn argv(&self) -> &[String] {
        &self.argv
    }

    pub(crate) fn network(&self) -> GateNetworkPolicy {
        self.network
    }

    pub(crate) fn credentials(&self) -> GateCredentialPolicy {
        self.credentials
    }

    pub(crate) fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_seconds)
    }
}

/// Captured high-trust authority for the complete ordered final-gate sequence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct FinalGateAuthority {
    version: String,
    commands: Vec<GateCommandAuthority>,
}

impl FinalGateAuthority {
    pub(crate) fn new(version: &str, commands: Vec<GateCommandAuthority>) -> Result<Self> {
        validate_component(
            "final-gate authority version",
            version,
            MAX_AUTHORITY_VERSION_BYTES,
        )?;
        if commands.is_empty() {
            bail!("final-gate authority requires at least one command");
        }
        if commands.len() > MAX_GATE_COMMANDS {
            bail!("final-gate authority exceeds its maximum of {MAX_GATE_COMMANDS} commands");
        }
        let mut command_ids = HashSet::new();
        for command in &commands {
            if !command_ids.insert(command.command_id.clone()) {
                bail!(
                    "final-gate authority contains duplicate command ID '{}'",
                    command.command_id
                );
            }
        }
        Ok(Self {
            version: version.to_string(),
            commands,
        })
    }

    pub(crate) fn version(&self) -> &str {
        &self.version
    }

    pub(crate) fn commands(&self) -> &[GateCommandAuthority] {
        &self.commands
    }

    /// Domain-separated digest for binding the selected host command authority.
    pub(crate) fn digest(&self) -> Sha256Digest {
        let bytes = serde_json::to_vec(self).expect("gate authority serialization is infallible");
        let mut payload = b"csa-convergence-final-gate-authority-v1\0".to_vec();
        payload.extend_from_slice(&bytes);
        Sha256Digest::compute(&payload)
    }
}

/// An exact, host-authorized plan for one leased campaign epoch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FinalGatePlan {
    schema_version: u32,
    policy_digest: Sha256Digest,
    command_authority_digest: Sha256Digest,
    authority_version: String,
    lease: WorkspaceLeaseIdentity,
    commands: Vec<GateCommandAuthority>,
    minimal_env: BTreeMap<String, String>,
}

impl FinalGatePlan {
    /// Form a plan only when a lower-trust proposal is byte-for-byte equal to
    /// the captured high-trust command authority, including sequence order.
    pub(crate) fn from_authority(
        policy_digest: Sha256Digest,
        lease: WorkspaceLeaseIdentity,
        authority: &FinalGateAuthority,
        proposed_commands: Vec<GateCommandAuthority>,
    ) -> Result<Self> {
        if proposed_commands != authority.commands {
            bail!(
                "project final-gate command proposal differs from the high-trust command authority"
            );
        }
        Ok(Self {
            schema_version: FINAL_GATE_PLAN_SCHEMA_VERSION,
            policy_digest,
            command_authority_digest: authority.digest(),
            authority_version: authority.version.clone(),
            lease,
            commands: authority.commands.clone(),
            minimal_env: minimal_gate_environment(),
        })
    }

    pub(crate) fn policy_digest(&self) -> &Sha256Digest {
        &self.policy_digest
    }

    pub(crate) fn command_authority_digest(&self) -> &Sha256Digest {
        &self.command_authority_digest
    }

    pub(crate) fn authority_version(&self) -> &str {
        &self.authority_version
    }

    pub(crate) fn lease(&self) -> &WorkspaceLeaseIdentity {
        &self.lease
    }

    pub(crate) fn commands(&self) -> &[GateCommandAuthority] {
        &self.commands
    }

    pub(crate) fn minimal_env(&self) -> &BTreeMap<String, String> {
        &self.minimal_env
    }

    pub(crate) fn schema_version(&self) -> u32 {
        self.schema_version
    }
}

fn minimal_gate_environment() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("CI".to_string(), "1".to_string()),
        ("GIT_CONFIG_NOSYSTEM".to_string(), "1".to_string()),
        ("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()),
        ("LC_ALL".to_string(), "C".to_string()),
        (
            "PATH".to_string(),
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string(),
        ),
    ])
}

fn reject_shell_program(program: &str) -> Result<()> {
    let executable = program.rsplit('/').next().unwrap_or(program);
    if matches!(executable, "sh" | "bash" | "dash" | "zsh" | "fish") {
        bail!("final-gate commands must use a direct argv program, not a shell");
    }
    Ok(())
}

fn validate_component(label: &str, value: &str, maximum_bytes: usize) -> Result<()> {
    if value.is_empty() || value.contains('\0') || value.len() > maximum_bytes {
        bail!("{label} must be nonempty, NUL-free, and within its byte quota");
    }
    Ok(())
}
