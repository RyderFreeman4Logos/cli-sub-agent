//! Final-gate execution port and fail-closed evidence aggregation.
//!
//! This module never spawns a subprocess itself. A production caller supplies a driver; tests use
//! deterministic recording drivers so each required command, outcome, and log is mechanically
//! retained without touching a live workspace.

#![expect(
    dead_code,
    reason = "B5 Slice 2 defines isolated ports; production orchestration is wired in a later slice"
)]

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use csa_session::convergence::EpochRecord;

/// Exact bounded command contract for one required final gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GateCommandSpec {
    name: String,
    program: String,
    args: Vec<String>,
    cwd: PathBuf,
    env: BTreeMap<String, String>,
    timeout: Duration,
}

impl GateCommandSpec {
    pub(crate) fn new(
        name: &str,
        program: &str,
        args: Vec<String>,
        cwd: impl AsRef<Path>,
    ) -> Result<Self> {
        validate_component("gate name", name)?;
        validate_component("gate program", program)?;
        for argument in &args {
            validate_component("gate argument", argument)?;
        }
        let cwd = cwd.as_ref();
        if !cwd.is_absolute() {
            bail!("gate cwd must be an absolute path: {}", cwd.display());
        }
        let cwd_text = cwd
            .to_str()
            .with_context(|| format!("gate cwd must be valid UTF-8: {}", cwd.display()))?;
        validate_component("gate cwd", cwd_text)?;
        Ok(Self {
            name: name.to_string(),
            program: program.to_string(),
            args,
            cwd: cwd.to_path_buf(),
            env: BTreeMap::from([
                ("CI".to_string(), "1".to_string()),
                ("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()),
            ]),
            timeout: Duration::from_secs(900),
        })
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn program(&self) -> &str {
        &self.program
    }

    pub(crate) fn args(&self) -> &[String] {
        &self.args
    }

    pub(crate) fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub(crate) fn env(&self) -> &BTreeMap<String, String> {
        &self.env
    }

    pub(crate) fn timeout(&self) -> Duration {
        self.timeout
    }
}

/// Required gates bound to one immutable exact-OID epoch and one clean-room root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FinalGatePlan {
    epoch: EpochRecord,
    clean_room_root: PathBuf,
    commands: Vec<GateCommandSpec>,
}

impl FinalGatePlan {
    pub(crate) fn new(
        epoch: EpochRecord,
        clean_room_root: impl AsRef<Path>,
        commands: Vec<GateCommandSpec>,
    ) -> Result<Self> {
        epoch.validate().context("validate final-gate epoch")?;
        let clean_room_root = clean_room_root.as_ref();
        if !clean_room_root.is_absolute() {
            bail!(
                "final-gate clean-room root must be absolute: {}",
                clean_room_root.display()
            );
        }
        if commands.is_empty() {
            bail!("final-gate plan requires at least one gate command");
        }
        let mut names = HashSet::new();
        for command in &commands {
            if command.cwd != clean_room_root {
                bail!(
                    "final gate '{}' cwd {} differs from clean-room root {}",
                    command.name,
                    command.cwd.display(),
                    clean_room_root.display()
                );
            }
            if !names.insert(command.name.clone()) {
                bail!(
                    "final-gate plan contains duplicate gate '{}': command identities must be unique",
                    command.name
                );
            }
        }
        Ok(Self {
            epoch,
            clean_room_root: clean_room_root.to_path_buf(),
            commands,
        })
    }

    pub(crate) fn epoch(&self) -> &EpochRecord {
        &self.epoch
    }

    pub(crate) fn commands(&self) -> &[GateCommandSpec] {
        &self.commands
    }

