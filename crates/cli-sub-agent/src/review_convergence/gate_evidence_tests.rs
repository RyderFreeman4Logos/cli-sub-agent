use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, anyhow};
use csa_session::convergence::{EpochRecord, GitObjectId, Sha256Digest};

use super::gate_evidence::{
    FinalGateDriver, FinalGatePlan, FinalGateRunner, GateCommandOutcome, GateCommandSpec,
    ProductionFinalGateRunner,
};

fn epoch() -> EpochRecord {
    EpochRecord::new(
        GitObjectId::parse(&"a".repeat(40)).expect("base oid"),
        GitObjectId::parse(&"b".repeat(40)).expect("head oid"),
        Sha256Digest::compute(b"immutable diff"),
    )
}

fn command(root: &std::path::Path, name: &str, args: &[&str]) -> GateCommandSpec {
    GateCommandSpec::new(
        name,
        "just",
        args.iter().map(|arg| (*arg).to_string()).collect(),
        root,
    )
    .expect("gate command")
}

struct RecordingGateDriver {
    calls: Arc<Mutex<Vec<String>>>,
    outcomes: Vec<Result<GateCommandOutcome>>,
}

impl FinalGateDriver for RecordingGateDriver {
    fn run(&mut self, command: &GateCommandSpec) -> Result<GateCommandOutcome> {
        self.calls
            .lock()
            .expect("calls")
            .push(command.name().to_string());
        if self.outcomes.is_empty() {
            return Err(anyhow!("missing scripted outcome"));
        }
        self.outcomes.remove(0)
    }
}

#[test]
fn final_gate_evidence_retains_every_command_outcome_and_log() {
    let root = PathBuf::from("/tmp/csa-clean-room-tests/gates");
    let plan = FinalGatePlan::new(
        epoch(),
        &root,
        vec![
            command(&root, "format", &["fmt-check"]),
            command(&root, "test", &["test", "cli-sub-agent"]),
        ],
    )
    .expect("plan");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut runner = ProductionFinalGateRunner::new(RecordingGateDriver {
        calls: Arc::clone(&calls),
        outcomes: vec![
            Ok(GateCommandOutcome::new(0, b"fmt ok", b"")),
            Ok(GateCommandOutcome::new(0, b"tests ok", b"warning log")),
        ],
    });

    let evidence = runner.run(&plan).expect("gate evidence");

    let format = &plan.commands()[0];
    assert_eq!(format.program(), "just");
    assert_eq!(format.args(), &["fmt-check"]);
    assert_eq!(format.cwd(), root);
    assert_eq!(format.env().get("CI").map(String::as_str), Some("1"));
    assert_eq!(
        format.env().get("GIT_TERMINAL_PROMPT").map(String::as_str),
        Some("0")
    );
    assert_eq!(format.timeout(), Duration::from_secs(900));
    assert_eq!(calls.lock().expect("calls").as_slice(), &["format", "test"]);
    assert_eq!(evidence.epoch(), plan.epoch());
    assert_eq!(evidence.records().len(), 2);
    assert_eq!(evidence.records()[0].command(), &plan.commands()[0]);
    assert_eq!(evidence.records()[0].outcome().stdout(), b"fmt ok");
    assert_eq!(evidence.records()[1].outcome().stderr(), b"warning log");
    assert!(evidence.passed());
    evidence.require_success().expect("all gates passed");
}

#[test]
fn one_failed_required_gate_never_claims_success_but_retains_all_evidence() {
    let root = PathBuf::from("/tmp/csa-clean-room-tests/failure");
    let plan = FinalGatePlan::new(
        epoch(),
        &root,
        vec![
            command(&root, "test", &["test", "cli-sub-agent"]),
            command(&root, "lint", &["clippy-check"]),
        ],
    )
    .expect("plan");
    let mut runner = ProductionFinalGateRunner::new(RecordingGateDriver {
        calls: Arc::new(Mutex::new(Vec::new())),
        outcomes: vec![
            Ok(GateCommandOutcome::new(1, b"", b"test failed")),
            Ok(GateCommandOutcome::new(0, b"lint ok", b"")),
        ],
    });

    let evidence = runner.run(&plan).expect("completed evidence");

    assert_eq!(evidence.records().len(), 2);
    assert_eq!(evidence.records()[0].outcome().exit_code(), 1);
    assert!(!evidence.passed());
    let error = evidence
        .require_success()
        .expect_err("failure must remain fail-closed");
    assert!(error.to_string().contains("test"));
    assert_eq!(evidence.records()[0].outcome().stderr(), b"test failed");
}

#[test]
fn driver_error_rejects_incomplete_evidence_instead_of_claiming_pass() {
    let root = PathBuf::from("/tmp/csa-clean-room-tests/incomplete");
    let plan = FinalGatePlan::new(
        epoch(),
        &root,
        vec![command(&root, "test", &["test", "cli-sub-agent"])],
    )
    .expect("plan");
    let mut runner = ProductionFinalGateRunner::new(RecordingGateDriver {
        calls: Arc::new(Mutex::new(Vec::new())),
        outcomes: vec![Err(anyhow!("driver unavailable"))],
    });

    let error = runner
        .run(&plan)
        .expect_err("incomplete evidence must be rejected");
    assert!(format!("{error:#}").contains("driver unavailable"));
}

#[test]
fn final_gate_plan_rejects_empty_duplicate_or_out_of_room_commands() {
    let root = PathBuf::from("/tmp/csa-clean-room-tests/validation");
    let duplicate = command(&root, "test", &["test"]);
    let error = FinalGatePlan::new(epoch(), &root, vec![duplicate.clone(), duplicate])
        .expect_err("duplicate gates must fail");
    assert!(error.to_string().contains("duplicate"));

    let outside = command(
        std::path::Path::new("/tmp/outside"),
        "lint",
        &["clippy-check"],
    );
    let error =
        FinalGatePlan::new(epoch(), &root, vec![outside]).expect_err("outside cwd must fail");
    assert!(error.to_string().contains("clean-room root"));

    let error =
        FinalGatePlan::new(epoch(), &root, Vec::new()).expect_err("empty required gates must fail");
    assert!(error.to_string().contains("at least one"));
}

#[test]
fn gate_command_rejects_ambiguous_or_unbounded_process_contracts() {
    let root = PathBuf::from("/tmp/csa-clean-room-tests/command");
    assert!(GateCommandSpec::new("", "just", vec![], &root).is_err());
    assert!(GateCommandSpec::new("test", "", vec![], &root).is_err());
    assert!(GateCommandSpec::new("test", "just", vec!["bad\0arg".to_string()], &root).is_err());
    assert!(GateCommandSpec::new("test", "just", vec![], "relative/root").is_err());
}
