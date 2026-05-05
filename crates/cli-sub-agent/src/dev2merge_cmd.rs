//! Thin CLI alias for running the dev2merge weave workflow.

use anyhow::{Context, Result, bail};
use tokio::process::Command;

use crate::cli::Dev2mergeArgs;
use crate::plan_cmd::{PlanRunArgs, PlanRunPipelineSource};
use crate::plan_cmd_daemon::{PlanRunDispatchInput, dispatch_plan_run};

const DEV2MERGE_PATTERN: &str = "dev2merge";
const FEATURE_INPUT_VAR: &str = "FEATURE_INPUT";
const MKTD_TIMEOUT_VAR: &str = "MKTD_TIMEOUT_SECONDS";

pub(crate) async fn handle_dev2merge(
    args: Dev2mergeArgs,
    current_depth: u32,
    sa_mode_active: bool,
    text_output: bool,
) -> Result<()> {
    let issue_body = match args.issue {
        Some(issue) => Some(fetch_issue_body(issue).await?),
        None => None,
    };
    let sa_mode = args.sa_mode;
    let plan_args = build_plan_run_args(args, current_depth, issue_body);
    let forwarded_args = build_forwarded_plan_args(&plan_args, sa_mode);
    dispatch_plan_run(
        plan_args,
        PlanRunDispatchInput {
            foreground: false,
            daemon_child: false,
            session_id: None,
            sa_mode_active,
            text_output,
            forwarded_args: Some(forwarded_args),
        },
    )
    .await
}

fn build_plan_run_args(
    args: Dev2mergeArgs,
    current_depth: u32,
    issue_body: Option<String>,
) -> PlanRunArgs {
    let mut vars = Vec::new();
    if let Some(body) = issue_body {
        vars.push(format!("{FEATURE_INPUT_VAR}={body}"));
    }
    if let Some(timeout) = args.timeout {
        vars.push(format!("{MKTD_TIMEOUT_VAR}={timeout}"));
    }
    vars.extend(args.vars);

    PlanRunArgs {
        file: None,
        pattern: Some(DEV2MERGE_PATTERN.to_string()),
        vars,
        tool_override: None,
        dry_run: false,
        chunked: false,
        resume: None,
        cd: None,
        current_depth,
        pipeline_source: PlanRunPipelineSource::CliAlias,
    }
}

fn build_forwarded_plan_args(plan_args: &PlanRunArgs, sa_mode: Option<bool>) -> Vec<String> {
    let mut forwarded = Vec::new();
    if let Some(file) = &plan_args.file {
        forwarded.push(file.clone());
    }
    if let Some(pattern) = &plan_args.pattern {
        forwarded.push("--pattern".to_string());
        forwarded.push(pattern.clone());
    }
    if let Some(sa_mode) = sa_mode {
        forwarded.push("--sa-mode".to_string());
        forwarded.push(sa_mode.to_string());
    }
    for var in &plan_args.vars {
        forwarded.push("--var".to_string());
        forwarded.push(var.clone());
    }
    if let Some(tool) = &plan_args.tool_override {
        forwarded.push("--tool".to_string());
        forwarded.push(tool.to_string());
    }
    if plan_args.dry_run {
        forwarded.push("--dry-run".to_string());
    }
    if plan_args.chunked {
        forwarded.push("--chunked".to_string());
    }
    if let Some(resume) = &plan_args.resume {
        forwarded.push("--resume".to_string());
        forwarded.push(resume.clone());
    }
    if let Some(cd) = &plan_args.cd {
        forwarded.push("--cd".to_string());
        forwarded.push(cd.clone());
    }
    forwarded
}

async fn fetch_issue_body(issue: u64) -> Result<String> {
    let issue_arg = issue.to_string();
    let output = Command::new("gh")
        .env(
            "GH_CONFIG_DIR",
            format!(
                "{}/.config/gh-aider",
                std::env::var("HOME").unwrap_or_default()
            ),
        )
        .args([
            "issue",
            "view",
            issue_arg.as_str(),
            "--json",
            "body",
            "-q",
            ".body",
        ])
        .output()
        .await
        .with_context(|| format!("Failed to run gh issue view {issue}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("gh issue view {issue} failed: {}", stderr.trim());
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end_matches(['\r', '\n'])
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_dev2merge_plan_args_without_issue() {
        let args = Dev2mergeArgs {
            issue: None,
            vars: vec!["EXTRA=value".to_string()],
            sa_mode: Some(false),
            timeout: None,
        };

        let plan_args = build_plan_run_args(args, 2, None);

        assert_eq!(plan_args.file, None);
        assert_eq!(plan_args.pattern.as_deref(), Some("dev2merge"));
        assert_eq!(plan_args.vars, vec!["EXTRA=value"]);
        assert_eq!(plan_args.current_depth, 2);
        assert_eq!(plan_args.pipeline_source, PlanRunPipelineSource::CliAlias);
    }

    #[test]
    fn issue_and_timeout_become_workflow_vars_before_passthrough_vars() {
        let args = Dev2mergeArgs {
            issue: Some(1287),
            vars: vec![
                "OTHER=value".to_string(),
                "FEATURE_INPUT=explicit".to_string(),
            ],
            sa_mode: Some(true),
            timeout: Some(1800),
        };

        let plan_args = build_plan_run_args(args, 0, Some("issue body".to_string()));

        assert_eq!(
            plan_args.vars,
            vec![
                "FEATURE_INPUT=issue body",
                "MKTD_TIMEOUT_SECONDS=1800",
                "OTHER=value",
                "FEATURE_INPUT=explicit",
            ]
        );
    }

    #[test]
    fn forwarded_args_reexec_dev2merge_as_plan_run_with_alias_vars() {
        let args = Dev2mergeArgs {
            issue: Some(1287),
            vars: vec!["OTHER=value".to_string()],
            sa_mode: Some(true),
            timeout: Some(1800),
        };

        let plan_args = build_plan_run_args(args, 0, Some("issue body".to_string()));
        let forwarded = build_forwarded_plan_args(&plan_args, Some(true));

        assert_eq!(
            forwarded,
            vec![
                "--pattern",
                "dev2merge",
                "--sa-mode",
                "true",
                "--var",
                "FEATURE_INPUT=issue body",
                "--var",
                "MKTD_TIMEOUT_SECONDS=1800",
                "--var",
                "OTHER=value",
            ]
        );
    }
}
