use std::path::Path;

use csa_session::{SessionPhase, SessionResult};

const MAX_SESSION_CANDIDATES: usize = 8;
const MAX_SUMMARY_CHARS: usize = 180;

pub(crate) fn format_plan_child_died_diagnostics(
    project_root: &Path,
    stdout: &str,
    stderr: &str,
) -> Option<String> {
    let mut diagnostics = Vec::new();
    let combined = format!("{stdout}\n{stderr}");
    for candidate in session_id_candidates(&combined) {
        if diagnostics.len() >= MAX_SESSION_CANDIDATES {
            break;
        }
        let Some(diagnostic) = inspect_child_session(project_root, &candidate) else {
            continue;
        };
        if diagnostic.is_failure_like() {
            diagnostics.push(diagnostic.render());
        }
    }

    (!diagnostics.is_empty()).then(|| diagnostics.join("\n"))
}

pub(crate) fn append_child_diagnostics(
    message: &mut String,
    project_root: &Path,
    stdout: &str,
    stderr: &str,
) {
    let Some(child_diagnostics) = format_plan_child_died_diagnostics(project_root, stdout, stderr)
    else {
        return;
    };
    message.push('\n');
    message.push_str(&child_diagnostics);
}

fn inspect_child_session(project_root: &Path, candidate: &str) -> Option<PlanChildDiagnostic> {
    let resolved =
        crate::session_cmds::resolve_session_prefix_with_global_fallback(project_root, candidate)
            .ok()?;
    let session_dir = resolved.sessions_dir.join(&resolved.session_id);
    let effective_root = resolved
        .foreign_project_root
        .as_deref()
        .unwrap_or(project_root);
    let session = csa_session::load_session(effective_root, &resolved.session_id)
        .ok()
        .or_else(|| load_session_state_from_dir(&session_dir));
    let result = csa_session::load_result(effective_root, &resolved.session_id)
        .ok()
        .flatten()
        .or_else(|| load_result_from_dir(&session_dir));
    let live_process = csa_process::ToolLiveness::has_live_process(&session_dir)
        || csa_process::ToolLiveness::daemon_pid_is_alive(&session_dir);
    let phase = session
        .as_ref()
        .map(|session| phase_name(&session.phase))
        .unwrap_or("unknown");
    let status = derive_child_status(
        session.as_ref().map(|s| &s.phase),
        result.as_ref(),
        live_process,
    );

    Some(PlanChildDiagnostic {
        session_id: resolved.session_id,
        status,
        phase,
        live_process,
        result_status: result.as_ref().map(|result| result.status.clone()),
        result_exit_code: result.as_ref().map(|result| result.exit_code),
        summary: result
            .as_ref()
            .map(|result| compact_summary(&result.summary))
            .filter(|summary| !summary.is_empty()),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlanChildDiagnostic {
    session_id: String,
    status: &'static str,
    phase: &'static str,
    live_process: bool,
    result_status: Option<String>,
    result_exit_code: Option<i32>,
    summary: Option<String>,
}

impl PlanChildDiagnostic {
    fn is_failure_like(&self) -> bool {
        matches!(self.status, "NoLivePID" | "Failed" | "Error")
    }

    fn render(&self) -> String {
        let result_status = self.result_status.as_deref().unwrap_or("missing");
        let result_exit = self
            .result_exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "missing".to_string());
        let mut rendered = format!(
            "plan_child_died session={} status={} phase={} live_process={} result_status={} result_exit={}",
            self.session_id, self.status, self.phase, self.live_process, result_status, result_exit
        );
        if let Some(summary) = &self.summary {
            rendered.push_str(" summary=\"");
            rendered.push_str(summary);
            rendered.push('"');
        }
        rendered
    }
}

fn derive_child_status(
    phase: Option<&SessionPhase>,
    result: Option<&SessionResult>,
    live_process: bool,
) -> &'static str {
    if let Some(result) = result {
        let normalized = result.status.trim().to_ascii_lowercase();
        if matches!(normalized.as_str(), "success" | "retired") && result.exit_code == 0 {
            return "Succeeded";
        }
        if normalized == "error" {
            return "Error";
        }
        return "Failed";
    }

    if matches!(phase, Some(SessionPhase::Active)) && !live_process {
        return "NoLivePID";
    }

    match phase {
        Some(SessionPhase::Active) => "Active",
        Some(SessionPhase::Retired) => "Retired",
        Some(SessionPhase::Available) => "Available",
        Some(SessionPhase::ToolExhausted) => "ToolExhausted",
        None => "Error",
    }
}

fn phase_name(phase: &SessionPhase) -> &'static str {
    match phase {
        SessionPhase::Active => "Active",
        SessionPhase::Available => "Available",
        SessionPhase::Retired => "Retired",
        SessionPhase::ToolExhausted => "ToolExhausted",
    }
}

