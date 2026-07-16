use std::collections::VecDeque;
use std::time::Duration;

use anyhow::{Result, anyhow};
use csa_session::convergence::{
    CampaignId, CsaSessionId, EpochRecord, GitObjectId, SessionRelativeArtifactPath, Sha256Digest,
    WorkspaceLeaseIdentity,
};
use ulid::Ulid;

use super::gate_authority::{
    FinalGateAuthority, FinalGatePlan, GateCommandAuthority, GateNetworkPolicy,
};
use super::gate_evidence::{
    FinalGateDriver, FinalGateLease, GateInvocation, GateProcessOutcome, GateProcessTermination,
    HostFinalGatePort, HostGateArtifactStore,
};

fn epoch() -> EpochRecord {
    EpochRecord::new(
        GitObjectId::parse(&"a".repeat(40)).unwrap(),
        GitObjectId::parse(&"b".repeat(40)).unwrap(),
        Sha256Digest::compute(b"diff"),
    )
}

#[derive(Clone)]
struct Lease {
    identity: WorkspaceLeaseIdentity,
    valid: bool,
}
impl FinalGateLease for Lease {
    fn identity(&self) -> &WorkspaceLeaseIdentity {
        &self.identity
    }
    fn validate_current(&self) -> Result<()> {
        if self.valid {
            Ok(())
        } else {
            Err(anyhow!("inode changed"))
        }
    }
}

fn lease(root: &std::path::Path) -> Lease {
    Lease {
        identity: WorkspaceLeaseIdentity::new(
            CampaignId::generate(),
            epoch(),
            1,
            root.to_path_buf(),
            1,
            1,
            Ulid::new().to_string(),
        )
        .unwrap(),
        valid: true,
    }
}

fn command(id: &str, args: &[&str]) -> GateCommandAuthority {
    GateCommandAuthority::new(
        id,
        "just",
        args.iter().map(|item| (*item).to_string()).collect(),
        GateNetworkPolicy::Denied,
        Duration::from_secs(30),
    )
    .unwrap()
}

fn plan(lease: &Lease) -> FinalGatePlan {
    let authority = FinalGateAuthority::new(
        "global-gates-v2",
        vec![
            command("format", &["fmt-check"]),
            command("test", &["test"]),
        ],
    )
    .unwrap();
    FinalGatePlan::from_authority(
        Sha256Digest::compute(b"policy"),
        lease.identity.clone(),
        &authority,
        authority.commands().to_vec(),
    )
    .unwrap()
}

struct Driver {
    calls: Vec<GateInvocation>,
    outcomes: VecDeque<Result<GateProcessOutcome>>,
}
impl FinalGateDriver for Driver {
    fn run(&mut self, invocation: &GateInvocation) -> Result<GateProcessOutcome> {
        self.calls.push(invocation.clone());
        self.outcomes
            .pop_front()
            .unwrap_or_else(|| Err(anyhow!("unexpected gate")))
    }
}

fn store(root: &std::path::Path) -> HostGateArtifactStore {
    HostGateArtifactStore::new(
        root,
        CsaSessionId::parse("01ARZ3NDEKTSV4RRFFQ69G5FC5").unwrap(),
        SessionRelativeArtifactPath::new("gates").unwrap(),
    )
    .unwrap()
}

#[test]
fn success_is_ordered_minimal_and_authority_bound() {
    let temp = tempfile::tempdir().unwrap();
    let lease = lease(temp.path());
    let mut port = HostFinalGatePort::new(
        Driver {
            calls: Vec::new(),
            outcomes: VecDeque::from([
                Ok(GateProcessOutcome::new(
                    GateProcessTermination::Exited(0),
                    b"fmt ok",
                    b"",
                )),
                Ok(GateProcessOutcome::new(
                    GateProcessTermination::Exited(0),
                    b"test ok",
                    b"",
                )),
            ]),
        },
        store(temp.path()),
    );
    let evidence = port.run(&plan(&lease), &lease).unwrap();
    assert_eq!(evidence.commands(), &["format", "test"]);
    assert!(
        evidence
            .artifact()
            .path()
            .as_str()
            .starts_with("gates/final-gate-v2-")
    );
    assert_eq!(
        port.driver
            .calls
            .iter()
            .map(|call| call.command().command_id())
            .collect::<Vec<_>>(),
        vec!["format", "test"]
    );
    assert!(
        port.driver
            .calls
            .iter()
            .all(GateInvocation::independent_process_group)
    );
    assert!(port.driver.calls.iter().all(|call| {
        call.env()
            .iter()
            .all(|(key, _)| key != "HOME" && key != "AWS_SECRET_ACCESS_KEY")
    }));
    assert!(
        port.driver
            .calls
            .iter()
            .all(|call| call.command().network() == GateNetworkPolicy::Denied)
    );
    let bytes = std::fs::read(
        temp.path().join(
            evidence
                .artifact()
                .path()
                .as_str()
                .strip_prefix("gates/")
                .unwrap(),
        ),
    )
    .unwrap();
    let rendered = String::from_utf8(bytes).unwrap();
    assert!(!rendered.contains("\"artifact\""));
    assert!(!rendered.contains("\"content_digest\""));
}

