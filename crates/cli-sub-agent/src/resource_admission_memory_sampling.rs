use std::{io, path::Path};

use csa_session::{MetaSessionState, SessionTreeMemorySampler};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SessionMemorySample {
    RssMb(u64),
    UnsupportedLiveProcess,
    UnavailableLiveProcess,
    Terminal,
    Unavailable,
}

pub(super) fn sample_session_memory(session: &MetaSessionState) -> SessionMemorySample {
    let project_root = Path::new(&session.project_path);
    // Terminal only after its writer/daemon exits; live results remain charged.
    if csa_session::load_result(project_root, &session.meta_session_id)
        .ok()
        .flatten()
        .is_some()
        && !session_has_live_process_signal(project_root, &session.meta_session_id)
    {
        return SessionMemorySample::Terminal;
    }

    let sample = SessionTreeMemorySampler::new(project_root, &session.meta_session_id)
        .and_then(|sampler| sampler.sample_rss_mb());
    let live_process_signal =
        sample.is_err() && session_has_live_process_signal(project_root, &session.meta_session_id);
    classify_session_memory_sample(sample, live_process_signal)
}

pub(super) fn classify_session_memory_sample(
    sample: io::Result<u64>,
    live_process_signal: bool,
) -> SessionMemorySample {
    match sample {
        Ok(rss_mb) => SessionMemorySample::RssMb(rss_mb),
        Err(err) if live_process_signal && err.kind() == io::ErrorKind::Unsupported => {
            SessionMemorySample::UnsupportedLiveProcess
        }
        Err(_) if live_process_signal => SessionMemorySample::UnavailableLiveProcess,
        Err(_) => SessionMemorySample::Unavailable,
    }
}

fn session_has_live_process_signal(project_root: &Path, session_id: &str) -> bool {
    let Ok(session_dir) = csa_session::get_session_dir(project_root, session_id) else {
        return false;
    };

    csa_process::ToolLiveness::has_live_process(&session_dir)
        || csa_process::ToolLiveness::daemon_pid_is_alive(&session_dir)
}
