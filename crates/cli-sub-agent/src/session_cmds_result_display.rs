use anyhow::Result;
use csa_session::state::ReviewSessionMeta;
use csa_session::{SessionResultView, TokenUsage};
use std::fs;
use std::path::Path;

use super::memory_soft_limit_result_display::{insert, lines};
use super::{StructuredOutputOpts, TranscriptSummary};
use crate::require_commit_recovery_display::{
    build_require_commit_recovery_guidance_for_display_session,
    format_require_commit_recovery_lines_for_display_session,
};
use crate::review_failure_context as rfc;
use crate::session_display_alias;
use crate::session_provider_quota::{
    ProviderQuotaDisplay, provider_quota_display_for_result, provider_quota_display_for_session_dir,
};
use crate::session_summary_text::{enrich_review_summary, human_session_summary};
use crate::session_unavailable_reason::review_unavailable_reason_label;
use crate::token_usage_display::{display_total_tokens, token_usage_json_value};

#[path = "session_cmds_result_post_exec_gate.rs"]
mod post_exec_gate;
#[path = "session_cmds_result_pre_exec_summary.rs"]
mod pre_exec_summary;
pub(super) use post_exec_gate::{
    build_all_sections_json_payload, build_gate_aware_summary_content,
    build_summary_section_json_payload, gate_summary_employee_section,
    load_structured_post_exec_gate_report, structured_sections_with_gate_first,
};
use post_exec_gate::{
    gate_failure_rendered_section, print_rendered_sections, rendered_fallback_sections,
};

pub(super) fn display_result_json_with_identity(
    session_id: &str,
    session_dir: &Path,
    result: &SessionResultView,
    transcript_summary: Option<&TranscriptSummary>,
    review_meta: Option<&ReviewSessionMeta>,
    token_usage: Option<&TokenUsage>,
) -> Result<()> {
    let payload = build_result_json_payload_with_identity(
        session_id,
        session_dir,
        result,
        transcript_summary,
        review_meta,
        token_usage,
    )?;
    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(())
}

pub(super) fn display_result_text(
    session_id: &str,
    session_dir: &Path,
    result: &SessionResultView,
    transcript_summary: Option<&TranscriptSummary>,
    review_meta: Option<&ReviewSessionMeta>,
    token_usage: Option<&TokenUsage>,
) {
    let envelope = &result.envelope;
    let provider_quota = provider_quota_display_for_result(session_dir, envelope);
    println!("Session: {session_id}");
    for line in session_display_alias::text_lines(session_dir, session_id) {
        println!("{line}");
    }
    println!("Status:  {}", envelope.status);
    println!("Exit:    {}", envelope.exit_code);
    println!("Tool:    {}", envelope.tool);
    println!("Started: {}", envelope.started_at);
    println!("Ended:   {}", envelope.completed_at);
    if let Some(report) = envelope.post_exec_gate.as_ref() {
        println!(
            "Post-exec gate: {}",
            csa_session::post_exec_gate_failure_label(report)
        );
    }
    let display_summary = result_display_summary(session_dir, envelope, provider_quota.as_ref());
    let used_provider_quota = provider_quota
        .as_ref()
        .is_some_and(|quota| display_summary.as_deref() == Some(quota.summary.as_str()));
    if let Some(summary) = display_summary {
        println!("Summary: {summary}");
    }
    rfc::print(session_dir);
    if let (true, Some(provider_quota)) = (used_provider_quota, provider_quota.as_ref()) {
        println!("Hint: {}", provider_quota.hint);
    }
    if let Some(reason) = review_unavailable_reason_label(session_dir) {
        println!("Unavailable reason: {reason}");
    }
    if let Some(kill_hint) = envelope.kill_hint.as_deref() {
        println!("Kill hint: {kill_hint}");
    }
    if let Some(diagnostics) = envelope.kill_diagnostics.as_ref() {
        println!("Kill diagnostics: {}", format_kill_diagnostics(diagnostics));
    }
    if let Some(recovery) = envelope.require_commit_recovery.as_ref() {
        for line in format_require_commit_recovery_lines_for_display_session(
            session_dir,
            session_id,
            recovery,
        ) {
            println!("{line}");
        }
    }
    if let Some(recovery) = envelope.memory_soft_limit_recovery.as_ref() {
        for line in lines(session_id, session_dir, envelope, recovery) {
            println!("{line}");
        }
    }
    if !envelope.artifacts.is_empty() {
        println!("Artifacts:");
        for a in &envelope.artifacts {
            println!("  - {a}");
        }
    }
    display_sidecar("Manager Sidecar", result.manager_sidecar.as_ref());
    display_sidecar("Legacy Sidecar", result.legacy_sidecar.as_ref());
    if let Some(meta) = review_meta {
        println!("Review Iterations: {}", meta.review_iterations);
    }
    if let Some(usage) = token_usage {
        print_token_usage(usage);
    }
    if let Some(summary) = transcript_summary {
        println!("Transcript:");
        println!("  Events: {}", summary.event_count);
        println!("  Size:   {} bytes", summary.size_bytes);
        println!(
            "  First:  {}",
            summary.first_timestamp.as_deref().unwrap_or("-")
        );
        println!(
            "  Last:   {}",
            summary.last_timestamp.as_deref().unwrap_or("-")
        );
    }
}

