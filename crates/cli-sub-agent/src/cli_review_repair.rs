//! Strict CLI validation for explicit consolidated repair authorization.

use super::{ReviewArgs, ReviewChunkingMode, ReviewDepth};

pub(super) fn validate_repair_only_args(args: &ReviewArgs) -> std::result::Result<(), clap::Error> {
    let error = |kind, detail: &str| {
        clap::Error::raw(
            kind,
            format!("explicit consolidated repair authorization: {detail}"),
        )
    };
    if !args.repair_only || args.campaign.is_none() {
        return Err(error(
            clap::error::ErrorKind::MissingRequiredArgument,
            "--repair-only and --campaign <CAMPAIGN_ID> require each other",
        ));
    }
    let conflict = if args.converge || args.discovery_only || args.execute_completion {
        Some("--converge/--discovery-only/--execute-completion")
    } else if args.check_verdict {
        Some("--check-verdict")
    } else if args.fix || args.fix_finding {
        Some("--fix/--fix-finding")
    } else if args.diff
        || args.branch.is_some()
        || args.commit.is_some()
        || args.range.is_some()
        || args.files.is_some()
    {
        Some("review scope flags")
    } else if args.tool.is_some()
        || args.model.is_some()
        || args.model_spec.is_some()
        || args.tier.is_some()
        || args.hint_difficulty.is_some()
        || args.thinking.is_some()
        || args.force_ignore_tier_setting
        || args.force_override_user_config
        || args.no_failover
        || args.fast_but_more_cost
    {
        Some("model-routing flags")
    } else if args.session.is_some()
        || args.context.is_some()
        || args.prompt.is_some()
        || args.prompt_file.is_some()
        || args.spec.is_some()
        || args.prior_rounds_summary.is_some()
    {
        Some("ordinary review input flags")
    } else if args.reviewers.is_some() || args.single {
        Some("reviewer-count flags")
    } else if args.full_consistency
        || args.chunked_review != ReviewChunkingMode::Auto
        || args.max_rounds != 3
        || args.review_mode.is_some()
        || args.depth != ReviewDepth::Standard
        || args.red_team
        || args.security_mode != "auto"
        || args.consensus != "majority"
        || args.allow_fallback
    {
        Some("review policy flags")
    } else if args.no_fs_sandbox
        || args.allow_user_daemon_ipc
        || !args.extra_writable.is_empty()
        || !args.extra_readable.is_empty()
    {
        Some("sandbox override flags")
    } else {
        None
    };
    if let Some(flag) = conflict {
        return Err(error(
            clap::error::ErrorKind::ArgumentConflict,
            &format!("{flag} cannot accompany --repair-only"),
        ));
    }
    Ok(())
}
