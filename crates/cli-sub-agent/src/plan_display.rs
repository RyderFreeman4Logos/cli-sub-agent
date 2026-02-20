//! Display helpers for `csa plan run` dry-run and execution summary.

use std::collections::HashMap;

use csa_config::ProjectConfig;
use weave::compiler::{ExecutionPlan, FailAction};

use crate::plan_cmd::{StepResult, StepTarget, resolve_step_tool};

/// Print the execution plan for dry-run mode.
pub(crate) fn print_plan(
    plan: &ExecutionPlan,
    variables: &HashMap<String, String>,
    config: Option<&ProjectConfig>,
) {
    println!("Workflow: {}", plan.name);
    if !plan.description.is_empty() {
        println!("  {}", plan.description);
    }
    println!();

    if !variables.is_empty() {
        println!("Variables:");
        for (k, v) in variables {
            println!("  ${{{k}}} = {v}");
        }
        println!();
    }

    println!("Steps ({}):", plan.steps.len());
    for step in &plan.steps {
        let tool_info = match resolve_step_tool(step, config) {
            Ok(StepTarget::DirectBash) => "bash (direct)".into(),
            Ok(StepTarget::WeaveInclude) => "weave (include)".into(),
            Ok(StepTarget::CsaTool {
                tool_name,
                model_spec,
            }) => match model_spec {
                Some(s) => format!("{} ({})", tool_name.as_str(), s),
                None => tool_name.as_str().to_string(),
            },
            Err(e) => format!("<error: {e}>"),
        };

        let on_fail = match &step.on_fail {
            FailAction::Abort => "abort",
            FailAction::Skip => "skip",
            FailAction::Retry(n) => &format!("retry({})", n),
            FailAction::Delegate(t) => &format!("delegate({})", t),
        };

        let flags = [
            step.condition.as_ref().map(|c| format!("IF {}", c)),
            step.loop_var
                .as_ref()
                .map(|l| format!("FOR {}", l.variable)),
        ];
        let flag_str: Vec<String> = flags.into_iter().flatten().collect();
        let flag_display = if flag_str.is_empty() {
            String::new()
        } else {
            format!(" [{}]", flag_str.join(", "))
        };

        println!(
            "  {}. {} [tool={}, on_fail={}]{}",
            step.id, step.title, tool_info, on_fail, flag_display,
        );
        println!(
            "     -> sets ${{STEP_{}_OUTPUT}} for subsequent steps",
            step.id
        );
    }
}

/// Print execution summary.
pub(crate) fn print_summary(results: &[StepResult], total_duration: f64) {
    println!();
    println!("=== Workflow Execution Summary ===");
    println!();

    let mut pass = 0;
    let mut fail = 0;
    let mut skip = 0;

    for r in results {
        let status = if r.skipped {
            skip += 1;
            "- SKIP"
        } else if r.exit_code == 0 {
            pass += 1;
            "✓ PASS"
        } else {
            fail += 1;
            "✗ FAIL"
        };

        println!(
            "{:8} Step {} - {} ({:.2}s){}",
            status,
            r.step_id,
            r.title,
            r.duration_secs,
            r.error
                .as_ref()
                .map(|e| format!(" — {}", e))
                .unwrap_or_default(),
        );
    }

    println!();
    println!("Total: {} steps", results.len());
    println!("Passed: {pass}, Failed: {fail}, Skipped: {skip}");
    println!("Duration: {:.2}s", total_duration);
}
