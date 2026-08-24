use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub(super) fn real_git_binary(guard_dir: &Path) -> Option<PathBuf> {
    // The guard's child environment is attacker-controlled. Resolve a known
    // host Git rather than inheriting an ambient CSA_REAL_GIT override; this
    // is the trust anchor used to pin the projected transfer's libexec path.
    let guard_dir_str = guard_dir.to_string_lossy();
    let is_not_guard = |path: &Path| !path.to_string_lossy().starts_with(guard_dir_str.as_ref());

    ["/usr/bin/git", "/usr/local/bin/git", "/bin/git"]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_file() && is_not_guard(path))
        .or_else(|| which::which("git").ok().filter(|path| is_not_guard(path)))
}

pub(super) fn prepend_guard_dir(env: &mut HashMap<String, String>, guard_dir: &Path) {
    let guard_dir_str = guard_dir.to_string_lossy().into_owned();
    let current_path = env
        .get("PATH")
        .cloned()
        .or_else(|| std::env::var("PATH").ok())
        .unwrap_or_default();
    let filtered = current_path
        .split(':')
        .filter(|entry| !entry.is_empty() && *entry != guard_dir_str)
        .collect::<Vec<_>>()
        .join(":");
    let new_path = if filtered.is_empty() {
        guard_dir_str
    } else {
        format!("{guard_dir_str}:{filtered}")
    };
    env.insert("PATH".to_string(), new_path);
}
