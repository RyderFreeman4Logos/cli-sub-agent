use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

use anyhow::Result;
use csa_core::types::ReviewDecision;
use csa_session::{ReviewSessionMeta, ReviewVerdictArtifact};

use super::SessionWaitOutputMode;
use crate::tier_model_fallback::opaque_total_exhaustion_message;

const WAIT_OUTPUT_MAX_BYTES: u64 = 1024 * 1024;

fn stream_wait_output(session_dir: &Path) -> Result<bool> {
    let stdout_log = session_dir.join("stdout.log");
    if !stdout_log.is_file() {
        return Ok(false);
    }
    let log = read_wait_output_log(&stdout_log)?;
    if log.truncated {
        eprintln!(
            "[csa] stdout.log exceeded {WAIT_OUTPUT_MAX_BYTES} bytes; showing bounded tail output"
        );
    }
    let Some(rendered) = render_wait_output_log(&log.raw, log.truncated) else {
        return Ok(false);
    };
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(rendered.as_bytes())?;
    let bytes = rendered.len() as u64;
    stdout.flush()?;
    Ok(bytes > 0)
}

pub(crate) fn render_wait_terminal_output(
    session_dir: &Path,
    session_id: &str,
    result: Option<&csa_session::SessionResult>,
    output_mode: SessionWaitOutputMode,
) -> Result<Option<String>> {
    if output_mode == SessionWaitOutputMode::Verbose {
        let stdout_log = session_dir.join("stdout.log");
        if !stdout_log.is_file() {
            return Ok(None);
        }
        let log = read_wait_output_log(&stdout_log)?;
        let Some(rendered) = render_wait_output_log(&log.raw, log.truncated) else {
            return Ok(None);
        };
        if log.truncated {
            return Ok(Some(format!(
                "[csa] stdout.log exceeded {WAIT_OUTPUT_MAX_BYTES} bytes; showing bounded tail output\n{rendered}"
            )));
        }
        return Ok(Some(rendered));
    }

    let Some(result) = result else {
        return Ok(None);
    };

    let rendered = match output_mode {
        SessionWaitOutputMode::CompactText => {
            render_wait_result_summary(session_dir, session_id, result)
        }
        SessionWaitOutputMode::CompactJson => {
            render_wait_result_json(session_dir, session_id, result)?
        }
        SessionWaitOutputMode::Verbose => unreachable!("handled above"),
    };
    Ok(Some(rendered))
}

pub(super) fn emit_wait_terminal_output(
    session_dir: &Path,
    session_id: &str,
    result: Option<&csa_session::SessionResult>,
    output_mode: SessionWaitOutputMode,
) -> Result<bool> {
    if output_mode == SessionWaitOutputMode::Verbose {
        return stream_wait_output(session_dir);
    }

    let Some(result) = result else {
        return Ok(false);
    };

    let rendered = match output_mode {
        SessionWaitOutputMode::CompactText => {
            render_wait_result_summary(session_dir, session_id, result)
        }
        SessionWaitOutputMode::CompactJson => {
            render_wait_result_json(session_dir, session_id, result)?
        }
        SessionWaitOutputMode::Verbose => unreachable!("handled above"),
    };
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(rendered.as_bytes())?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(true)
}

