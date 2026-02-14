//! Detect ancestor tool processes by walking the process tree.
//!
//! When CSA is invoked directly from a tool (e.g., `claude` running
//! `csa review --diff`), the `CSA_TOOL` environment variable is not set.
//! This module provides a fallback by walking the process tree to find an
//! ancestor process whose executable matches a known tool.
//!
//! Platform support:
//! - Linux: reads `/proc/<pid>/stat` and `/proc/<pid>/comm`
//! - macOS: uses `ps` command to query process info
//! - Other: returns `None` (graceful fallback)

/// Maximum number of ancestor levels to walk before giving up.
const MAX_ANCESTOR_DEPTH: usize = 16;

/// Mapping from process comm basenames to CSA tool names.
///
/// Must stay in sync with `Executor::executable_name()` in csa-executor
/// and `is_tool_binary_available()` in run_helpers.rs.
const KNOWN_TOOL_EXECUTABLES: &[(&str, &str)] = &[
    // ACP mode uses the same executable basenames with ACP-specific args
    // (e.g. `claude --acp`, `codex acp`), so no separate ACP process names
    // are required here.
    ("claude", "claude-code"),
    ("gemini", "gemini-cli"),
    ("codex", "codex"),
    ("opencode", "opencode"),
];

/// Detect the calling tool by walking the process tree.
///
/// Starts from the current process's parent and walks upward, checking
/// each ancestor's comm (executable basename) against known tools.
///
/// Returns the tool name string (e.g., `"claude-code"`) if found,
/// or `None` if no known tool is found, the platform is unsupported, or the
/// parent chain cannot be read. Individual ancestors that cannot be read
/// (e.g., permission denied on comm) are skipped rather than aborting the walk.
pub(crate) fn detect_ancestor_tool() -> Option<String> {
    let mut current_pid = read_ppid(std::process::id())?;

    for depth in 0..MAX_ANCESTOR_DEPTH {
        if current_pid <= 1 {
            return None;
        }

        // Best-effort: if we can read the comm, check it; if not, skip this
        // ancestor and try the next one (don't abort the entire walk).
        if let Some(comm) = read_comm(current_pid) {
            if let Some(tool_name) = match_tool_by_comm(&comm) {
                tracing::debug!(
                    tool = tool_name,
                    ancestor_pid = current_pid,
                    depth,
                    "Detected calling tool from process tree"
                );
                return Some(tool_name.to_string());
            }
        }

        // If we can't read the parent PID, we truly can't continue.
        current_pid = read_ppid(current_pid)?;
    }

    None
}

// ---------------------------------------------------------------------------
// Platform: Linux — read /proc
// ---------------------------------------------------------------------------

/// Read the parent PID from `/proc/<pid>/stat`.
///
/// The stat file format is: `pid (comm) state ppid ...`
/// The comm field can contain spaces and parentheses, so we find the
/// last `)` to safely skip it.
#[cfg(target_os = "linux")]
fn read_ppid(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let idx = stat.rfind(')')?;
    let after_comm = stat.get(idx + 2..)?; // skip ") "
    // Fields after comm: state ppid ...
    after_comm.split_whitespace().nth(1)?.parse().ok()
}

/// Read the command name from `/proc/<pid>/comm`.
///
/// Returns the basename of the executable, truncated to 15 chars by the
/// kernel. All known tool names are <= 8 chars, so truncation is not
/// a concern.
#[cfg(target_os = "linux")]
fn read_comm(pid: u32) -> Option<String> {
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    Some(comm.trim().to_string())
}

// ---------------------------------------------------------------------------
// Platform: macOS — use `ps` command (no unsafe, no extra deps)
// ---------------------------------------------------------------------------

