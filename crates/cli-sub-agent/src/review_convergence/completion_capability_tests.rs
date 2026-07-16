use clap::{CommandFactory, Parser};

use super::{SESSION, parsed_review};

#[test]
fn convergence_cli_modes_parse_validate_and_appear_in_help() {
    let report = parsed_review(&["csa", "review", "--converge", "--range", "main...HEAD"]);
    assert!(report.converge);
    assert!(!report.discovery_only);
    assert!(!report.execute_completion);
    crate::cli::validate_review_args(&report).expect("read-only report invocation should validate");

    let non_interactive_report = parsed_review(&[
        "csa",
        "review",
        "--converge",
        "--range",
        "main...HEAD",
        "--sa-mode",
        "true",
    ]);
    assert!(non_interactive_report.sa_mode.is_some());
    assert!(
        !non_interactive_report.execute_completion,
        "non-interactive mode must not gain completion execution by default"
    );

    let args = parsed_review(&[
        "csa",
        "review",
        "--converge",
        "--discovery-only",
        "--range",
        "main...HEAD",
    ]);
    assert!(args.converge);
    assert!(args.discovery_only);
    crate::cli::validate_review_args(&args).expect("experimental invocation should validate");

    let execute = parsed_review(&[
        "csa",
        "review",
        "--converge",
        "--execute-completion",
        "--campaign",
        "01ARZ3NDEKTSV4RRFFQ69G5FB2",
        "--range",
        "main...HEAD",
    ]);
    assert!(execute.execute_completion);
    crate::cli::validate_review_args(&execute)
        .expect("explicit completion execution invocation should validate");

    let mut command = crate::cli::Cli::command();
    let help = command
        .find_subcommand_mut("review")
        .expect("review subcommand")
        .render_long_help()
        .to_string();
    assert!(help.contains("--converge"));
    assert!(help.contains("--discovery-only"));
    assert!(help.contains("--execute-completion"));
    assert!(help.contains("read-only"));
    assert!(help.contains("execution"));
}

#[test]
fn convergence_cli_rejects_unpaired_non_range_and_unsafe_options() {
    let unsafe_case = |tail: &[&'static str]| {
        let mut args = vec!["--converge", "--discovery-only", "--range", "main...HEAD"];
        args.extend_from_slice(tail);
        args
    };
    let cases = [
        vec!["--discovery-only", "--range", "main...HEAD"],
        vec!["--execute-completion", "--range", "main...HEAD"],
        vec![
            "--converge",
            "--execute-completion",
            "--range",
            "main...HEAD",
        ],
        vec![
            "--converge",
            "--campaign",
            "01ARZ3NDEKTSV4RRFFQ69G5FB2",
            "--range",
            "main...HEAD",
        ],
        vec![
            "--converge",
            "--discovery-only",
            "--execute-completion",
            "--range",
            "main...HEAD",
        ],
        vec!["--converge", "--discovery-only"],
        vec!["--converge", "--discovery-only", "--range", "main..HEAD"],
        unsafe_case(&["--check-verdict"]),
        unsafe_case(&["--fix"]),
        unsafe_case(&["--fix-finding", "--session", SESSION]),
        unsafe_case(&["--session", SESSION]),
        unsafe_case(&["--reviewers", "2"]),
        unsafe_case(&["--no-fs-sandbox"]),
        unsafe_case(&["--extra-readable", "/tmp/provider-input"]),
        unsafe_case(&["--context", "context.md"]),
        unsafe_case(&["--prompt-file", "prompt.md"]),
        unsafe_case(&["--spec", "contract.spec"]),
        unsafe_case(&["--extra-writable", "/tmp"]),
        unsafe_case(&["--prior-rounds-summary", "old.toml"]),
    ];

    for mut tail in cases {
        let spec = tail.iter().position(|arg| *arg == "--spec").map(|index| {
            tail.remove(index);
            tail.remove(index).to_owned()
        });
        let mut argv = vec!["csa", "review"];
        argv.extend(tail);
        let mut args = parsed_review(&argv);
        args.spec = spec;
        let error = crate::cli::validate_review_args(&args)
            .expect_err("unsafe convergence combination must fail");
        assert!(
            error
                .to_string()
                .contains("convergence report/execute capability"),
            "unexpected validation error: {error}"
        );
    }
}

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
        .find("completion_policy::resolve(")
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
    assert!(
        include_str!("../review_cmd_completion_policy.rs")
            .contains("ensure_completion_execution_is_allowed(")
    );
}

