use clap::Parser;

#[test]
fn convergence_report_dispatch_precedes_config_and_all_execution_paths() {
    let source = include_str!("../review_cmd_handle.rs");
    let report_dispatch = source
        .find("if args.converge && !args.discovery_only && !args.execute_completion {")
        .expect("read-only convergence report dispatch is missing");
    for side_effect_boundary in [
        "load_and_validate",
        "run_repair",
        "resolve_selection_tool",
        "run_early_command",
        "run_pre_review_quality_gate",
    ] {
        let boundary = source
            .find(side_effect_boundary)
            .unwrap_or_else(|| panic!("side-effect boundary {side_effect_boundary} is missing"));
        assert!(
            report_dispatch < boundary,
            "read-only report must dispatch before {side_effect_boundary}"
        );
    }
}

#[test]
fn execute_completion_policy_admission_precedes_provider_selection_and_dispatch() {
    let source = include_str!("../review_cmd_handle.rs");
    let admission = source
        .find("ensure_completion_execution_is_allowed(")
        .expect("completion policy admission is missing");
    for provider_boundary in [
        "resolve_selection_tool",
        "enforce_review_tier_bypass_gate",
        "run_early_command",
    ] {
        let boundary = source
            .find(provider_boundary)
            .unwrap_or_else(|| panic!("provider boundary {provider_boundary} is missing"));
        assert!(
            admission < boundary,
            "completion policy admission must precede {provider_boundary}"
        );
    }
}

#[test]
fn discovery_only_keeps_the_legacy_clustering_json_contract() {
    let source = include_str!("../review_convergence/mod.rs");
    let legacy_output = source
        .find("\"kind\": \"convergence_clustering_complete\"")
        .expect("legacy discovery-only JSON output is missing");
    let execute_output = source
        .find("\"reason_code\": \"completion_runtime_not_wired\"")
        .expect("explicit completion safety block is missing");
    assert!(
        execute_output < legacy_output,
        "execute-only safety block must not replace the legacy discovery-only JSON path"
    );
    let legacy_tail = &source[legacy_output..];
    assert!(legacy_tail.contains("\"review_verdict\": null"));
    assert!(legacy_tail.contains("\"merge_attestation\": false"));
}

#[test]
fn completion_authorization_binds_cluster_identity_executor_and_policy() {
    use csa_config::{ConvergenceCompletionPolicy, ProjectConvergenceCompletionPolicy};
    use csa_session::convergence::{
        AdmittedModelIdentity, CampaignId, EpochRecord, GitObjectId, Sha256Digest,
    };

    let campaign = CampaignId::parse("01ARZ3NDEKTSV4RRFFQ69G5FB2").unwrap();
    let epoch = EpochRecord::new(
        GitObjectId::parse("0123456789abcdef0123456789abcdef01234567").unwrap(),
        GitObjectId::parse("fedcba9876543210fedcba9876543210fedcba98").unwrap(),
        Sha256Digest::compute(b"diff"),
    );
    let global = ConvergenceCompletionPolicy {
        allow_execution: true,
        allow_provider_egress: true,
        allow_shell_commands: true,
        allow_credential_inheritance: true,
        max_retention_days: 7,
    };
    let policy = ConvergenceCompletionPolicy::effective(
        &global,
        Some(&ProjectConvergenceCompletionPolicy::default()),
    );
    let event = super::super::completion_authorization::CompletionAuthorizationEvent::new(
        campaign,
        &epoch,
        2,
        AdmittedModelIdentity::new("codex", "openai", "gpt-5.6", "xhigh").unwrap(),
        &policy,
    )
    .unwrap();
    let event = serde_json::to_value(event).unwrap();
    assert_eq!(event["capability"], "execute_completion");
    assert_eq!(event["repair_batch_count"], 2);
    assert_eq!(event["admitted_executor"]["tool"], "codex");
    assert_eq!(event["admitted_executor"]["provider"], "openai");
    assert!(event["policy_digest"].as_str().is_some());
    assert_eq!(event["epoch_id"], epoch.id().as_str());
}

#[test]
fn convergence_cli_rejects_non_range_scope_selectors_at_parse_time() {
    let selectors = [
        vec!["--diff"],
        vec!["--branch", "feature"],
        vec!["--commit", "HEAD"],
        vec!["--files", "src/lib.rs"],
    ];
    for selector in selectors {
        let mut argv = vec![
            "csa",
            "review",
            "--converge",
            "--discovery-only",
            "--range",
            "main...HEAD",
        ];
        argv.extend(selector);
        assert!(crate::cli::Cli::try_parse_from(argv).is_err());
    }
}
