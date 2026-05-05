//! Thin CLI alias for running the dev2merge weave workflow.

use anyhow::{Context, Result, bail};
use tokio::process::Command;

use crate::cli::Dev2mergeArgs;
use crate::plan_cmd::{PlanRunArgs, PlanRunPipelineSource, handle_plan_run};

const DEV2MERGE_PATTERN: &str = "dev2merge";
const FEATURE_INPUT_VAR: &str = "FEATURE_INPUT";
const MKTD_TIMEOUT_VAR: &str = "MKTD_TIMEOUT_SECONDS";

pub(crate) async fn handle_dev2merge(args: Dev2mergeArgs, current_depth: u32) -> Result<()> {
    let issue_body = match args.issue {
        Some(issue) => Some(fetch_issue_body(issue).await?),
        None => None,
    };
    let plan_args = build_plan_run_args(args, current_depth, issue_body);
    handle_plan_run(plan_args).await
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

async fn fetch_issue_body(issue: u64) -> Result<String> {
    let issue_arg = issue.to_string();
    let output = Command::new("gh")
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
}
