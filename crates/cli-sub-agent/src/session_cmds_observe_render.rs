use super::{SessionPeekReport, SessionStatsBucket, SessionStatsGroup, SessionStatsReport};

pub(super) fn render_peek_text(report: &SessionPeekReport) -> String {
    let mut out = String::new();
    out.push_str(&format!("Session: {}\n", report.session_id));
    out.push_str(&format!("State: {}\n", report.state));
    out.push_str(&format!(
        "Idle: {} / timeout {}\n",
        format_secs(report.idle_secs),
        format_secs(report.idle_timeout_secs)
    ));
    out.push_str(&format!("Elapsed: {}\n", format_secs(report.elapsed_secs)));
    out.push_str(&format!("Last accessed: {}\n", report.last_accessed));
    if let Some(status) = &report.result_status {
        out.push_str(&format!(
            "Result: {} exit={}\n",
            status,
            report.result_exit_code.unwrap_or_default()
        ));
    }
    out.push_str("Operations:\n");
    if report.operations.is_empty() {
        out.push_str("  -\n");
    } else {
        for op in &report.operations {
            let tool = op
                .tool
                .as_deref()
                .map(|tool| format!(" tool={tool}"))
                .unwrap_or_default();
            let exit = op
                .exit_code
                .map(|code| format!(" exit={code}"))
                .unwrap_or_default();
            out.push_str(&format!(
                "  {} ({} ago) {}{}{}: {}\n",
                op.timestamp,
                format_secs(op.age_secs),
                op.kind,
                tool,
                exit,
                op.summary
            ));
        }
    }
    out
}

pub(super) fn render_stats_text(report: &SessionStatsReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Sessions since {}: {}\n",
        report.since, report.total.session_count
    ));
    out.push_str(&format!(
        "Wall-clock span: {}\n",
        format_secs(report.total.wall_clock_span_secs)
    ));
    append_bucket_detail(&mut out, &report.total);

    if !report.by_issue.is_empty() {
        out.push_str("By issue:\n");
        for group in &report.by_issue {
            append_group_line(&mut out, group);
        }
    }

    if !report.by_tool.is_empty() {
        out.push_str("By tool:\n");
        for group in &report.by_tool {
            append_group_line(&mut out, group);
        }
    }

    out
}

fn append_bucket_detail(out: &mut String, bucket: &SessionStatsBucket) {
    out.push_str(&format!(
        "Idle gaps: {} total, {} stuck across {} session(s)\n",
        format_secs(bucket.idle_gap_secs),
        format_secs(bucket.stuck_gap_secs),
        bucket.stuck_session_count
    ));
    out.push_str(&format!(
        "Tokens: uncached_input={} cached_input={} output={} total={}\n",
        bucket.tokens.uncached_input_tokens,
        bucket.tokens.cached_input_tokens,
        bucket.tokens.output_tokens,
        bucket.tokens.total_tokens
    ));
    if let Some(cost) = &bucket.cost {
        match cost.estimated_usd {
            Some(value) => out.push_str(&format!(
                "Cost: ${value:.4} ({:?}, {} session(s))\n",
                cost.source, cost.sessions_with_recorded_cost
            )),
            None => out.push_str(
                "Cost: unknown (no authoritative pricing table or recorded positive estimate)\n",
            ),
        }
    }
}

fn append_group_line(out: &mut String, group: &SessionStatsGroup) {
    out.push_str(&format!(
        "  {}: sessions={} wall={} idle={} stuck={} tokens={}\n",
        group.key,
        group.bucket.session_count,
        format_secs(group.bucket.wall_clock_span_secs),
        format_secs(group.bucket.idle_gap_secs),
        format_secs(group.bucket.stuck_gap_secs),
        group.bucket.tokens.total_tokens
    ));
}

fn format_secs(secs: u64) -> String {
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let minutes = (secs % 3_600) / 60;
    let seconds = secs % 60;

    if days > 0 {
        format!("{days}d{hours}h")
    } else if hours > 0 {
        format!("{hours}h{minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m{seconds}s")
    } else {
        format!("{seconds}s")
    }
}
