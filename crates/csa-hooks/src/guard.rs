//! Prompt Guard: user-configurable shell scripts that inject text into prompts.
//!
//! Guards run before tool execution and their stdout is injected into the
//! `effective_prompt`. This enables "reverse prompt injection" — reminding
//! tools (including those without native hook systems) to follow AGENTS.md
//! rules like branch protection, timely commits, etc.
//!
//! ## Protocol
//!
//! Each guard script receives a JSON [`GuardContext`] on stdin and writes
//! injection text to stdout. Empty stdout means "nothing to inject".
//! Non-zero exit or timeout results in a warning and skip (never blocks).
//!
//! ## Configuration
//!
//! ```toml
//! [[prompt_guard]]
//! name = "branch-protection"
//! command = "/path/to/guard-branch.sh"
//! timeout_secs = 5
//!
//! [[prompt_guard]]
//! name = "commit-reminder"
//! command = "/path/to/remind-commit.sh"
//! timeout_secs = 10
//! ```

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Maximum bytes to capture from a single guard's stdout.
/// Prevents prompt inflation and cost spiraling from runaway scripts.
const MAX_GUARD_OUTPUT_BYTES: usize = 32_768; // 32 KB

/// Configuration entry for a single prompt guard script.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptGuardEntry {
    /// Human-readable name for this guard (used in XML tag and logs).
    pub name: String,
    /// Shell command to execute (run via `sh -c`).
    pub command: String,
    /// Maximum execution time in seconds (default: 10).
    #[serde(default = "default_guard_timeout")]
    pub timeout_secs: u64,
}

fn default_guard_timeout() -> u64 {
    10
}

/// Result from executing a single prompt guard.
#[derive(Debug, Clone)]
pub struct PromptGuardResult {
    /// Guard name (from config).
    pub name: String,
    /// Captured stdout (injection text). Empty means no injection.
    pub output: String,
}

/// Context passed to guard scripts via stdin as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardContext {
    /// Absolute path to the project root directory.
    pub project_root: String,
    /// Current session ID (ULID).
    pub session_id: String,
    /// Tool name being executed (e.g., "codex", "claude-code").
    pub tool: String,
    /// Whether this is a resumed session (`--session` / `--last`).
    pub is_resume: bool,
    /// Current working directory.
    pub cwd: String,
}

/// Execute prompt guard scripts sequentially and collect results.
///
/// Each guard receives [`GuardContext`] as JSON on stdin. Stdout is captured
/// as injection text. Guards that fail (non-zero exit, timeout, spawn error)
/// are warned and skipped — they never block execution.
///
/// Returns results only for guards that produced non-empty output.
pub fn run_prompt_guards(
    guards: &[PromptGuardEntry],
    context: &GuardContext,
) -> Vec<PromptGuardResult> {
    if guards.is_empty() {
        return Vec::new();
    }

    let context_json = match serde_json::to_string(context) {
        Ok(json) => json,
        Err(e) => {
            tracing::warn!("Failed to serialize GuardContext: {e}");
            return Vec::new();
        }
    };

    let mut results = Vec::new();

    for guard in guards {
        match run_single_guard(guard, &context_json) {
            Ok(output) if !output.is_empty() => {
                results.push(PromptGuardResult {
                    name: guard.name.clone(),
                    output,
                });
            }
            Ok(_) => {
                tracing::debug!(guard = %guard.name, "Guard produced empty output, skipping");
            }
            Err(e) => {
                tracing::warn!(guard = %guard.name, "Guard failed, skipping: {e}");
            }
        }
    }

    results
}