#[test]
fn discovery_only_keeps_the_legacy_clustering_json_contract() {
    let source = include_str!("../review_convergence/mod.rs");
    let legacy_output = source
        .find("\"kind\": \"convergence_clustering_complete\"")
        .expect("legacy discovery-only JSON output is missing");
    let execute_output = source
        .find("run_clustered_completion")
        .expect("explicit clustered completion dispatch is missing");
    assert!(
        execute_output < legacy_output,
        "execute-only dispatch must not replace the legacy discovery-only JSON path"
    );
    let legacy_tail = &source[legacy_output..];
    assert!(legacy_tail.contains("\"review_verdict\": null"));
    assert!(legacy_tail.contains("\"merge_attestation\": false"));
}

#[test]
fn execute_completion_constructs_production_ports_and_drives_the_clustered_start() {
    let command = include_str!("../review_convergence/mod.rs");
    assert!(command.contains("ProductionCompletionPorts::new"));
    assert!(command.contains("run_to_attestation_from_start(&mut ports, budget, start)"));
    assert!(
        !command.contains("production_completion_ports_missing"),
        "an admitted execute request must not stop at the former wiring placeholder"
    );

    let ports = include_str!("../review_convergence/production_completion.rs");
    let final_gates = ports
        .find("fn run_final_gates")
        .expect("production final-gates port is missing");
    let clean_room = ports
        .find("async fn run_clean_room")
        .expect("production clean-room port is missing");
    assert!(final_gates < clean_room);
    assert!(ports.contains("lease: Option<DetachedWorkspaceLease<CurrentCheckoutCleanup>>"));
    assert!(ports.contains("lease.workspace()"));
}

#[test]
fn completion_authorization_binds_cluster_identity_executor_and_policy() {
    use std::path::PathBuf;

    use csa_config::{ConvergenceCompletionPolicy, ProjectConvergenceCompletionPolicy};
    use csa_session::convergence::{
        AdmittedModelIdentity, CampaignId, EpochRecord, GitObjectId, Sha256Digest,
        WorkspaceLeaseIdentity,
    };
    use ulid::Ulid;

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
    let workspace_lease = WorkspaceLeaseIdentity::new(
        campaign.clone(),
        epoch.clone(),
        1,
        PathBuf::from("/workspace"),
        1,
        2,
        Ulid::new().to_string(),
    )
    .unwrap();
    let event = super::super::completion_authorization::CompletionAuthorizationEvent::new(
        campaign,
        &epoch,
        2,
        AdmittedModelIdentity::new("codex", "openai", "gpt-5.6", "xhigh").unwrap(),
        &policy,
        workspace_lease,
    )
    .unwrap();
    let event = serde_json::to_value(event).unwrap();
    assert_eq!(event["capability"], "execute_completion");
    assert_eq!(event["repair_batch_count"], 2);
    assert_eq!(event["admitted_executor"]["tool"], "codex");
    assert_eq!(event["admitted_executor"]["provider"], "openai");
    assert!(event["policy_digest"].as_str().is_some());
    assert_eq!(event["epoch_id"], epoch.id().as_str());
    assert_eq!(event["workspace_lease"]["generation"], 1);
    assert_eq!(event["workspace_lease"]["workspace_root"], "/workspace");
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