pub(super) fn load_total_token_usage(session_dir: &Path) -> Option<TokenUsage> {
    let state_path = session_dir.join("state.toml");
    let content = fs::read_to_string(&state_path).ok()?;
    let value: toml::Value = toml::from_str(&content).ok()?;
    let usage_table = value.get("total_token_usage")?;
    usage_table.clone().try_into::<TokenUsage>().ok()
}

fn print_token_usage(usage: &TokenUsage) {
    for line in render_token_usage_lines(usage) {
        println!("{line}");
    }
}

pub(super) fn render_token_usage_lines(usage: &TokenUsage) -> Vec<String> {
    let any_field = usage.input_tokens.is_some()
        || usage.output_tokens.is_some()
        || usage.reasoning_output_tokens.is_some()
        || usage.total_tokens.is_some()
        || usage.estimated_cost_usd.is_some()
        || usage.cache_read_input_tokens.is_some();
    if !any_field {
        return Vec::new();
    }

    let mut lines = vec!["Tokens:".to_string()];
    if let Some(v) = usage.input_tokens {
        lines.push(format!("  Input:  {} tokens", format_thousands(v)));
    }
    if let Some(v) = usage.cache_read_input_tokens {
        if let Some(ratio) = usage.cache_read_ratio() {
            lines.push(format!(
                "  Cache read: {} tokens ({:.0}% hit rate)",
                format_thousands(v),
                ratio * 100.0
            ));
        } else {
            lines.push(format!("  Cache read: {} tokens", format_thousands(v)));
        }
    }
    if let Some(v) = usage.uncached_input_tokens() {
        lines.push(format!("  Uncached input: {} tokens", format_thousands(v)));
    }
    if let Some(v) = usage.output_tokens {
        lines.push(format!("  Output: {} tokens", format_thousands(v)));
    }
    if let Some(v) = usage.reasoning_output_tokens {
        lines.push(format!(
            "  Reasoning output: {} tokens",
            format_thousands(v)
        ));
    }
    if let Some(v) = display_total_tokens(usage) {
        lines.push(format!("  Total:  {} tokens", format_thousands(v)));
    }
    if let Some(cost) = usage.estimated_cost_usd {
        lines.push(format!("  Cost:   ${cost:.4}"));
    }
    lines
}

fn format_thousands(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len + len / 3);
    for (idx, byte) in bytes.iter().enumerate() {
        if idx > 0 && (len - idx).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*byte as char);
    }
    out
}

