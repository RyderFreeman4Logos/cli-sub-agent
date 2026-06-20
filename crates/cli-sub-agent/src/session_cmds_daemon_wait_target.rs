use std::path::{Path, PathBuf};

use anyhow::Result;

#[derive(Debug, Clone)]
pub(super) struct WaitTarget {
    pub(super) session_id: String,
    pub(super) session_dir: PathBuf,
    pub(super) follows_resume_target: bool,
}

pub(super) fn resolve_wait_target(
    project_root: &Path,
    wrapper_session_id: &str,
    wrapper_session_dir: &Path,
) -> Result<WaitTarget> {
    let resume_target =
        csa_session::resolve_resume_target_from_dir(project_root, wrapper_session_dir)?;
    Ok(resume_target.map_or_else(
        || WaitTarget {
            session_id: wrapper_session_id.to_string(),
            session_dir: wrapper_session_dir.to_path_buf(),
            follows_resume_target: false,
        },
        |target| WaitTarget {
            session_id: target.session_id,
            session_dir: target.session_dir,
            follows_resume_target: true,
        },
    ))
}