#[test]
fn nonzero_timeout_cancel_and_survivor_never_publish_success() {
    for termination in [
        GateProcessTermination::Exited(4),
        GateProcessTermination::TimedOut,
        GateProcessTermination::Cancelled,
        GateProcessTermination::ChildSurvivor,
        GateProcessTermination::ReapFailed,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let lease = lease(temp.path());
        let mut port = HostFinalGatePort::new(
            Driver {
                calls: Vec::new(),
                outcomes: VecDeque::from([Ok(GateProcessOutcome::new(
                    termination,
                    b"",
                    b"failure",
                ))]),
            },
            store(temp.path()),
        );
        assert!(port.run(&plan(&lease), &lease).is_err());
        assert!(std::fs::read_dir(temp.path()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("final-gate-v2-")
        }));
    }
}

#[test]
fn output_is_bounded_redacted_and_control_safe() {
    let temp = tempfile::tempdir().unwrap();
    let lease = lease(temp.path());
    let mut noisy = vec![b'x'; 20 * 1024];
    noisy.extend_from_slice(b" token=secret-value keep\x01TAIL");
    let mut port = HostFinalGatePort::new(
        Driver {
            calls: Vec::new(),
            outcomes: VecDeque::from([
                Ok(GateProcessOutcome::new(
                    GateProcessTermination::Exited(0),
                    noisy,
                    b"",
                )),
                Ok(GateProcessOutcome::new(
                    GateProcessTermination::Exited(0),
                    b"",
                    b"",
                )),
            ]),
        },
        store(temp.path()),
    );
    let evidence = port.run(&plan(&lease), &lease).unwrap();
    let bytes = std::fs::read(
        temp.path().join(
            evidence
                .artifact()
                .path()
                .as_str()
                .strip_prefix("gates/")
                .unwrap(),
        ),
    )
    .unwrap();
    let rendered = String::from_utf8(bytes).unwrap();
    assert!(rendered.len() < 160 * 1024);
    assert!(rendered.contains("[REDACTED]"));
    assert!(!rendered.contains("secret-value"));
    assert!(rendered.contains("\\\\u{0001}"));
}

#[test]
fn tamper_and_epoch_drift_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let lease = lease(temp.path());
    let plan = plan(&lease);
    let mut port = HostFinalGatePort::new(
        Driver {
            calls: Vec::new(),
            outcomes: VecDeque::from([
                Ok(GateProcessOutcome::new(
                    GateProcessTermination::Exited(0),
                    b"",
                    b"",
                )),
                Ok(GateProcessOutcome::new(
                    GateProcessTermination::Exited(0),
                    b"",
                    b"",
                )),
            ]),
        },
        store(temp.path()),
    );
    let evidence = port.run(&plan, &lease).unwrap();
    let path = temp.path().join(
        evidence
            .artifact()
            .path()
            .as_str()
            .strip_prefix("gates/")
            .unwrap(),
    );
    std::fs::write(&path, b"tampered").unwrap();
    assert!(port.readback(&plan, evidence.artifact()).is_err());

    let drifted = Lease {
        valid: false,
        ..lease
    };
    let mut second = HostFinalGatePort::new(
        Driver {
            calls: Vec::new(),
            outcomes: VecDeque::new(),
        },
        store(temp.path()),
    );
    assert!(second.run(&plan, &drifted).is_err());
}
