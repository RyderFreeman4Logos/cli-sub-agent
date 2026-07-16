use std::path::PathBuf;
use std::time::Duration;

use csa_session::convergence::{
    CampaignId, EpochRecord, GitObjectId, Sha256Digest, WorkspaceLeaseIdentity,
};
use ulid::Ulid;

use super::gate_authority::{
    FinalGateAuthority, FinalGatePlan, GateCommandAuthority, GateNetworkPolicy,
};

fn epoch() -> EpochRecord {
    EpochRecord::new(
        GitObjectId::parse(&"a".repeat(40)).unwrap(),
        GitObjectId::parse(&"b".repeat(40)).unwrap(),
        Sha256Digest::compute(b"gate diff"),
    )
}

fn lease() -> WorkspaceLeaseIdentity {
    WorkspaceLeaseIdentity::new(
        CampaignId::generate(),
        epoch(),
        1,
        PathBuf::from("/tmp/csa-final-gate-authority-tests"),
        1,
        1,
        Ulid::new().to_string(),
    )
    .unwrap()
}

fn command(id: &str, args: &[&str]) -> GateCommandAuthority {
    GateCommandAuthority::new(
        id,
        "just",
        args.iter().map(|arg| (*arg).to_string()).collect(),
        GateNetworkPolicy::Denied,
        Duration::from_secs(30),
    )
    .unwrap()
}

#[test]
fn plan_binds_policy_authority_version_order_and_argv() {
    let authority = FinalGateAuthority::new(
        "global-gates-v4",
        vec![
            command("format", &["fmt-check"]),
            command("test", &["test"]),
        ],
    )
    .unwrap();
    let plan = FinalGatePlan::from_authority(
        Sha256Digest::compute(b"completion policy"),
        lease(),
        &authority,
        authority.commands().to_vec(),
    )
    .unwrap();

    assert_eq!(plan.schema_version(), 1);
    assert_eq!(plan.authority_version(), "global-gates-v4");
    assert_eq!(plan.command_authority_digest(), &authority.digest());
    assert_eq!(plan.commands()[0].command_id(), "format");
    assert_eq!(plan.commands()[1].argv(), &["test"]);
    assert_eq!(
        plan.commands()[0].credentials(),
        super::gate_authority::GateCredentialPolicy::Denied
    );
    assert_eq!(plan.minimal_env().get("CI").map(String::as_str), Some("1"));
    assert!(plan.minimal_env().get("HOME").is_none());
}

#[test]
fn lower_trust_project_config_cannot_expand_reorder_or_modify_authorized_commands() {
    let authority = FinalGateAuthority::new(
        "global-gates-v1",
        vec![
            command("format", &["fmt-check"]),
            command("test", &["test"]),
        ],
    )
    .unwrap();
    let policy = Sha256Digest::compute(b"policy");

    for proposal in [
        vec![command("format", &["fmt-check"])],
        vec![
            command("test", &["test"]),
            command("format", &["fmt-check"]),
        ],
        vec![
            command("format", &["fmt-check", "--unsafe"]),
            command("test", &["test"]),
        ],
        vec![
            command("format", &["fmt-check"]),
            command("test", &["test"]),
            command("exfiltrate", &["publish"]),
        ],
    ] {
        let error = FinalGatePlan::from_authority(policy.clone(), lease(), &authority, proposal)
            .expect_err("lower trust must not alter final-gate authority");
        assert!(error.to_string().contains("high-trust"));
    }
}

#[test]
fn direct_argv_authority_rejects_shell_programs_and_invalid_timeout() {
    assert!(
        GateCommandAuthority::new(
            "bad-shell",
            "sh",
            vec!["-c".to_string(), "echo unsafe".to_string()],
            GateNetworkPolicy::Denied,
            Duration::from_secs(30),
        )
        .is_err()
    );
    assert!(
        GateCommandAuthority::new(
            "too-long",
            "just",
            vec![],
            GateNetworkPolicy::Denied,
            Duration::from_secs(30 * 60 + 1),
        )
        .is_err()
    );
}
