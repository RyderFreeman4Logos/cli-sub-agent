//! Token-bounded, transport-agnostic session observability.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use csa_core::types::OutputFormat;
use csa_session::{MetaSessionState, SessionPhase, SessionResult};
use serde::Serialize;

use crate::stdout_write::{write_stdout, write_stdout_line};

#[path = "session_cmds_observe_render.rs"]
mod render;

use self::render::{render_peek_text, render_stats_text};

const DEFAULT_PEEK_OPERATIONS: usize = 5;
const UNKNOWN_GROUP: &str = "unknown";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PeekState {
    Working,
    Idle,
    Dead,
}

impl std::fmt::Display for PeekState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Working => write!(f, "working"),
            Self::Idle => write!(f, "idle"),
            Self::Dead => write!(f, "dead"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct SessionOperation {
    pub timestamp: DateTime<Utc>,
    pub age_secs: u64,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct SessionPeekReport {
    pub session_id: String,
    pub state: PeekState,
    pub idle_secs: u64,
    pub elapsed_secs: u64,
    pub idle_timeout_secs: u64,
    pub created_at: DateTime<Utc>,
    pub last_accessed: DateTime<Utc>,
    pub phase: SessionPhase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_exit_code: Option<i32>,
    pub session_dir: PathBuf,
    pub operations: Vec<SessionOperation>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TokenRollup {
    pub uncached_input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub sessions_with_tokens: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CostSource {
    RecordedSessionEstimates,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct CostRollup {
    pub estimated_usd: Option<f64>,
    pub source: CostSource,
    pub sessions_with_recorded_cost: usize,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub(crate) struct SessionStatsBucket {
    pub session_count: usize,
    pub wall_clock_span_secs: u64,
    pub idle_gap_secs: u64,
    pub stuck_gap_secs: u64,
    pub stuck_session_count: usize,
    pub tokens: TokenRollup,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<CostRollup>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct SessionStatsGroup {
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue_source: Option<String>,
    #[serde(flatten)]
    pub bucket: SessionStatsBucket,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct SessionStatsReport {
    pub generated_at: DateTime<Utc>,
    pub since: String,
    pub since_secs: i64,
    pub cutoff: DateTime<Utc>,
    #[serde(flatten)]
    pub total: SessionStatsBucket,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub by_issue: Vec<SessionStatsGroup>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub by_tool: Vec<SessionStatsGroup>,
}

#[derive(Debug, Clone)]
struct SessionStatsRecord {
    session: MetaSessionState,
    state: PeekState,
    idle_secs: u64,
    stuck_gap_secs: u64,
    end_at: DateTime<Utc>,
    issue_key: String,
    issue_source: IssueSource,
    tool_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IssueSource {
    Explicit,
    Heuristic,
    Unknown,
}

impl IssueSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Heuristic => "heuristic",
            Self::Unknown => "unknown",
        }
    }
}

pub(crate) fn handle_session_peek(
    session: String,
    operations: Option<usize>,
    cd: Option<String>,
    format: OutputFormat,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let resolved = super::resolve_session_prefix_with_global_fallback(&project_root, &session)?;
    let session_dir = resolved.sessions_dir.join(&resolved.session_id);
    let effective_root = resolved
        .foreign_project_root
        .as_deref()
        .unwrap_or(&project_root);
    let idle_timeout_secs = resolve_idle_timeout_secs(effective_root)?;

    let report = build_peek_report(
        &resolved.session_id,
        &session_dir,
        effective_root,
        operations.unwrap_or(DEFAULT_PEEK_OPERATIONS),
        idle_timeout_secs,
        Utc::now(),
    )?;

    match format {
        OutputFormat::Json => write_stdout_line(&serde_json::to_string_pretty(&report)?)?,
        OutputFormat::Text => write_stdout(&render_peek_text(&report))?,
    }
    Ok(())
}

pub(crate) fn handle_session_stats(
    since: String,
    by_issue: bool,
    by_tool: bool,
    include_cost: bool,
    cd: Option<String>,
    format: OutputFormat,
) -> Result<()> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let duration = super::parse_duration_filter(&since)?;
    let now = Utc::now();
    let report = build_stats_report(
        &project_root,
        since,
        duration,
        by_issue,
        by_tool,
        include_cost,
        now,
    )?;

    match format {
        OutputFormat::Json => write_stdout_line(&serde_json::to_string_pretty(&report)?)?,
        OutputFormat::Text => write_stdout(&render_stats_text(&report))?,
    }
    Ok(())
}

fn resolve_idle_timeout_secs(project_root: &Path) -> Result<u64> {
    let config = csa_config::ProjectConfig::load(project_root)?;
    Ok(crate::pipeline::resolve_idle_timeout_seconds(
        config.as_ref(),
        None,
    ))
}

fn build_peek_report(
    session_id: &str,
    session_dir: &Path,
    project_root: &Path,
    operation_limit: usize,
    idle_timeout_secs: u64,
    now: DateTime<Utc>,
) -> Result<SessionPeekReport> {
    let mut session = load_session_from_dir(session_dir)
        .with_context(|| format!("failed to load session state for {session_id}"))?;
    let state = classify_session_liveness(session_dir);
    if state == PeekState::Dead && matches!(session.phase, SessionPhase::Active) {
        match super::ensure_terminal_result_for_dead_active_session(
            project_root,
            session_id,
            "session peek",
        ) {
            Ok(reconciliation) if reconciliation.result_became_available() => {
                session = load_session_from_dir(session_dir).with_context(|| {
                    format!(
                        "failed to reload session state for {session_id} after dead-session reconciliation"
                    )
                })?;
            }
            Ok(_) => {}
            Err(err) => {
                tracing::warn!(session_id, error = %err, "Failed to reconcile dead Active session in session peek");
            }
        }
    }

    let result = load_result_from_dir(session_dir)?;
    let created_at = super::list::session_created_at(&session);
    let elapsed_secs = nonnegative_secs(now - created_at);
    let idle_secs = nonnegative_secs(now - session.last_accessed);
    let operations = collect_operations(&session, result.as_ref(), now, operation_limit);

    Ok(SessionPeekReport {
        session_id: session_id.to_string(),
        state,
        idle_secs,
        elapsed_secs,
        idle_timeout_secs,
        created_at,
        last_accessed: session.last_accessed,
        phase: session.phase,
        result_status: result.as_ref().map(|result| result.status.clone()),
        result_exit_code: result.as_ref().map(|result| result.exit_code),
        session_dir: session_dir.to_path_buf(),
        operations,
    })
}

fn build_stats_report(
    project_root: &Path,
    since: String,
    duration: Duration,
    by_issue: bool,
    by_tool: bool,
    include_cost: bool,
    now: DateTime<Utc>,
) -> Result<SessionStatsReport> {
    let cutoff = now - duration;
    let idle_timeout_secs = resolve_idle_timeout_secs(project_root)?;
    let mut sessions = csa_session::list_sessions_readonly(project_root, None)?;
    sessions.retain(|session| session.last_accessed >= cutoff);

    let mut records = Vec::with_capacity(sessions.len());
    for session in sessions {
        let session_dir = csa_session::get_session_dir(project_root, &session.meta_session_id)?;
        let result = load_result_from_dir(&session_dir)?;
        let state = classify_session_liveness(&session_dir);
        let idle_secs = if matches!(session.phase, SessionPhase::Active) {
            nonnegative_secs(now - session.last_accessed)
        } else {
            0
        };
        let stuck_gap_secs = idle_secs.saturating_sub(idle_timeout_secs);
        let end_at = if matches!(session.phase, SessionPhase::Active)
            && matches!(state, PeekState::Working | PeekState::Idle)
        {
            now
        } else {
            session
                .last_accessed
                .max(super::list::session_created_at(&session))
        };
        let (issue_key, issue_source) = issue_key_for_session(&session);
        let tool_key = tool_key_for_session(&session, result.as_ref());

        records.push(SessionStatsRecord {
            session,
            state,
            idle_secs,
            stuck_gap_secs,
            end_at,
            issue_key,
            issue_source,
            tool_key,
        });
    }

    let total = build_bucket(&records, include_cost);
    let by_issue_groups = if by_issue {
        build_groups(&records, include_cost, |record| {
            (
                record.issue_key.clone(),
                Some(record.issue_source.as_str().to_string()),
            )
        })
    } else {
        Vec::new()
    };
    let by_tool_groups = if by_tool {
        build_groups(&records, include_cost, |record| {
            (record.tool_key.clone(), None)
        })
    } else {
        Vec::new()
    };

    Ok(SessionStatsReport {
        generated_at: now,
        since,
        since_secs: duration.num_seconds(),
        cutoff,
        total,
        by_issue: by_issue_groups,
        by_tool: by_tool_groups,
    })
}

fn load_session_from_dir(session_dir: &Path) -> Result<MetaSessionState> {
    let state_path = session_dir.join("state.toml");
    let content = std::fs::read_to_string(&state_path)
        .with_context(|| format!("failed to read {}", state_path.display()))?;
    toml::from_str(&content).with_context(|| format!("failed to parse {}", state_path.display()))
}

fn load_result_from_dir(session_dir: &Path) -> Result<Option<SessionResult>> {
    let result_path = session_dir.join(csa_session::result::RESULT_FILE_NAME);
    if !result_path.is_file() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&result_path)
        .with_context(|| format!("failed to read {}", result_path.display()))?;
    let result = toml::from_str(&content)
        .with_context(|| format!("failed to parse {}", result_path.display()))?;
    Ok(Some(result))
}

fn classify_session_liveness(session_dir: &Path) -> PeekState {
    if csa_process::ToolLiveness::is_working(session_dir) {
        PeekState::Working
    } else if csa_process::ToolLiveness::is_alive_read_only(session_dir) {
        PeekState::Idle
    } else {
        PeekState::Dead
    }
}

fn collect_operations(
    session: &MetaSessionState,
    result: Option<&SessionResult>,
    now: DateTime<Utc>,
    limit: usize,
) -> Vec<SessionOperation> {
    let mut operations = Vec::new();

    for (tool, state) in &session.tools {
        operations.push(SessionOperation {
            timestamp: state.updated_at,
            age_secs: nonnegative_secs(now - state.updated_at),
            kind: "tool".to_string(),
            tool: Some(tool.clone()),
            exit_code: Some(state.last_exit_code),
            summary: state.last_action_summary.clone(),
        });
    }

    if let Some(result) = result {
        operations.push(SessionOperation {
            timestamp: result.completed_at,
            age_secs: nonnegative_secs(now - result.completed_at),
            kind: "result".to_string(),
            tool: Some(result.tool.clone()),
            exit_code: Some(result.exit_code),
            summary: result.summary.clone(),
        });
    }

    operations.push(SessionOperation {
        timestamp: session.last_accessed,
        age_secs: nonnegative_secs(now - session.last_accessed),
        kind: "session_state".to_string(),
        tool: None,
        exit_code: None,
        summary: format!("phase={}", session.phase),
    });

    operations.sort_by_key(|operation| std::cmp::Reverse(operation.timestamp));
    operations.truncate(limit);
    operations
}

fn build_bucket(records: &[SessionStatsRecord], include_cost: bool) -> SessionStatsBucket {
    let session_count = records.len();
    let wall_clock_span_secs = wall_clock_span_secs(records);
    let idle_gap_secs = records.iter().map(|record| record.idle_secs).sum::<u64>();
    let stuck_gap_secs = records
        .iter()
        .map(|record| record.stuck_gap_secs)
        .sum::<u64>();
    let stuck_session_count = records
        .iter()
        .filter(|record| {
            record.stuck_gap_secs > 0
                || (record.state == PeekState::Dead
                    && matches!(record.session.phase, SessionPhase::Active))
        })
        .count();
    let tokens = rollup_tokens(records.iter().map(|record| &record.session));
    let cost = include_cost.then(|| rollup_cost(records.iter().map(|record| &record.session)));

    SessionStatsBucket {
        session_count,
        wall_clock_span_secs,
        idle_gap_secs,
        stuck_gap_secs,
        stuck_session_count,
        tokens,
        cost,
    }
}

fn build_groups<F>(
    records: &[SessionStatsRecord],
    include_cost: bool,
    key_fn: F,
) -> Vec<SessionStatsGroup>
where
    F: Fn(&SessionStatsRecord) -> (String, Option<String>),
{
    let mut grouped: BTreeMap<String, (Option<String>, Vec<SessionStatsRecord>)> = BTreeMap::new();
    for record in records {
        let (key, issue_source) = key_fn(record);
        let entry = grouped.entry(key).or_insert_with(|| (None, Vec::new()));
        entry.0 = preferred_issue_source(entry.0.take(), issue_source);
        entry.1.push(record.clone());
    }

    grouped
        .into_iter()
        .map(|(key, (issue_source, records))| SessionStatsGroup {
            key,
            issue_source,
            bucket: build_bucket(&records, include_cost),
        })
        .collect()
}

fn preferred_issue_source(current: Option<String>, candidate: Option<String>) -> Option<String> {
    match (current.as_deref(), candidate.as_deref()) {
        (Some("explicit"), _) | (_, Some("explicit")) => Some("explicit".to_string()),
        (Some("heuristic"), _) | (_, Some("heuristic")) => Some("heuristic".to_string()),
        (Some("unknown"), _) | (_, Some("unknown")) => Some("unknown".to_string()),
        _ => None,
    }
}

fn wall_clock_span_secs(records: &[SessionStatsRecord]) -> u64 {
    let Some(first) = records.first() else {
        return 0;
    };
    let mut start = super::list::session_created_at(&first.session);
    let mut end = first.end_at;

    for record in &records[1..] {
        start = start.min(super::list::session_created_at(&record.session));
        end = end.max(record.end_at);
    }

    nonnegative_secs(end - start)
}

fn rollup_tokens<'a>(sessions: impl Iterator<Item = &'a MetaSessionState>) -> TokenRollup {
    let mut rollup = TokenRollup::default();
    for session in sessions {
        let Some(usage) = session.total_token_usage.as_ref() else {
            continue;
        };
        let input = usage.input_tokens.unwrap_or(0);
        let cached = usage.cache_read_input_tokens.unwrap_or(0).min(input);
        let uncached = input.saturating_sub(cached);
        let output = usage.output_tokens.unwrap_or(0);
        let total = usage
            .total_tokens
            .unwrap_or_else(|| input.saturating_add(output));

        rollup.uncached_input_tokens = rollup.uncached_input_tokens.saturating_add(uncached);
        rollup.cached_input_tokens = rollup.cached_input_tokens.saturating_add(cached);
        rollup.output_tokens = rollup.output_tokens.saturating_add(output);
        rollup.total_tokens = rollup.total_tokens.saturating_add(total);
        rollup.sessions_with_tokens += 1;
    }
    rollup
}

fn rollup_cost<'a>(sessions: impl Iterator<Item = &'a MetaSessionState>) -> CostRollup {
    let mut total = 0.0;
    let mut sessions_with_recorded_cost = 0;

    for session in sessions {
        let Some(cost) = session
            .total_token_usage
            .as_ref()
            .and_then(|usage| usage.estimated_cost_usd)
        else {
            continue;
        };
        if cost > 0.0 {
            total += cost;
            sessions_with_recorded_cost += 1;
        }
    }

    if sessions_with_recorded_cost == 0 {
        CostRollup {
            estimated_usd: None,
            source: CostSource::Unknown,
            sessions_with_recorded_cost,
        }
    } else {
        CostRollup {
            estimated_usd: Some(total),
            source: CostSource::RecordedSessionEstimates,
            sessions_with_recorded_cost,
        }
    }
}

fn issue_key_for_session(session: &MetaSessionState) -> (String, IssueSource) {
    for text in [
        session.spec_id.as_deref(),
        session.description.as_deref(),
        session.task_context.task_type.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(issue) = extract_issue_with_marker(text) {
            return (issue, IssueSource::Explicit);
        }
    }

    if let Some(branch) = session.branch.as_deref()
        && let Some(issue) = extract_issue_heuristic(branch)
    {
        return (issue, IssueSource::Heuristic);
    }

    (UNKNOWN_GROUP.to_string(), IssueSource::Unknown)
}

fn extract_issue_with_marker(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    for marker in ["issue #", "issue-", "issue/", "gh #", "github #"] {
        if let Some(pos) = lower.find(marker) {
            let raw = &text[pos + marker.len()..];
            if let Some(issue) = leading_issue_number(raw) {
                return Some(format!("#{issue}"));
            }
        }
    }
    None
}

fn extract_issue_heuristic(text: &str) -> Option<String> {
    for part in text.split(|c: char| !c.is_ascii_alphanumeric() && c != '#') {
        let candidate = part.strip_prefix('#').unwrap_or(part);
        if (3..=7).contains(&candidate.len()) && candidate.chars().all(|c| c.is_ascii_digit()) {
            return Some(format!("#{candidate}"));
        }
    }
    None
}

fn leading_issue_number(raw: &str) -> Option<String> {
    let digits: String = raw
        .chars()
        .skip_while(|c| c.is_whitespace())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    (!digits.is_empty()).then_some(digits)
}

fn tool_key_for_session(session: &MetaSessionState, result: Option<&SessionResult>) -> String {
    if let Some(tool) = result
        .map(|result| result.tool.as_str())
        .filter(|tool| !tool.trim().is_empty())
    {
        return tool.to_string();
    }

    match session.tools.len() {
        0 => UNKNOWN_GROUP.to_string(),
        1 => session
            .tools
            .keys()
            .next()
            .cloned()
            .unwrap_or_else(|| UNKNOWN_GROUP.to_string()),
        _ => "multiple".to_string(),
    }
}

fn nonnegative_secs(duration: Duration) -> u64 {
    duration.num_seconds().max(0) as u64
}

#[cfg(test)]
#[path = "session_cmds_observe_tests.rs"]
mod tests;
