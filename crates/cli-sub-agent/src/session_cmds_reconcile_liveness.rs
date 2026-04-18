use std::fs;
use std::path::Path;

use csa_process::ToolLiveness;

const LIVENESS_SNAPSHOT_FILE: &str = ".liveness.snapshot";
const ACP_EVENTS_LOG_FILE: &str = "output/acp-events.jsonl";
const STDERR_LOG_FILE: &str = "stderr.log";

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
    let mut snapshot = load_reconcile_liveness_snapshot(session_dir);
    let spool_growth = matches!(
        (
            snapshot.spool_bytes_written,
            snapshot.observed_spool_bytes_written
        ),
        (Some(current), Some(previous)) if current != previous
    );
    snapshot.observed_spool_bytes_written = snapshot.spool_bytes_written;

    let (acp_growth, acp_size) = detect_file_growth(
        &session_dir.join(ACP_EVENTS_LOG_FILE),
        snapshot.acp_events_size,
    );
    snapshot.acp_events_size = acp_size;

    let (stderr_growth, stderr_size) =
        detect_file_growth(&session_dir.join(STDERR_LOG_FILE), snapshot.stderr_log_size);
    snapshot.stderr_log_size = stderr_size;

    save_reconcile_liveness_snapshot(session_dir, &snapshot);
    spool_growth || acp_growth || stderr_growth
}

fn detect_file_growth(path: &Path, previous_size: Option<u64>) -> (bool, Option<u64>) {
    let current_size = fs::metadata(path).ok().map(|meta| meta.len());
    let growth = match (previous_size, current_size) {
        (Some(prev), Some(current)) => current != prev,
        _ => false,
    };
    (growth, current_size)
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