pub(crate) fn render_wait_result_summary(
    session_dir: &Path,
    session_id: &str,
    result: &csa_session::SessionResult,
) -> String {
    let provider_quota =
        crate::session_provider_quota::provider_quota_display_for_result(session_dir, result);
    let mut lines = vec![format!("Session: {session_id}")];
    lines.extend(crate::session_display_alias::text_lines(
        session_dir,
        session_id,
    ));
    lines.extend([
        format!("Status: {}", result.status),
        format!("Exit code: {}", result.exit_code),
        format!("Tool: {}", wait_result_tool_label(result)),
        format!("Elapsed: {}", format_wait_elapsed(result)),
    ]);
    if let Some(outcome) = result.outcome_code() {
        lines.push(format!("Outcome: {outcome}"));
    }

    if let Some(tokens) = extract_wait_token_summary(result) {
        lines.push(format!("Tokens: {}", tokens.render_text()));
    }

    if let Some(verdict) = read_review_verdict_label(session_dir, result) {
        lines.push(format!("Review verdict: {verdict}"));
    }

    if let Some(reason) =
        crate::session_unavailable_reason::review_unavailable_reason_label(session_dir)
    {
        lines.push(format!("Unavailable reason: {reason}"));
    }

    if let Some(failover) = format_failover_chain_label(session_dir, result) {
        lines.push(format!("Failover: {failover}"));
    }

    if let Some(kill_hint) = format_kill_hint_label(session_dir, result) {
        lines.push(kill_hint);
    }

    if let Some(recovery) = result.require_commit_recovery.as_ref() {
        lines.extend(
            crate::require_commit_recovery_display::format_require_commit_recovery_lines(recovery),
        );
    }
    if let Some(recovery) = result.memory_soft_limit_recovery.as_ref() {
        lines.extend(
            crate::memory_soft_limit_recovery_display::format_memory_soft_limit_recovery_lines(
                recovery,
            ),
        );
    }
    lines.extend(crate::session_fix_finding_recovery::wait_summary_lines(
        session_dir,
    ));

    if let Some(changes) = result.uncommitted_changes.as_ref() {
        lines.push(crate::run_cmd::format_uncommitted_warning(changes));
    }
    if let Some(warning) = result.large_diff_warning.as_ref() {
        lines.push(crate::run_cmd::format_large_diff_warning_block(warning));
    }

    for warning in &result.warnings {
        lines.push(format!("Warning: {warning}"));
    }

    if let Some(report) = result.post_exec_gate.as_ref() {
        lines.push(format!(
            "Post-exec gate: {}",
            csa_session::post_exec_gate_failure_label(report)
        ));
    }

    let display_summary = wait_display_summary(session_dir, result, provider_quota.as_ref());
    let used_provider_quota = provider_quota
        .as_ref()
        .is_some_and(|quota| display_summary.as_deref() == Some(quota.summary.as_str()));
    if let Some(summary) = display_summary {
        lines.push(format!("Summary: {summary}"));
    }
    if let (true, Some(provider_quota)) = (used_provider_quota, provider_quota.as_ref()) {
        lines.push(format!("Hint: {}", provider_quota.hint));
    }

    lines.join("\n")
}

/// Render the per-tool failover chain as a single line for the wait summary,
/// e.g. `opencode: rate-limit-429; antigravity-cli: disabled; codex: disabled
/// → claude-code` (#1714). Returns `None` when no failover was recorded.
fn format_failover_chain_label(
    session_dir: &Path,
    result: &csa_session::SessionResult,
) -> Option<String> {
    if let Some(artifact) = read_review_verdict_artifact(session_dir)
        && artifact.decision == ReviewDecision::Unavailable
    {
        if let Some(message) = opaque_total_exhaustion_message(
            artifact.primary_failure.as_deref(),
            artifact.failure_reason.as_deref(),
        ) {
            return Some(message);
        }
        if result
            .fallback_chain
            .as_ref()
            .is_some_and(|chain| !chain.is_empty())
            && let Some(primary_failure) = artifact.primary_failure.as_deref()
            && !primary_failure.trim().is_empty()
        {
            return Some(primary_failure.trim().to_string());
        }
    }

    let chain = result.fallback_chain.as_ref()?;
    if chain.is_empty() {
        return None;
    }
    let steps = chain
        .iter()
        .map(|attempt| format!("{}: {}", attempt.tool, attempt.skip_reason))
        .collect::<Vec<_>>()
        .join("; ");
    let landed = result
        .fallback_tool
        .as_deref()
        .unwrap_or(result.tool.as_str());
    Some(format!("{steps} → {landed}"))
}