fn load_session_state_from_dir(session_dir: &Path) -> Option<csa_session::MetaSessionState> {
    let raw = std::fs::read_to_string(session_dir.join("state.toml")).ok()?;
    toml::from_str(&raw).ok()
}

fn load_result_from_dir(session_dir: &Path) -> Option<SessionResult> {
    let raw =
        std::fs::read_to_string(session_dir.join(csa_session::result::RESULT_FILE_NAME)).ok()?;
    toml::from_str(&raw).ok()
}

fn session_id_candidates(text: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    for token in text.split(|ch: char| !ch.is_ascii_alphanumeric()) {
        let token = token.trim();
        if !looks_like_session_prefix(token) || candidates.iter().any(|seen| seen == token) {
            continue;
        }
        candidates.push(token.to_string());
    }
    candidates
}

fn looks_like_session_prefix(token: &str) -> bool {
    (10..=26).contains(&token.len())
        && token
            .chars()
            .all(|ch| matches!(ch, '0'..='9' | 'A'..='H' | 'J'..='K' | 'M'..='N' | 'P'..='T' | 'V'..='Z'))
        && token.starts_with("01")
}

fn compact_summary(summary: &str) -> String {
    let redacted = csa_session::redact_text_content(summary);
    let compacted = redacted.split_whitespace().collect::<Vec<_>>().join(" ");
    let escaped = compacted.replace('"', "'");
    let mut chars = escaped.chars();
    let truncated: String = chars.by_ref().take(MAX_SUMMARY_CHARS).collect();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_session_sandbox::ScopedSessionSandbox;
    use chrono::Utc;
    use csa_session::{create_session, get_session_dir, load_session, save_result, save_session};
    use tempfile::tempdir;

    fn make_result(status: &str, exit_code: i32, summary: &str) -> SessionResult {
        let now = Utc::now();
        SessionResult {
            post_exec_gate: None,
            status: status.to_string(),
            exit_code,
            summary: summary.to_string(),
            tool: "codex".to_string(),
            original_tool: None,
            fallback_tool: None,
            fallback_reason: None,
            started_at: now,
            completed_at: now,
            events_count: 0,
            artifacts: Vec::new(),
            ..Default::default()
        }
    }

    #[test]
    fn no_live_active_session_renders_plan_child_died() {
        let td = tempdir().expect("tempdir");
        let _sandbox = ScopedSessionSandbox::new_blocking(&td);
        let project = td.path();
        let session = create_session(project, Some("child"), None, Some("codex")).unwrap();
        let session_id = session.meta_session_id.clone();

        let rendered = format_plan_child_died_diagnostics(
            project,
            "",
            &format!(
                "Session {session_id} has no live daemon process and no terminal result packet."
            ),
        )
        .expect("diagnostic should render");

        assert!(rendered.contains("plan_child_died"));
        assert!(rendered.contains(&format!("session={session_id}")));
        assert!(rendered.contains("status=NoLivePID"));
        assert!(rendered.contains("result_status=missing"));
    }

    #[test]
    fn failed_child_result_renders_result_summary() {
        let td = tempdir().expect("tempdir");
        let _sandbox = ScopedSessionSandbox::new_blocking(&td);
        let project = td.path();
        let mut session = create_session(project, Some("child"), None, Some("codex")).unwrap();
        session
            .apply_phase_event(csa_session::PhaseEvent::Retired)
            .unwrap();
        let session_id = session.meta_session_id.clone();
        let session_dir = get_session_dir(project, &session_id).unwrap();
        save_session(&session).unwrap();
        save_result(
            project,
            &session_id,
            &make_result("failure", 1, "child failed before writing expected output"),
        )
        .unwrap();
        assert!(load_session(project, &session_id).unwrap().phase == SessionPhase::Retired);

        let rendered =
            format_plan_child_died_diagnostics(project, &format!("waiting on {session_id}"), "")
                .expect("diagnostic should render");

        assert!(session_dir.is_dir());
        assert!(rendered.contains("status=Failed"));
        assert!(rendered.contains("result_status=failure"));
        assert!(rendered.contains("summary=\"child failed before writing expected output\""));
    }

    #[test]
    fn successful_child_is_not_reported_as_died() {
        let td = tempdir().expect("tempdir");
        let _sandbox = ScopedSessionSandbox::new_blocking(&td);
        let project = td.path();
        let mut session = create_session(project, Some("child"), None, Some("codex")).unwrap();
        session
            .apply_phase_event(csa_session::PhaseEvent::Retired)
            .unwrap();
        let session_id = session.meta_session_id.clone();
        save_session(&session).unwrap();
        save_result(project, &session_id, &make_result("success", 0, "ok")).unwrap();

        assert!(
            format_plan_child_died_diagnostics(project, &session_id, "").is_none(),
            "successful child should not produce a death diagnostic"
        );
    }
}
