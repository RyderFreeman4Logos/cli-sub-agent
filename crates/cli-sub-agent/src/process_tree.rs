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
/// or `None` on any failure (unsupported platform, permission denied, no match).
pub(crate) fn detect_ancestor_tool() -> Option<String> {
    let mut current_pid = read_ppid(std::process::id())?;

    for depth in 0..MAX_ANCESTOR_DEPTH {
        if current_pid <= 1 {
            return None;
        }

        let comm = read_comm(current_pid)?;

        if let Some(tool_name) = match_tool_by_comm(&comm) {
            tracing::debug!(
                tool = tool_name,
                ancestor_pid = current_pid,
                depth,
                "Detected calling tool from process tree"
            );
            return Some(tool_name.to_string());
        }

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
#[cfg(target_os = "macos")]
fn read_ppid(pid: u32) -> Option<u32> {
    let output = std::process::Command::new("ps")
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
/// On macOS, `ps -o comm=` returns the full path (e.g., `/usr/local/bin/claude`),
/// so we extract the basename.
#[cfg(target_os = "macos")]
fn read_comm(pid: u32) -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-o", "comm=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let full_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if full_path.is_empty() {
        return None;
    }
    // Extract basename: "/usr/local/bin/claude" → "claude"
    let basename = full_path
        .rsplit('/')
        .next()
        .unwrap_or(&full_path)
        .to_string();
    Some(basename)
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

/// Match a comm field against known tool executables.
fn match_tool_by_comm(comm: &str) -> Option<&'static str> {
    KNOWN_TOOL_EXECUTABLES
        .iter()
        .find(|(exe, _)| *exe == comm)
        .map(|(_, name)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;

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