fn render_wait_result_json(
    session_dir: &Path,
    session_id: &str,
    result: &csa_session::SessionResult,
) -> Result<String> {
    let provider_quota =
        crate::session_provider_quota::provider_quota_display_for_result(session_dir, result);
    let tokens = extract_wait_token_summary(result).map(|usage| {
        serde_json::json!({
            "input_tokens": usage.input_tokens,
            "output_tokens": usage.output_tokens,
            "reasoning_output_tokens": usage.reasoning_output_tokens,
            "total_tokens": usage.total_tokens,
            "cache_read_input_tokens": usage.cache_read_input_tokens,
            "uncached_input_tokens": usage.uncached_input_tokens(),
            "cache_read_ratio": usage.cache_read_ratio(),
        })
    });
    let mut value = serde_json::json!({
        "session_id": session_id,
        "status": result.status,
        "exit_code": result.exit_code,
        "outcome": result.outcome_code(),
        "tool": wait_result_tool_label(result),
        "elapsed_seconds": wait_elapsed_seconds(result),
        "tokens": tokens,
        "review_verdict": read_review_verdict_label(session_dir, result),
        "unavailable_reason": crate::session_unavailable_reason::review_unavailable_reason_label(session_dir),
        "failover": format_failover_chain_label(session_dir, result),
        "kill_hint": result.kill_hint.as_deref(),
        "kill_diagnostics": result.kill_diagnostics.as_ref(),
        "require_commit_recovery": result.require_commit_recovery.as_ref(),
        "memory_soft_limit_recovery": result.memory_soft_limit_recovery.as_ref(),
        "fix_finding_recovery": crate::session_fix_finding_recovery::read_recovery_sidecar(session_dir),
        "post_exec_gate": result.post_exec_gate.as_ref(),
        "large_diff_warning": result.large_diff_warning.as_ref(),
        "warnings": result.warnings,
        "summary": wait_display_summary(session_dir, result, provider_quota.as_ref()),
        "provider_quota_hint": provider_quota.as_ref().map(|quota| quota.hint.as_str()),
    });
    crate::session_display_alias::apply_json_identity(&mut value, session_dir, session_id);
    serde_json::to_string_pretty(&value).map_err(Into::into)
}

fn wait_display_summary(
    session_dir: &Path,
    result: &csa_session::SessionResult,
    provider_quota: Option<&crate::session_provider_quota::ProviderQuotaDisplay>,
) -> Option<String> {
    if let Some(report) = result.post_exec_gate.as_ref() {
        return compact_wait_summary_text(&csa_session::post_exec_gate_failure_summary(report));
    }
    if let Some(text) =
        crate::session_summary_text::human_session_summary(session_dir, &result.summary)
            .and_then(|text| compact_wait_summary_text(&text))
    {
        return Some(text);
    }
    provider_quota.and_then(|quota| compact_wait_summary_text(&quota.summary))
}

fn wait_result_tool_label(result: &csa_session::SessionResult) -> String {
    match (&result.original_tool, &result.fallback_tool) {
        (Some(original), Some(fallback)) if original != fallback => {
            format!("{fallback} (fallback from {original})")
        }
        _ => result.tool.clone(),
    }
}

fn format_kill_hint_label(
    session_dir: &Path,
    result: &csa_session::SessionResult,
) -> Option<String> {
    let kill_hint = result.kill_hint.as_deref()?.trim();
    if kill_hint.is_empty() {
        return None;
    }
    let summary = crate::session_summary_text::human_session_summary(session_dir, &result.summary)
        .and_then(|text| compact_wait_summary_text(&text));
    if let Some(summary) = summary
        && summary.starts_with("CSA diagnostic:")
    {
        return Some(format!("Kill hint: {kill_hint} ({summary})"));
    }
    if let Some(diagnostics) = result.kill_diagnostics.as_ref() {
        return Some(format!(
            "Kill hint: {kill_hint} ({})",
            format_kill_diagnostics(diagnostics)
        ));
    }
    Some(format!("Kill hint: {kill_hint}"))
}

