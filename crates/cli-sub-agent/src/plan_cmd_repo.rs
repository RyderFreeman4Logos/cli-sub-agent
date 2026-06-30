use std::path::Path;
use std::process::Command;

pub(crate) fn detect_effective_repo(project_root: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["config", "--get", "remote.origin.url"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if raw.is_empty() {
        return None;
    }
    let sanitized = if let Some(pos) = raw.find("://") {
        let (scheme, rest) = raw.split_at(pos + 3);
        rest.find('@')
            .map(|at_pos| format!("{}{}", scheme, &rest[at_pos + 1..]))
            .unwrap_or(raw)
    } else {
        raw
    };

    let trimmed = sanitized.trim_end_matches(".git");
    for prefix in [
        "git@github.com:",
        "https://github.com/",
        "ssh://git@github.com/",
    ] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return Some(rest.to_string());
        }
    }
    Some(trimmed.to_string())
}
