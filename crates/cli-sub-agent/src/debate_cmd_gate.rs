use std::path::Path;

use anyhow::Result;
use csa_config::{GlobalConfig, ProjectConfig};
use tracing::{debug, warn};

pub(super) async fn run_pre_debate_quality_gate(
    project_root: &Path,
    config: Option<&ProjectConfig>,
    global_config: &GlobalConfig,
) -> Result<()> {
    let gate_steps = global_config.review.effective_gate_steps();
    let gate_timeout = config
        .and_then(|c| c.review.as_ref())
        .map(|r| r.gate_timeout_secs)
        .unwrap_or_else(csa_config::ReviewConfig::default_gate_timeout);
    let gate_mode = &global_config.review.gate_mode;

    if gate_steps.is_empty() {
        let gate_command = config
            .and_then(|c| c.review.as_ref())
            .and_then(|r| r.gate_command.as_deref());
        let gate_result = crate::pipeline::gate::evaluate_quality_gate(
            project_root,
            gate_command,
            gate_timeout,
            gate_mode,
        )
        .await?;

        if gate_result.skipped {
            debug!(
                reason = gate_result.skip_reason.as_deref().unwrap_or("unknown"),
                "Quality gate skipped"
            );
        } else if !gate_result.passed() {
            match gate_mode {
                csa_config::GateMode::Monitor => {
                    warn!(
                        command = %gate_result.command,
                        exit_code = ?gate_result.exit_code,
                        "Quality gate failed (monitor mode — continuing with debate)"
                    );
                }
                csa_config::GateMode::CriticalOnly | csa_config::GateMode::Full => {
                    let mut msg = format!(
                        "Pre-debate quality gate failed (mode={gate_mode:?}).\n\
                         Command: {}\nExit code: {:?}",
                        gate_result.command, gate_result.exit_code
                    );
                    if !gate_result.stdout.is_empty() {
                        msg.push_str(&format!("\n--- stdout ---\n{}", gate_result.stdout));
                    }
                    if !gate_result.stderr.is_empty() {
                        msg.push_str(&format!("\n--- stderr ---\n{}", gate_result.stderr));
                    }
                    anyhow::bail!(msg);
                }
            }
        } else {
            debug!(command = %gate_result.command, "Quality gate passed");
        }
        return Ok(());
    }

    let pipeline_result = crate::pipeline::gate::evaluate_quality_gates(
        project_root,
        &gate_steps,
        gate_timeout,
        gate_mode,
    )
    .await?;

    if pipeline_result.passed {
        debug!("Quality gate pipeline passed");
        return Ok(());
    }

    match gate_mode {
        csa_config::GateMode::Monitor => {
            warn!("Quality gate pipeline failed (monitor mode — continuing)");
            Ok(())
        }
        csa_config::GateMode::CriticalOnly | csa_config::GateMode::Full => {
            let failed = pipeline_result.failed_step.as_deref().unwrap_or("unknown");
            let mut msg = format!(
                "Pre-debate quality gate pipeline FAILED at step: {failed}\n\
                 (mode={gate_mode:?})\n"
            );
            for step in &pipeline_result.steps {
                if !step.passed() {
                    msg.push_str(&format!(
                        "\nL{} {} ({}): exit {:?}",
                        step.level, step.name, step.command, step.exit_code
                    ));
                    if !step.stderr.is_empty() {
                        msg.push_str(&format!("\n  stderr: {}", step.stderr));
                    }
                }
            }
            anyhow::bail!(msg);
        }
    }
}