fn format_kill_diagnostics(diagnostics: &csa_session::KillDiagnosticReport) -> String {
    let mut parts = vec![format!("source={}", diagnostics.source)];
    if let Some(signal) = diagnostics.signal {
        parts.push(format!("signal={signal}"));
    }
    if let Some(current_mb) = diagnostics.current_mb {
        parts.push(format!("current_mb={current_mb}"));
    }
    if let Some(threshold_mb) = diagnostics.threshold_mb {
        parts.push(format!("threshold_mb={threshold_mb}"));
    }
    if let Some(memory_max_mb) = diagnostics.memory_max_mb {
        parts.push(format!("memory_max_mb={memory_max_mb}"));
    }
    if let Some(soft_limit_percent) = diagnostics.soft_limit_percent {
        parts.push(format!("soft_limit_percent={soft_limit_percent}"));
    }
    if let Some(scope_name) = diagnostics.scope_name.as_deref() {
        parts.push(format!("scope_name={scope_name}"));
    }
    parts.join(", ")
}

fn wait_elapsed_seconds(result: &csa_session::SessionResult) -> i64 {
    result
        .completed_at
        .signed_duration_since(result.started_at)
        .num_seconds()
        .max(0)
}

fn format_wait_elapsed(result: &csa_session::SessionResult) -> String {
    let seconds = wait_elapsed_seconds(result);
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        return format!("{hours}h {minutes}m {seconds}s");
    }
    if minutes > 0 {
        return format!("{minutes}m {seconds}s");
    }
    format!("{seconds}s")
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct WaitTokenSummary {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    reasoning_output_tokens: Option<u64>,
    total_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
}

impl WaitTokenSummary {
    fn render_text(self) -> String {
        let mut parts = Vec::new();
        if let Some(input) = self.input_tokens {
            parts.push(format!("input={input}"));
        }
        if let Some(output) = self.output_tokens {
            parts.push(format!("output={output}"));
        }
        if let Some(reasoning) = self.reasoning_output_tokens {
            parts.push(format!("reasoning_output={reasoning}"));
        }
        if let Some(total) = self.total_tokens {
            parts.push(format!("total={total}"));
        }
        if let Some(cache_read) = self.cache_read_input_tokens {
            parts.push(format!("cache_read={cache_read}"));
        }
        if let Some(uncached) = self.uncached_input_tokens() {
            parts.push(format!("uncached={uncached}"));
        }
        if let Some(ratio) = self.cache_read_ratio() {
            parts.push(format!("cache={:.0}%", ratio * 100.0));
        }
        parts.join(", ")
    }

    fn uncached_input_tokens(self) -> Option<u64> {
        Some(
            self.input_tokens?
                .saturating_sub(self.cache_read_input_tokens?),
        )
    }

    fn cache_read_ratio(self) -> Option<f64> {
        let input = self.input_tokens? as f64;
        if input == 0.0 {
            return None;
        }
        Some(self.cache_read_input_tokens? as f64 / input)
    }
}

