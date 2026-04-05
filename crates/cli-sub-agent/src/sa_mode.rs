use crate::cli::{Commands, PlanCommands};
use csa_core::types::OutputFormat;

const SA_MODE_REQUIRED_ERROR_PREFIX: &str = "--sa-mode true|false is required for root callers on execution commands.\n\
     Hint: add --sa-mode false for interactive use, or --sa-mode true for autonomous workflows";
const CSA_INTERNAL_INVOCATION_ENV: &str = "CSA_INTERNAL_INVOCATION";

fn command_name_for_sa_mode(command: &Commands) -> Option<&'static str> {
    match command {
        Commands::Run { .. } => Some("run"),
        Commands::Review(_) => Some("review"),
        Commands::Debate(_) => Some("debate"),
        Commands::Batch { .. } => Some("batch"),
        Commands::Plan {
            cmd: PlanCommands::Run { .. },
        } => Some("plan run"),
        Commands::ClaudeSubAgent(_) => Some("claude-sub-agent"),
        _ => None,
    }
}

pub(crate) fn command_sa_mode_arg(command: &Commands) -> Option<Option<bool>> {
    match command {
        Commands::Run { sa_mode, .. } => Some(*sa_mode),
        Commands::Review(args) => Some(args.sa_mode),
        Commands::Debate(args) => Some(args.sa_mode),
        Commands::Batch { sa_mode, .. } => Some(*sa_mode),
        Commands::Plan {
            cmd: PlanCommands::Run { sa_mode, .. },
        } => Some(*sa_mode),
        Commands::ClaudeSubAgent(args) => Some(args.sa_mode),
        _ => None,
    }
}

fn is_internal_sa_invocation(current_depth: u32) -> bool {
    if current_depth == 0 {
        return false;
    }

    std::env::var(CSA_INTERNAL_INVOCATION_ENV)
        .ok()
        .map(|raw| {
            let normalized = raw.trim().to_ascii_lowercase();
            matches!(normalized.as_str(), "1" | "true" | "yes" | "on")
        })
        .unwrap_or(false)
}

pub(crate) fn validate_sa_mode(command: &Commands, current_depth: u32) -> anyhow::Result<bool> {
    let Some(sa_mode_arg) = command_sa_mode_arg(command) else {
        return Ok(false);
    };

    if sa_mode_arg.is_none() && !is_internal_sa_invocation(current_depth) {
        let command_name = command_name_for_sa_mode(command).unwrap_or("execution command");
        anyhow::bail!("{SA_MODE_REQUIRED_ERROR_PREFIX}: command `{command_name}`");
    }

    Ok(sa_mode_arg.unwrap_or(false))
}

/// Apply SA mode prompt guard and emit caller-side constraint if active.
///
/// Returns `true` when SA mode is effectively enabled for this invocation.
/// When SA mode is active at root depth, a structured guard block is emitted
/// to stdout so the calling agent sees the Layer 0 Manager constraints.
pub(crate) fn apply_sa_mode_prompt_guard(
    command: &Commands,
    current_depth: u32,
    output_format: OutputFormat,
) -> anyhow::Result<bool> {
    if command_sa_mode_arg(command).is_none() {
        return Ok(false);
    }

    let sa_mode_enabled = validate_sa_mode(command, current_depth)?;
    let value = if sa_mode_enabled { "true" } else { "false" };

    // SAFETY: process-level env updated once during startup before async work begins.
    unsafe {
        std::env::set_var(
            crate::pipeline::prompt_guard::PROMPT_GUARD_CALLER_INJECTION_ENV,
            value,
        )
    };

    // Emit SA mode caller guard to stdout (pre-session constraint).
    // Only for Text output — JSON mode must not be corrupted by guard XML.
    let text_mode = matches!(output_format, OutputFormat::Text);
    crate::pipeline::prompt_guard::emit_sa_mode_caller_guard(
        sa_mode_enabled,
        current_depth,
        text_mode,
    );

    Ok(sa_mode_enabled)
}