    pub(crate) fn clean_room_root(&self) -> &Path {
        &self.clean_room_root
    }
}

/// Complete process outcome, including the logs required to explain a failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GateCommandOutcome {
    exit_code: i32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl GateCommandOutcome {
    pub(crate) fn new(exit_code: i32, stdout: &[u8], stderr: &[u8]) -> Self {
        Self {
            exit_code,
            stdout: stdout.to_vec(),
            stderr: stderr.to_vec(),
        }
    }

    pub(crate) fn exit_code(&self) -> i32 {
        self.exit_code
    }

    pub(crate) fn stdout(&self) -> &[u8] {
        &self.stdout
    }

    pub(crate) fn stderr(&self) -> &[u8] {
        &self.stderr
    }
}

/// One immutable command/outcome/log pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GateEvidenceRecord {
    command: GateCommandSpec,
    outcome: GateCommandOutcome,
}

impl GateEvidenceRecord {
    pub(crate) fn command(&self) -> &GateCommandSpec {
        &self.command
    }

    pub(crate) fn outcome(&self) -> &GateCommandOutcome {
        &self.outcome
    }
}

/// Complete evidence for every gate required by a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FinalGateEvidence {
    epoch: EpochRecord,
    required_gate_count: usize,
    records: Vec<GateEvidenceRecord>,
}

impl FinalGateEvidence {
    fn complete(plan: &FinalGatePlan, records: Vec<GateEvidenceRecord>) -> Result<Self> {
        if records.len() != plan.commands.len() {
            bail!(
                "final-gate evidence is incomplete: expected {} outcomes, retained {}",
                plan.commands.len(),
                records.len()
            );
        }
        for (expected, actual) in plan.commands.iter().zip(&records) {
            if expected != &actual.command {
                bail!("final-gate evidence command order or identity changed");
            }
        }
        Ok(Self {
            epoch: plan.epoch.clone(),
            required_gate_count: plan.commands.len(),
            records,
        })
    }

    pub(crate) fn epoch(&self) -> &EpochRecord {
        &self.epoch
    }

    pub(crate) fn records(&self) -> &[GateEvidenceRecord] {
        &self.records
    }

    pub(crate) fn passed(&self) -> bool {
        self.records.len() == self.required_gate_count
            && self
                .records
                .iter()
                .all(|record| record.outcome.exit_code == 0)
    }

    pub(crate) fn require_success(&self) -> Result<()> {
        if self.records.len() != self.required_gate_count {
            bail!("final-gate evidence is incomplete");
        }
        let failures = self
            .records
            .iter()
            .filter(|record| record.outcome.exit_code != 0)
            .map(|record| format!("{}={}", record.command.name, record.outcome.exit_code))
            .collect::<Vec<_>>();
        if !failures.is_empty() {
            bail!("required final gates failed: {}", failures.join(", "));
        }
        Ok(())
    }
}

/// Injected command execution boundary.
pub(crate) trait FinalGateDriver {
    fn run(&mut self, command: &GateCommandSpec) -> Result<GateCommandOutcome>;
}

pub(crate) trait FinalGateRunner {
    fn run(&mut self, plan: &FinalGatePlan) -> Result<FinalGateEvidence>;
}

/// Production evidence adapter; only its injected driver can execute commands.
pub(crate) struct ProductionFinalGateRunner<D> {
    driver: D,
}

impl<D> ProductionFinalGateRunner<D> {
    pub(crate) fn new(driver: D) -> Self {
        Self { driver }
    }
}

impl<D: FinalGateDriver> FinalGateRunner for ProductionFinalGateRunner<D> {
    fn run(&mut self, plan: &FinalGatePlan) -> Result<FinalGateEvidence> {
        let mut records = Vec::with_capacity(plan.commands.len());
        for command in &plan.commands {
            let outcome = self.driver.run(command).with_context(|| {
                format!("final gate '{}' did not produce an outcome", command.name)
            })?;
            records.push(GateEvidenceRecord {
                command: command.clone(),
                outcome,
            });
        }
        FinalGateEvidence::complete(plan, records)
    }
}

fn validate_component(label: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        bail!("{label} must not be empty");
    }
    if value.contains('\0') {
        bail!("{label} must not contain NUL");
    }
    Ok(())
}
