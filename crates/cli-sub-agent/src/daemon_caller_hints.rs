use std::path::Path;

pub(crate) fn format_session_wait_command(session_id: &str, project_root: &Path) -> String {
    format!(
        "csa session wait --session {}{}",
        session_id,
        format_cd_arg(project_root)
    )
}

pub(crate) fn format_session_attach_command(session_id: &str, project_root: &Path) -> String {
    format!(
        "csa session attach --session {}{}",
        session_id,
        format_cd_arg(project_root)
    )
}

fn format_cd_arg(project_root: &Path) -> String {
    format!(" --cd '{}'", project_root.display())
}