fn extract_wait_token_summary(result: &csa_session::SessionResult) -> Option<WaitTokenSummary> {
    let summary_json: serde_json::Value = serde_json::from_str(&result.summary).ok()?;
    let usage = summary_json.get("usage")?;
    let input_tokens = usage
        .get("input_tokens")
        .and_then(serde_json::Value::as_u64);
    let output_tokens = usage
        .get("output_tokens")
        .and_then(serde_json::Value::as_u64);
    let reasoning_output_tokens = usage
        .get("reasoning_output_tokens")
        .or_else(|| usage.get("reasoning_tokens"))
        .or_else(|| {
            usage
                .get("output_tokens_details")
                .and_then(|details| details.get("reasoning_tokens"))
        })
        .or_else(|| {
            usage
                .get("completion_tokens_details")
                .and_then(|details| details.get("reasoning_tokens"))
        })
        .and_then(serde_json::Value::as_u64);
    let total_tokens = usage
        .get("total_tokens")
        .and_then(serde_json::Value::as_u64)
        .or_else(|| {
            input_tokens
                .zip(output_tokens)
                .map(|(input, output)| input + output)
        });
    let cache_read_input_tokens = usage
        .get("cache_read_input_tokens")
        .or_else(|| usage.get("cached_input_tokens"))
        .or_else(|| {
            usage
                .get("input_tokens_details")
                .and_then(|details| details.get("cached_tokens"))
        })
        .or_else(|| {
            usage
                .get("prompt_tokens_details")
                .and_then(|details| details.get("cached_tokens"))
        })
        .and_then(serde_json::Value::as_u64);
    Some(WaitTokenSummary {
        input_tokens,
        output_tokens,
        reasoning_output_tokens,
        total_tokens,
        cache_read_input_tokens,
    })
}

fn compact_wait_summary_text(summary: &str) -> Option<String> {
    let summary = summary.trim();
    if summary.is_empty() {
        return None;
    }
    const MAX_CHARS: usize = 500;
    let mut compact = summary.replace(['\r', '\n'], " ");
    if compact.chars().count() > MAX_CHARS {
        compact = compact.chars().take(MAX_CHARS).collect::<String>();
        compact.push_str("...");
    }
    Some(compact)
}

fn read_review_verdict_label(
    session_dir: &Path,
    result: &csa_session::SessionResult,
) -> Option<String> {
    let summary_requires_failed_gate =
        crate::session_observability::human_review_summary_requires_failed_gate(
            session_dir,
            &result.summary,
        );
    if let Some(artifact) = read_review_verdict_artifact(session_dir) {
        let meta = read_review_meta_for_label(session_dir);
        if let Some(label) = meta
            .as_ref()
            .and_then(|meta| format_fix_loop_noop_label(meta.failure_reason.as_deref()))
            .or_else(|| format_fix_loop_noop_label(artifact.failure_reason.as_deref()))
        {
            return Some(label);
        }
        if summary_requires_failed_gate {
            return Some("FAIL".to_string());
        }
        if artifact.decision == ReviewDecision::Pass {
            if !wait_result_allows_pass_verdict(result) {
                return Some("UNAVAILABLE".to_string());
            }
            if meta.as_ref().is_some_and(|meta| {
                meta.requires_fail_closed_verdict() || !meta.fix_clean_converged()
            }) {
                return Some("UNAVAILABLE".to_string());
            }
            return Some("PASS".to_string());
        }
        if artifact.decision == ReviewDecision::Unavailable
            && let Some(primary_failure) = artifact.primary_failure.as_deref()
            && !primary_failure.trim().is_empty()
        {
            return Some(format!("UNAVAILABLE ({})", primary_failure.trim()));
        }
        return Some(normalize_review_verdict_label(
            artifact.decision.as_str(),
            result,
        ));
    }

    let meta_path = session_dir.join("review_meta.json");
    if meta_path.is_file()
        && let Ok(raw) = std::fs::read_to_string(&meta_path)
        && let Ok(meta) = serde_json::from_str::<ReviewSessionMeta>(&raw)
    {
        if let Some(label) = format_fix_loop_noop_label(meta.failure_reason.as_deref()) {
            return Some(label);
        }
        if summary_requires_failed_gate {
            return Some("FAIL".to_string());
        }
        if meta.fix_attempted && !meta.fix_clean_converged() {
            return Some("UNAVAILABLE".to_string());
        }
        return Some(normalize_review_verdict_label(&meta.decision, result));
    }

    if summary_requires_failed_gate {
        return Some("FAIL".to_string());
    }

    None
}

