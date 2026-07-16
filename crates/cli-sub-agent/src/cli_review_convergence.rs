use super::{ReviewArgs, repair_args};

pub(super) fn validate(args: &ReviewArgs) -> std::result::Result<(), clap::Error> {
    if args.repair_only {
        return repair_args::validate_repair_only_args(args);
    }
    if !args.converge && !args.discovery_only && !args.execute_completion {
        return Ok(());
    }

    let error = |kind, detail: &str| {
        clap::Error::raw(
            kind,
            format!(
                "convergence report/execute capability: {detail}; this command never falls back to ordinary review"
            ),
        )
    };
    if !args.converge && args.discovery_only {
        return Err(error(
            clap::error::ErrorKind::MissingRequiredArgument,
            "--discovery-only requires --converge",
        ));
    }
    if !args.converge && args.execute_completion {
        return Err(error(
            clap::error::ErrorKind::MissingRequiredArgument,
            "--execute-completion requires --converge",
        ));
    }
    if args.discovery_only && args.execute_completion {
        return Err(error(
            clap::error::ErrorKind::ArgumentConflict,
            "--discovery-only is a legacy observation mode and conflicts with --execute-completion",
        ));
    }
    if args.execute_completion && args.campaign.is_none() {
        return Err(error(
            clap::error::ErrorKind::MissingRequiredArgument,
            "--execute-completion requires --campaign <CAMPAIGN_ID>",
        ));
    }
    if !args.execute_completion && args.campaign.is_some() {
        return Err(error(
            clap::error::ErrorKind::ArgumentConflict,
            "--campaign requires --converge --execute-completion or --repair-only",
        ));
    }
    let Some(range) = args.range.as_deref() else {
        return Err(error(
            clap::error::ErrorKind::MissingRequiredArgument,
            "an explicit --range <base>...HEAD is required",
        ));
    };
    let Some(base) = range.strip_suffix("...HEAD") else {
        return Err(error(
            clap::error::ErrorKind::ValueValidation,
            "--range must use the exact three-dot form <base>...HEAD",
        ));
    };
    if base.is_empty() || base.contains("..") {
        return Err(error(
            clap::error::ErrorKind::ValueValidation,
            "--range must name a nonempty base in the exact form <base>...HEAD",
        ));
    }

    let conflict = if args.check_verdict {
        Some("--check-verdict")
    } else if args.fix {
        Some("--fix")
    } else if args.fix_finding {
        Some("--fix-finding")
    } else if args.session.is_some() {
        Some("--session/--resume")
    } else if args.diff {
        Some("--diff")
    } else if args.branch.is_some() {
        Some("--branch")
    } else if args.commit.is_some() {
        Some("--commit")
    } else if args.files.is_some() {
        Some("--files")
    } else if args.requested_reviewers() > 1 {
        Some("--reviewers > 1")
    } else if args.context.is_some() {
        Some("--context")
    } else if args.prompt.is_some() {
        Some("--prompt")
    } else if args.prompt_file.is_some() {
        Some("--prompt-file")
    } else if args.spec.is_some() {
        Some("--spec")
    } else if args.no_fs_sandbox {
        Some("--no-fs-sandbox")
    } else if args.allow_user_daemon_ipc {
        Some("--allow-user-daemon-ipc")
    } else if !args.extra_writable.is_empty() {
        Some("--extra-writable")
    } else if !args.extra_readable.is_empty() {
        Some("--extra-readable")
    } else if args.prior_rounds_summary.is_some() {
        Some("--prior-rounds-summary")
    } else {
        None
    };
    if let Some(flag) = conflict {
        return Err(error(
            clap::error::ErrorKind::ArgumentConflict,
            &format!("{flag} is outside the convergence report/execute capability"),
        ));
    }
    Ok(())
}
