use clap::CommandFactory;

use super::parsed_review;

#[test]
fn repair_only_cli_requires_explicit_campaign_and_rejects_review_routing() {
    let campaign = "01ARZ3NDEKTSV4RRFFQ69G5FB2";
    let args = parsed_review(&["csa", "review", "--repair-only", "--campaign", campaign]);
    assert!(args.repair_only);
    assert_eq!(args.campaign.as_deref(), Some(campaign));
    crate::cli::validate_review_args(&args).expect("explicit repair authorization should validate");
    let mut command = crate::cli::Cli::command();
    let help = command
        .find_subcommand_mut("review")
        .expect("review subcommand")
        .render_long_help()
        .to_string();
    assert!(help.contains("--repair-only"));
    assert!(help.contains("--campaign"));

    for tail in [
        vec!["--repair-only"],
        vec!["--campaign", campaign],
        vec![
            "--repair-only",
            "--campaign",
            campaign,
            "--range",
            "main...HEAD",
        ],
        vec!["--repair-only", "--campaign", campaign, "--fix"],
        vec![
            "--repair-only",
            "--campaign",
            campaign,
            "--execute-completion",
        ],
        vec!["--repair-only", "--campaign", campaign, "--tier", "quality"],
        vec![
            "--repair-only",
            "--campaign",
            campaign,
            "--model-spec",
            "codex/openai/gpt-5.6/high",
        ],
    ] {
        let mut argv = vec!["csa", "review"];
        argv.extend(tail);
        let args = parsed_review(&argv);
        assert!(
            crate::cli::validate_review_args(&args).is_err(),
            "unsafe repair-only invocation unexpectedly validated: {argv:?}"
        );
    }
}

#[test]
fn repair_only_dispatch_precedes_ordinary_review_work() {
    let source = include_str!("../review_cmd_handle.rs");
    let repair_dispatch = source
        .find("if args.repair_only {")
        .expect("repair-only dispatch is missing");
    for ordinary_step in [
        "verify_review_skill_available",
        "run_pre_review_quality_gate",
        "derive_scope_for_project",
        "resolve_review_depth_for_project",
        "compute_review_diff_size",
        "build_review_instruction_for_project",
    ] {
        let position = source
            .find(ordinary_step)
            .unwrap_or_else(|| panic!("ordinary review step is missing: {ordinary_step}"));
        assert!(
            repair_dispatch < position,
            "repair-only dispatch must precede ordinary review step {ordinary_step}"
        );
    }
}