/// Read the parent PID via `ps -o ppid= -p <pid>`.
///
/// Uses absolute path to avoid PATH injection.
#[cfg(target_os = "macos")]
fn read_ppid(pid: u32) -> Option<u32> {
    let output = std::process::Command::new("/bin/ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

/// Read the command name via `ps -o comm= -p <pid>`.
///
/// On macOS, `ps -o comm=` may return either a full path (e.g.,
/// `/usr/local/bin/claude`) or just the basename depending on how the
/// process was launched. We always extract the basename to normalize.
///
/// Uses absolute path to avoid PATH injection.
#[cfg(target_os = "macos")]
fn read_comm(pid: u32) -> Option<String> {
    let output = std::process::Command::new("/bin/ps")
        .args(["-o", "comm=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if raw.is_empty() {
        return None;
    }
    Some(normalize_basename(&raw).to_string())
}

// ---------------------------------------------------------------------------
// Platform: other — graceful fallback
// ---------------------------------------------------------------------------

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn read_ppid(_pid: u32) -> Option<u32> {
    None
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn read_comm(_pid: u32) -> Option<String> {
    None
}

/// Normalize a process name to its basename.
///
/// Handles both bare names (`claude`) and full paths (`/usr/local/bin/claude`).
/// Trims whitespace and extracts the last path component.
fn normalize_basename(raw: &str) -> &str {
    let trimmed = raw.trim().trim_end_matches('/');
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}

/// Match a comm field against known tool executables.
///
/// Normalizes to basename before matching, so both bare names and full
/// paths work correctly.
fn match_tool_by_comm(comm: &str) -> Option<&'static str> {
    let basename = normalize_basename(comm);
    KNOWN_TOOL_EXECUTABLES
        .iter()
        .find(|(exe, _)| *exe == basename)
        .map(|(_, name)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_basename_bare_name() {
        assert_eq!(normalize_basename("claude"), "claude");
    }

    #[test]
    fn test_normalize_basename_full_path() {
        assert_eq!(normalize_basename("/usr/local/bin/claude"), "claude");
    }

    #[test]
    fn test_normalize_basename_with_whitespace() {
        assert_eq!(normalize_basename("  /bin/ps  "), "ps");
    }

    #[test]
    fn test_normalize_basename_root_binary() {
        assert_eq!(normalize_basename("/bin/sh"), "sh");
    }

    #[test]
    fn test_normalize_basename_trailing_slash() {
        // Trailing slashes are stripped before extracting basename
        assert_eq!(normalize_basename("/usr/bin/"), "bin");
    }

    #[test]
    fn test_match_tool_by_comm_claude() {
        assert_eq!(match_tool_by_comm("claude"), Some("claude-code"));
    }

    #[test]
    fn test_match_tool_by_comm_gemini() {
        assert_eq!(match_tool_by_comm("gemini"), Some("gemini-cli"));
    }

    #[test]
    fn test_match_tool_by_comm_codex() {
        assert_eq!(match_tool_by_comm("codex"), Some("codex"));
    }

    #[test]
    fn test_match_tool_by_comm_opencode() {
        assert_eq!(match_tool_by_comm("opencode"), Some("opencode"));
    }

    #[test]
    fn test_match_tool_by_comm_unknown() {
        assert_eq!(match_tool_by_comm("bash"), None);
        assert_eq!(match_tool_by_comm("zsh"), None);
        assert_eq!(match_tool_by_comm("python"), None);
        assert_eq!(match_tool_by_comm(""), None);
    }

    #[test]
    fn test_match_tool_by_comm_full_path() {
        // macOS ps may return full paths; match_tool_by_comm normalizes
        assert_eq!(
            match_tool_by_comm("/usr/local/bin/claude"),
            Some("claude-code")
        );
        assert_eq!(
            match_tool_by_comm("/opt/homebrew/bin/gemini"),
            Some("gemini-cli")
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_read_ppid_self_linux() {
        let ppid = read_ppid(std::process::id());
        assert!(ppid.is_some(), "read_ppid(self) should succeed on Linux");
        assert!(ppid.unwrap() > 0);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn test_read_ppid_self_macos() {
        let ppid = read_ppid(std::process::id());
        assert!(ppid.is_some(), "read_ppid(self) should succeed on macOS");
        assert!(ppid.unwrap() > 0);
    }

    #[test]
    fn test_read_ppid_invalid_pid() {
        let ppid = read_ppid(999_999_999);
        assert!(ppid.is_none());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_read_comm_self_linux() {
        let comm = read_comm(std::process::id());
        assert!(comm.is_some(), "read_comm(self) should succeed on Linux");
        assert!(!comm.unwrap().is_empty());
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn test_read_comm_self_macos() {
        let comm = read_comm(std::process::id());
        assert!(comm.is_some(), "read_comm(self) should succeed on macOS");
        let comm_str = comm.unwrap();
        assert!(!comm_str.is_empty());
        // On macOS, comm should be a basename (no slashes)
        assert!(
            !comm_str.contains('/'),
            "comm should be basename, got: {comm_str}"
        );
    }

    #[test]
    fn test_read_comm_invalid_pid() {
        let comm = read_comm(999_999_999);
        assert!(comm.is_none());
    }

    #[test]
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn test_detect_ancestor_tool_does_not_panic() {
        // Just verify it doesn't panic. Result depends on runtime context.
        let _result = detect_ancestor_tool();
    }
}
