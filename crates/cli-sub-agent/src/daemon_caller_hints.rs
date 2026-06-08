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

pub(crate) fn format_cd_arg(project_root: &Path) -> String {
    let project_root = project_root.to_string_lossy();
    format!(" --cd {}", shell_escape_for_command(&project_root))
}

pub(crate) fn escape_structured_comment_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn shell_escape_for_command(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
