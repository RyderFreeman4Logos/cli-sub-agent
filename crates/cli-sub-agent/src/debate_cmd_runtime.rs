use std::io::IsTerminal;
use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use csa_core::types::OutputFormat;
use tokio::time::Instant;
use tracing::debug;

use crate::debate_cmd_output::{
    DebateOutputHeader, DebateSummary, format_debate_stdout_text, render_debate_stdout_json,
};
use crate::debate_errors::DebateErrorKind;
use crate::pattern_resolver::ResolvedPattern;

pub(super) fn render_debate_cli_output(
    output_format: OutputFormat,
    debate_summary: &DebateSummary,
    transcript: &str,
    meta_session_id: &str,
    output_header: Option<DebateOutputHeader>,
) -> Result<String> {
    match output_format {
        OutputFormat::Text => Ok(format_debate_stdout_text(
            debate_summary,
            transcript,
            output_header,
        )),
        OutputFormat::Json => {
            render_debate_stdout_json(debate_summary, transcript, meta_session_id, output_header)
        }
    }
}

pub(super) const STILL_WORKING_BACKOFF: Duration = Duration::from_secs(5);

/// Verify the debate pattern is installed before attempting execution.
///
/// Fails fast with actionable install guidance if the pattern is missing,
/// preventing silent degradation where the tool runs without skill context.
pub(super) fn verify_debate_skill_available(project_root: &Path) -> Result<ResolvedPattern> {
    match crate::pattern_resolver::resolve_pattern("debate", project_root) {
        Ok(resolved) => {
            debug!(
                pattern_dir = %resolved.dir.display(),
                has_config = resolved.config.is_some(),
                skill_md_len = resolved.skill_md.len(),
                "Debate pattern resolved"
            );
            Ok(resolved)
        }
        Err(resolve_err) => {
            anyhow::bail!(
                "Debate pattern not found — `csa debate` requires the 'debate' pattern.\n\n\
                 {resolve_err}\n\n\
                 Install the debate pattern with one of:\n\
                 1) csa skill install RyderFreeman4Logos/cli-sub-agent\n\
                 2) Manually place skills/debate/SKILL.md (or PATTERN.md) inside .csa/patterns/debate/ or patterns/debate/\n\n\
                 Without the pattern, the debate tool cannot follow the structured debate protocol."
            )
        }
    }
}

/// Resolve stream mode for debate command.
///
/// - `--stream-stdout` forces TeeToStderr (progressive output)
/// - `--no-stream-stdout` forces BufferOnly (silent until complete)
/// - Default: auto-detect TTY on stderr -> TeeToStderr if interactive,
///   BufferOnly otherwise. Symmetric with review's behavior (#139).
pub(super) fn resolve_debate_stream_mode(
    stream_stdout: bool,
    no_stream_stdout: bool,
) -> csa_process::StreamMode {
    if no_stream_stdout {
        csa_process::StreamMode::BufferOnly
    } else if stream_stdout || std::io::stderr().is_terminal() {
        csa_process::StreamMode::TeeToStderr
    } else {
        csa_process::StreamMode::BufferOnly
    }
}

pub(super) fn resolve_debate_thinking(
    cli_thinking: Option<&str>,
    config_thinking: Option<&str>,
    model_spec_active: bool,
) -> Option<String> {
    cli_thinking.map(str::to_string).or_else(|| {
        (!model_spec_active)
            .then_some(config_thinking)
            .flatten()
            .map(str::to_string)
    })
}

pub(super) fn resolve_debate_timeout_seconds(
    cli_timeout_seconds: Option<u64>,
    global_timeout_seconds: Option<u64>,
) -> Option<u64> {
    cli_timeout_seconds.or(global_timeout_seconds)
}

pub(super) fn ensure_debate_wall_clock_within_timeout(
    wall_clock_start: Instant,
    timeout_seconds: Option<u64>,
) -> Result<()> {
    if let Some(timeout_secs) = timeout_seconds
        && wall_clock_start.elapsed() > Duration::from_secs(timeout_secs)
    {
        anyhow::bail!("Wall-clock timeout exceeded ({timeout_secs}s)");
    }
    Ok(())
}

pub(super) fn should_retry_debate_after_error(
    kind: &DebateErrorKind,
    retry_count: u8,
    no_failover: bool,
) -> bool {
    if no_failover {
        return false;
    }
    matches!(kind, DebateErrorKind::Transient(_)) && retry_count < 1
}

pub(super) async fn wait_for_still_working_backoff() {
    tracing::info!("Tool still working, waiting before next attempt...");
    tokio::time::sleep(STILL_WORKING_BACKOFF).await;
}