/// Execute a single guard script, passing context JSON on stdin and capturing stdout.
///
/// Stdout is redirected to a tempfile to avoid pipe-buffer deadlock. Unlike a
/// pipe (which has ~64 KB kernel buffer), a tempfile has no write-side backpressure,
/// so the child never blocks on stdout regardless of output volume. The parent
/// reads the tempfile after the child exits — no background threads needed.
///
/// Output is capped at [`MAX_GUARD_OUTPUT_BYTES`] to prevent prompt inflation.
fn run_single_guard(guard: &PromptGuardEntry, context_json: &str) -> anyhow::Result<String> {
    // Use a tempfile for stdout to avoid pipe-buffer deadlock.
    // The child writes freely (no 64 KB limit); we read after exit.
    let stdout_file = tempfile::tempfile().map_err(|e| {
        anyhow::anyhow!(
            "Failed to create stdout tempfile for guard '{}': {e}",
            guard.name
        )
    })?;
    let stdout_for_child = stdout_file.try_clone().map_err(|e| {
        anyhow::anyhow!(
            "Failed to clone stdout tempfile for guard '{}': {e}",
            guard.name
        )
    })?;

    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(&guard.command)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(stdout_for_child))
        .stderr(Stdio::null());

    // Create new process group for clean timeout kill.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to spawn guard '{}': {e}", guard.name))?;

    // Write context JSON to stdin, then close it.
    if let Some(mut stdin) = child.stdin.take() {
        // Ignore write errors — script may have exited early.
        let _ = stdin.write_all(context_json.as_bytes());
        // stdin is dropped here, closing the pipe.
    }

    // Save the process group ID before the wait loop. On Unix, process_group(0)
    // makes the child the group leader (PGID == child PID).
    let pgid = child.id();
    let timeout = Duration::from_secs(guard.timeout_secs);
    let start = Instant::now();

    // Cache descendant PIDs periodically while child is alive. When the child
    // exits (success/failure), descendants are reparented to init and the
    // parent-PID chain breaks — so we use the last cached snapshot to clean up.
    #[cfg(target_os = "linux")]
    let mut cached_descendants: Vec<u32> = Vec::new();
    #[cfg(target_os = "linux")]
    let mut last_desc_scan = Instant::now();

    loop {
        // Refresh descendant cache BEFORE checking exit status. When the child
        // exits, do_exit() reparents descendants to init — so the parent-PID
        // chain is broken by the time try_wait() returns Some. Scanning here
        // (while child is alive) ensures we have a recent snapshot.
        //
        // First iteration always scans (empty cache); subsequent scans are
        // throttled to ~2/second to avoid excessive /proc walks.
        #[cfg(target_os = "linux")]
        if cached_descendants.is_empty() || last_desc_scan.elapsed() >= Duration::from_millis(500) {
            cached_descendants = collect_descendant_pids(pgid);
            last_desc_scan = Instant::now();
        }

        match child.try_wait()? {
            Some(status) => {
                // Kill cached descendants before reading output.
                #[cfg(target_os = "linux")]
                for pid in &cached_descendants {
                    // SAFETY: kill() is async-signal-safe. Positive PID targets one process.
                    unsafe {
                        libc::kill(*pid as i32, libc::SIGKILL);
                    }
                }

                let output = read_guard_output(stdout_file);

                if status.success() {
                    return Ok(output);
                } else {
                    let code = status.code().unwrap_or(-1);
                    anyhow::bail!("Guard '{}' exited with code {code}", guard.name);
                }
            }
            None => {
                if start.elapsed() >= timeout {
                    // Re-check exit state at timeout boundary. The process may
                    // have exited between the previous poll and timeout check.
                    if let Some(status) = child.try_wait()? {
                        let output = read_guard_output(stdout_file);
                        if status.success() {
                            return Ok(output);
                        }
                        let code = status.code().unwrap_or(-1);
                        anyhow::bail!("Guard '{}' exited with code {code}", guard.name);
                    }

                    // Timeout path: collect descendants BEFORE kill, then kill all.
                    //
                    // Order is critical on Linux: child.kill() triggers do_exit() which
                    // reparents descendants to init BEFORE the process reaches zombie state.
                    // So collect_descendant_pids() must run while the child is still alive
                    // and the parent-PID chain is intact.
                    //
                    // On non-Linux Unix: fall back to process group kill since /proc is
                    // unavailable. This is safe on the timeout path (unlike the success
                    // path) because PGID reuse within the timeout window is negligible.
                    #[cfg(target_os = "linux")]
                    let descendant_pids = collect_descendant_pids(pgid);

                    let _ = child.kill();

                    #[cfg(target_os = "linux")]
                    for pid in &descendant_pids {
                        // SAFETY: kill() is async-signal-safe. Positive PID targets one process.
                        unsafe {
                            libc::kill(*pid as i32, libc::SIGKILL);
                        }
                    }

                    #[cfg(all(unix, not(target_os = "linux")))]
                    {
                        // No /proc on macOS/BSD — use process group kill as fallback.
                        // SAFETY: Negative PID targets the process group.
                        unsafe {
                            libc::kill(-(pgid as i32), libc::SIGKILL);
                        }
                    }

                    let _ = child.wait();
                    anyhow::bail!(
                        "Guard '{}' timed out after {}s",
                        guard.name,
                        guard.timeout_secs
                    );
                }
                // Enforce output cap during execution. The tempfile is unbounded
                // while the child runs; a noisy guard (e.g. `yes`) could fill /tmp.
                if let Ok(meta) = stdout_file.metadata() {
                    if meta.len() > MAX_GUARD_OUTPUT_BYTES as u64 {
                        let _ = child.kill();
                        let _ = child.wait();
                        anyhow::bail!(
                            "Guard '{}' output exceeded {}B cap",
                            guard.name,
                            MAX_GUARD_OUTPUT_BYTES
                        );
                    }
                }

                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

/// Read guard output from the stdout tempfile, capped at [`MAX_GUARD_OUTPUT_BYTES`].
///
/// Seeks to the start of the file and reads up to the cap. The tempfile is
/// automatically cleaned up when the `File` handle is dropped.
fn read_guard_output(mut file: std::fs::File) -> String {
    use std::io::{Read, Seek, SeekFrom};

    if file.seek(SeekFrom::Start(0)).is_err() {
        return String::new();
    }

    let mut buf = vec![0u8; MAX_GUARD_OUTPUT_BYTES];
    let mut total = 0;
    while total < MAX_GUARD_OUTPUT_BYTES {
        match file.read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(_) => break,
        }
    }

    String::from_utf8_lossy(&buf[..total]).trim().to_string()
}

#[cfg(target_os = "linux")]
fn collect_descendant_pids(root_pid: u32) -> Vec<u32> {
    use std::collections::{HashMap, VecDeque};

    // Guard against PID reuse: only proceed if root_pid is still our direct child.
    if read_parent_pid(root_pid) != Some(std::process::id()) {
        return Vec::new();
    }

    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };

    let mut children_by_parent: HashMap<u32, Vec<u32>> = HashMap::new();
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        if let Some(ppid) = read_parent_pid(pid) {
            children_by_parent.entry(ppid).or_default().push(pid);
        }
    }

    let mut queue = VecDeque::from([root_pid]);
    let mut descendants = Vec::new();

    while let Some(parent) = queue.pop_front() {
        if let Some(children) = children_by_parent.get(&parent) {
            for &child in children {
                descendants.push(child);
                queue.push_back(child);
            }
        }
    }

    // Kill deeper descendants first to reduce re-parenting windows.
    descendants.reverse();
    descendants
}

