//! Slot acquisition helpers for the `csa run` attempt loop.

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::{ToolName, ToolSelectionStrategy};
use csa_lock::slot::{
    SlotAcquireResult, ToolSlot, acquire_slot_blocking, format_slot_diagnostic, slot_usage,
    try_acquire_slot,
};
use tracing::info;

use crate::run_cmd_tool_selection::resolve_slot_wait_timeout_seconds;
use crate::run_helpers::{is_tool_binary_available_for_config, parse_tool_name};

pub(super) enum AttemptSlotOutcome {
    Acquired(ToolSlot),
    RetryWithTool(ToolName),
    Exit(i32),
}

pub(super) struct AttemptSlotRequest<'a> {
    pub(super) slots_dir: &'a Path,
    pub(super) tool_name: &'a str,
    pub(super) max_concurrent: u32,
    pub(super) session_arg: Option<&'a str>,
    pub(super) global_config: &'a GlobalConfig,
    pub(super) config: Option<&'a ProjectConfig>,
    pub(super) cross_tool_failover_enabled: bool,
    pub(super) attempts: usize,
    pub(super) max_failover_attempts: usize,
    pub(super) wait: bool,
    pub(super) strategy: &'a ToolSelectionStrategy,
}

pub(super) fn acquire_attempt_slot(
    request: AttemptSlotRequest<'_>,
    tried_tools: &mut Vec<String>,
) -> Result<AttemptSlotOutcome> {
    match try_acquire_slot(
        request.slots_dir,
        request.tool_name,
        request.max_concurrent,
        request.session_arg,
    )? {
        SlotAcquireResult::Acquired(slot) => {
            info!(
                tool = %request.tool_name,
                slot = slot.slot_index(),
                max = request.max_concurrent,
                "Acquired global slot"
            );
            Ok(AttemptSlotOutcome::Acquired(slot))
        }
        SlotAcquireResult::Exhausted(status) => {
            let all_tools = request.global_config.all_tool_slots();
            let all_tools_ref: Vec<(&str, u32)> =
                all_tools.iter().map(|(name, max)| (*name, *max)).collect();
            let all_usage = slot_usage(request.slots_dir, &all_tools_ref);
            let diag_msg = format_slot_diagnostic(request.tool_name, &status, &all_usage);

            if request.cross_tool_failover_enabled
                && request.attempts < request.max_failover_attempts
            {
                let free_alt = all_usage.iter().find(|slot_status| {
                    slot_status.tool_name != request.tool_name
                        && slot_status.free() > 0
                        && !tried_tools.contains(&slot_status.tool_name)
                        && request
                            .config
                            .map(|config| config.is_tool_auto_selectable(&slot_status.tool_name))
                            .unwrap_or(false)
                        && is_tool_binary_available_for_config(
                            &slot_status.tool_name,
                            request.config,
                        )
                });

                if let Some(alt) = free_alt {
                    info!(
                        from = %request.tool_name,
                        to = %alt.tool_name,
                        reason = "slot_exhausted",
                        "Failing over to tool with free slots"
                    );
                    tried_tools.push(request.tool_name.to_string());
                    return Ok(AttemptSlotOutcome::RetryWithTool(parse_tool_name(
                        &alt.tool_name,
                    )?));
                }
            }

            if request.wait {
                info!(
                    tool = %request.tool_name,
                    "All slots occupied, waiting for a free slot"
                );
                let timeout =
                    Duration::from_secs(resolve_slot_wait_timeout_seconds(request.config));
                let slot = acquire_slot_blocking(
                    request.slots_dir,
                    request.tool_name,
                    request.max_concurrent,
                    timeout,
                    request.session_arg,
                )?;
                info!(
                    tool = %request.tool_name,
                    slot = slot.slot_index(),
                    "Acquired slot after waiting"
                );
                Ok(AttemptSlotOutcome::Acquired(slot))
            } else {
                eprintln!("{diag_msg}");
                if matches!(request.strategy, ToolSelectionStrategy::Explicit(_))
                    && !request.cross_tool_failover_enabled
                {
                    eprintln!(
                        "Explicit --tool {} is currently unavailable. Retry later or choose a different --tool.",
                        request.tool_name
                    );
                }
                Ok(AttemptSlotOutcome::Exit(1))
            }
        }
    }
}