fn format_fix_loop_noop_label(reason: Option<&str>) -> Option<String> {
    let reason = reason?.strip_prefix("fix_loop_noop:")?.trim();
    if reason.is_empty() {
        return None;
    }
    Some(format!("FIX-LOOP-NO-OP ({reason})"))
}

fn read_review_verdict_artifact(session_dir: &Path) -> Option<ReviewVerdictArtifact> {
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    if !verdict_path.is_file() {
        return None;
    }
    let raw = std::fs::read_to_string(&verdict_path).ok()?;
    serde_json::from_str::<ReviewVerdictArtifact>(&raw).ok()
}

fn read_review_meta_for_label(session_dir: &Path) -> Option<ReviewSessionMeta> {
    let meta_path = session_dir.join("review_meta.json");
    if !meta_path.is_file() {
        return None;
    }
    let raw = std::fs::read_to_string(&meta_path).ok()?;
    serde_json::from_str::<ReviewSessionMeta>(&raw).ok()
}

fn wait_result_allows_pass_verdict(result: &csa_session::SessionResult) -> bool {
    result.exit_code == 0 && result.status.trim().eq_ignore_ascii_case("success")
}

fn normalize_review_verdict_label(value: &str, result: &csa_session::SessionResult) -> String {
    match value.trim().to_ascii_uppercase().as_str() {
        "PASS" | "CLEAN" if !wait_result_allows_pass_verdict(result) => "UNAVAILABLE".to_string(),
        "PASS" | "CLEAN" => "PASS".to_string(),
        "FAIL" | "FAILED" | "HAS_ISSUES" => "FAIL".to_string(),
        other => other.to_string(),
    }
}

struct WaitOutputLog {
    raw: Vec<u8>,
    truncated: bool,
}

fn read_wait_output_log(stdout_log: &Path) -> Result<WaitOutputLog> {
    let mut file = File::open(stdout_log)?;
    let len = file.metadata()?.len();
    if len <= WAIT_OUTPUT_MAX_BYTES {
        let mut raw = Vec::with_capacity(len as usize);
        file.read_to_end(&mut raw)?;
        return Ok(WaitOutputLog {
            raw,
            truncated: false,
        });
    }

    let start = len - WAIT_OUTPUT_MAX_BYTES;
    file.seek(SeekFrom::Start(start))?;
    let mut reader = BufReader::new(file);
    discard_partial_line_if_needed(&mut reader, stdout_log, start)?;
    let mut raw = Vec::with_capacity(WAIT_OUTPUT_MAX_BYTES as usize);
    reader.take(WAIT_OUTPUT_MAX_BYTES).read_to_end(&mut raw)?;
    Ok(WaitOutputLog {
        raw,
        truncated: true,
    })
}

fn discard_partial_line_if_needed(
    reader: &mut BufReader<File>,
    stdout_log: &Path,
    start: u64,
) -> Result<()> {
    if start == 0 {
        return Ok(());
    }
    let mut boundary = File::open(stdout_log)?;
    boundary.seek(SeekFrom::Start(start - 1))?;
    let mut previous = [0_u8; 1];
    boundary.read_exact(&mut previous)?;
    if previous[0] == b'\n' {
        return Ok(());
    }
    let mut discarded = Vec::new();
    reader.read_until(b'\n', &mut discarded)?;
    Ok(())
}

fn render_wait_output_log(raw: &[u8], truncated: bool) -> Option<String> {
    if truncated {
        let raw_text = String::from_utf8_lossy(raw);
        return crate::codex_transcript_filter::extract_codex_json_event_text(raw_text.as_ref())
            .or_else(|| crate::codex_transcript_filter::render_codex_or_plain_output(raw));
    }
    crate::codex_transcript_filter::render_codex_or_plain_output(raw)
}

#[cfg(test)]
#[path = "session_cmds_daemon_wait_summary_alias_tests.rs"]
mod wait_alias_tests;
#[cfg(test)]
#[path = "session_cmds_daemon_wait_summary_tests.rs"]
mod wait_output_tests;