#[cfg(target_os = "linux")]
fn read_parent_pid(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let idx = stat.rfind(')')?;
    let after_comm = stat.get(idx + 2..)?; // skip ") "
    // Fields after comm: state ppid ...
    after_comm.split_whitespace().nth(1)?.parse().ok()
}

/// Format guard results into XML blocks for prompt injection.
///
/// Returns `None` if all results are empty. Output values are XML-escaped.
///
/// Example output:
/// ```text
/// <prompt-guard name="branch-protection">
/// You are on branch main. Do NOT commit directly.
/// </prompt-guard>
/// <prompt-guard name="commit-reminder">
/// You have uncommitted changes. Commit before stopping.
/// </prompt-guard>
/// ```
pub fn format_guard_output(results: &[PromptGuardResult]) -> Option<String> {
    let non_empty: Vec<_> = results.iter().filter(|r| !r.output.is_empty()).collect();
    if non_empty.is_empty() {
        return None;
    }

    let mut buf = String::new();
    for (i, result) in non_empty.iter().enumerate() {
        if i > 0 {
            buf.push('\n');
        }
        // Truncate escaped text to MAX_GUARD_OUTPUT_BYTES to bound the
        // final prompt size. XML escaping can expand characters (e.g.,
        // & → &amp;), so the raw byte cap alone is not sufficient.
        let escaped = xml_escape_text(&result.output);
        let capped = if escaped.len() > MAX_GUARD_OUTPUT_BYTES {
            // Truncate at a char boundary to avoid splitting UTF-8
            let mut end = MAX_GUARD_OUTPUT_BYTES;
            while end > 0 && !escaped.is_char_boundary(end) {
                end -= 1;
            }
            &escaped[..end]
        } else {
            &escaped
        };
        buf.push_str(&format!(
            "<prompt-guard name=\"{}\">\n{}\n</prompt-guard>",
            xml_escape_attr(&result.name),
            capped,
        ));
    }

    Some(buf)
}

/// Escape a string for use in an XML attribute value (inside double quotes).
fn xml_escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Escape a string for use as XML text content.
fn xml_escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
#[path = "guard_tests.rs"]
mod tests;
