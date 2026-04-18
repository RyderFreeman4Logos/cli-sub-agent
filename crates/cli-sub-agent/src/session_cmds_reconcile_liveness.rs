use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use csa_process::ToolLiveness;

const LIVENESS_SNAPSHOT_FILE: &str = ".liveness.snapshot";
const OUTPUT_LOG_FILE: &str = "output.log";
const ACP_EVENTS_LOG_FILE: &str = "output/acp-events.jsonl";
const STDERR_LOG_FILE: &str = "stderr.log";
// Keep this aligned with csa_process::tool_liveness::LIVENESS_RECENT_WINDOW_SECS.
const RECENT_SESSION_WRITE_WINDOW_SECS: u64 = 30;

pub(crate) struct ReconcileLivenessDecision {
    pub(crate) blocks_synthesis: bool,
    pub(crate) reason: &'static str,
}

#[derive(Default)]
struct ReconcileLivenessSnapshot {
    spool_bytes_written: Option<u64>,
    observed_spool_bytes_written: Option<u64>,
    acp_events_size: Option<u64>,
    stderr_log_size: Option<u64>,
}

struct ReconcileDecisionInputs {
    snapshot: Option<ReconcileObservedSnapshot>,
    spool_bytes_written: Option<u64>,
    acp_events_size: Option<u64>,
    stderr_log_size: Option<u64>,
    output_log_mtime_within_grace: bool,
    stderr_log_mtime_within_grace: bool,
}

struct ReconcileObservedSnapshot {
    observed_spool_bytes_written: Option<u64>,
    observed_acp_events_size: Option<u64>,
    observed_stderr_log_size: Option<u64>,
}

pub(crate) fn reconcile_liveness_decision(session_dir: &Path) -> ReconcileLivenessDecision {
    if ToolLiveness::has_live_process(session_dir) {
        return ReconcileLivenessDecision {
            blocks_synthesis: true,
            reason: "live_pid",
        };
    }
    if ToolLiveness::daemon_pid_is_alive(session_dir) {
        return ReconcileLivenessDecision {
            blocks_synthesis: true,
            reason: "live_daemon_pid",
        };
    }
    if has_reconcile_progress_signal(session_dir) {
        return ReconcileLivenessDecision {
            blocks_synthesis: true,
            reason: "progress_signal",
        };
    }
    ReconcileLivenessDecision {
        blocks_synthesis: false,
        reason: "no_pid_no_progress",
    }
}

fn has_reconcile_progress_signal(session_dir: &Path) -> bool {
    let now = SystemTime::now();
    let mut snapshot = load_reconcile_liveness_snapshot(session_dir);
    let acp_size = fs::metadata(session_dir.join(ACP_EVENTS_LOG_FILE))
        .ok()
        .map(|meta| meta.len());
    let stderr_size = fs::metadata(session_dir.join(STDERR_LOG_FILE))
        .ok()
        .map(|meta| meta.len());
    let decision_inputs = ReconcileDecisionInputs {
        snapshot: snapshot.prior_observation(),
        spool_bytes_written: snapshot.spool_bytes_written,
        acp_events_size: acp_size,
        stderr_log_size: stderr_size,
        output_log_mtime_within_grace: file_modified_recently(
            &session_dir.join(OUTPUT_LOG_FILE),
            now,
        ),
        stderr_log_mtime_within_grace: file_modified_recently(
            &session_dir.join(STDERR_LOG_FILE),
            now,
        ),
    };

    snapshot.observed_spool_bytes_written = snapshot.spool_bytes_written;
    snapshot.acp_events_size = acp_size;
    snapshot.stderr_log_size = stderr_size;

    save_reconcile_liveness_snapshot(session_dir, &snapshot);

    has_reconcile_progress_signal_from_inputs(&decision_inputs)
}

fn has_reconcile_progress_signal_from_inputs(decision_inputs: &ReconcileDecisionInputs) -> bool {
    if let Some(snapshot) = &decision_inputs.snapshot {
        if decision_inputs.spool_bytes_written != snapshot.observed_spool_bytes_written {
            return true;
        }
        if decision_inputs.acp_events_size != snapshot.observed_acp_events_size {
            return true;
        }
        if decision_inputs.stderr_log_size != snapshot.observed_stderr_log_size {
            return true;
        }
    }

    if decision_inputs.spool_bytes_written.unwrap_or(0) > 0
        && decision_inputs.output_log_mtime_within_grace
    {
        return true;
    }
    if decision_inputs.stderr_log_size.unwrap_or(0) > 0
        && decision_inputs.stderr_log_mtime_within_grace
    {
        return true;
    }

    false
}

fn file_modified_recently(path: &Path, now: SystemTime) -> bool {
    let modified = match fs::metadata(path).and_then(|meta| meta.modified()) {
        Ok(modified) => modified,
        Err(_) => return false,
    };
    let elapsed = now.duration_since(modified).unwrap_or(Duration::ZERO);
    elapsed <= Duration::from_secs(RECENT_SESSION_WRITE_WINDOW_SECS)
}

fn load_reconcile_liveness_snapshot(session_dir: &Path) -> ReconcileLivenessSnapshot {
    let snapshot_path = session_dir.join(LIVENESS_SNAPSHOT_FILE);
    let Ok(content) = fs::read_to_string(snapshot_path) else {
        return ReconcileLivenessSnapshot::default();
    };

    let mut snapshot = ReconcileLivenessSnapshot::default();
    for line in content.lines() {
        let mut parts = line.splitn(2, '=');
        let key = parts.next().unwrap_or_default().trim();
        let value = parts.next().unwrap_or_default().trim().parse::<u64>().ok();
        match key {
            "spool_bytes_written" => snapshot.spool_bytes_written = value,
            "observed_spool_bytes_written" => snapshot.observed_spool_bytes_written = value,
            "acp_events_size" => snapshot.acp_events_size = value,
            "stderr_log_size" => snapshot.stderr_log_size = value,
            _ => {}
        }
    }
    snapshot
}

impl ReconcileLivenessSnapshot {
    fn prior_observation(&self) -> Option<ReconcileObservedSnapshot> {
        let has_prior_observation = self.observed_spool_bytes_written.is_some()
            || self.acp_events_size.is_some()
            || self.stderr_log_size.is_some();
        has_prior_observation.then_some(ReconcileObservedSnapshot {
            observed_spool_bytes_written: self.observed_spool_bytes_written,
            observed_acp_events_size: self.acp_events_size,
            observed_stderr_log_size: self.stderr_log_size,
        })
    }
}

fn save_reconcile_liveness_snapshot(session_dir: &Path, snapshot: &ReconcileLivenessSnapshot) {
    let mut lines = Vec::with_capacity(4);
    if let Some(value) = snapshot.spool_bytes_written {
        lines.push(format!("spool_bytes_written={value}"));
    }
    if let Some(value) = snapshot.observed_spool_bytes_written {
        lines.push(format!("observed_spool_bytes_written={value}"));
    }
    if let Some(value) = snapshot.acp_events_size {
        lines.push(format!("acp_events_size={value}"));
    }
    if let Some(value) = snapshot.stderr_log_size {
        lines.push(format!("stderr_log_size={value}"));
    }
    if lines.is_empty() {
        return;
    }
    let _ = fs::write(session_dir.join(LIVENESS_SNAPSHOT_FILE), lines.join("\n"));
}