pub(super) fn build_result_json_payload(
    result: &SessionResultView,
    transcript_summary: Option<&TranscriptSummary>,
    review_meta: Option<&ReviewSessionMeta>,
    token_usage: Option<&TokenUsage>,
) -> Result<serde_json::Value> {
    let mut payload = serde_json::to_value(&result.envelope)?;
    if let Some(report) = result.envelope.post_exec_gate.as_ref() {
        let authoritative_summary = csa_session::post_exec_gate_failure_summary(report);
        if result.envelope.summary.trim() != authoritative_summary.as_str() {
            payload["superseded_employee_summary"] =
                serde_json::Value::String(result.envelope.summary.clone());
        }
        payload["summary"] = serde_json::Value::String(authoritative_summary);
    }
    if let Some(outcome) = result.envelope.outcome_code() {
        payload["outcome"] = serde_json::Value::String(outcome.to_string());
    }
    if let Some(sidecar) = result
        .manager_sidecar
        .as_ref()
        .and_then(redact_result_sidecar_for_json)
    {
        payload["manager_sidecar"] = serde_json::to_value(sidecar)?;
    }
    if let Some(sidecar) = result
        .legacy_sidecar
        .as_ref()
        .and_then(redact_result_sidecar_for_json)
    {
        payload["legacy_sidecar"] = serde_json::to_value(sidecar)?;
    }
    if let Some(summary) = transcript_summary {
        payload["transcript_summary"] = serde_json::json!({
            "event_count": summary.event_count,
            "size_bytes": summary.size_bytes,
            "first_timestamp": summary.first_timestamp,
            "last_timestamp": summary.last_timestamp,
        });
    }
    if let Some(meta) = review_meta {
        payload["review_meta"] = serde_json::to_value(meta)?;
    }
    if let Some(usage) = token_usage {
        let usage = normalized_token_usage_for_output(usage);
        payload["total_token_usage"] = token_usage_json_value(&usage);
    }
    Ok(payload)
}

pub(super) fn build_result_json_payload_with_identity(
    session_id: &str,
    session_dir: &Path,
    result: &SessionResultView,
    transcript_summary: Option<&TranscriptSummary>,
    review_meta: Option<&ReviewSessionMeta>,
    token_usage: Option<&TokenUsage>,
) -> Result<serde_json::Value> {
    let mut payload =
        build_result_json_payload(result, transcript_summary, review_meta, token_usage)?;
    session_display_alias::apply_json_identity(&mut payload, session_dir, session_id);
    if let Some(recovery) = result.envelope.require_commit_recovery.as_ref() {
        payload["require_commit_recovery_guidance"] =
            build_require_commit_recovery_guidance_for_display_session(
                session_dir,
                session_id,
                recovery,
            )
            .to_json();
    }
    insert(&mut payload, session_id, session_dir, &result.envelope);
    rfc::insert_json(&mut payload, session_dir);
    Ok(payload)
}

fn result_display_summary(
    session_dir: &Path,
    envelope: &csa_session::SessionResult,
    provider_quota: Option<&ProviderQuotaDisplay>,
) -> Option<String> {
    if let Some(report) = envelope.post_exec_gate.as_ref() {
        return Some(csa_session::post_exec_gate_failure_summary(report));
    }
    if let Some(summary) = human_session_summary(session_dir, &envelope.summary)
        .map(|summary| enrich_review_summary(session_dir, &summary))
    {
        return Some(summary);
    }
    provider_quota.map(|quota| quota.summary.clone())
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

fn normalized_token_usage_for_output(usage: &TokenUsage) -> TokenUsage {
    let mut normalized = usage.clone();
    normalized.total_tokens = display_total_tokens(usage);
    normalized
}

fn display_sidecar(label: &str, sidecar: Option<&toml::Value>) {
    if let Some(rendered) = sidecar.and_then(render_result_sidecar_for_text) {
        println!("{label}:");
        print_rendered_sidecar(&rendered, 2);
    }
}

pub(super) fn render_result_sidecar_for_text(sidecar: &toml::Value) -> Option<String> {
    match csa_session::render_redacted_result_sidecar(sidecar) {
        Ok(rendered) if rendered.trim().is_empty() => None,
        Ok(rendered) => Some(rendered),
        Err(err) => Some(format!("<failed to render TOML sidecar: {err}>")),
    }
}

fn redact_result_sidecar_for_json(sidecar: &toml::Value) -> Option<toml::Value> {
    match csa_session::redact_result_sidecar_value(sidecar) {
        Ok(toml::Value::Table(table)) if table.is_empty() => None,
        Ok(value) => Some(value),
        Err(_) => Some(toml::Value::String(
            "<failed to render TOML sidecar>".to_string(),
        )),
    }
}

fn print_rendered_sidecar(rendered: &str, indent: usize) {
    let padding = " ".repeat(indent);
    for line in rendered.lines() {
        println!("{padding}{line}");
    }
}

include!("session_cmds_result_sections.rs");
