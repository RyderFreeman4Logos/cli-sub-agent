use anyhow::{Context, Result};
use serde_json::Value as JsonValue;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tracing::{debug, info};

/// Handle setup for Claude Code MCP integration
pub(crate) fn handle_setup_claude_code() -> Result<()> {
    let csa_path = detect_csa_binary()?;
    let config_path = get_claude_code_config_path()?;

    eprintln!("Setting up MCP integration for Claude Code...");
    eprintln!("CSA binary: {}", csa_path.display());
    eprintln!("Config file: {}", config_path.display());

    // Create parent directory if it doesn't exist
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent).context("Failed to create config directory")?;
    }

    // Read existing config or create new
    let mut config = if config_path.exists() {
        let content = fs::read_to_string(&config_path).context("Failed to read config file")?;
        serde_json::from_str::<JsonValue>(&content).context("Failed to parse config JSON")?
    } else {
        serde_json::json!({
            "mcpServers": {}
        })
    };

    // Add or update csa server entry
    if let Some(servers) = config.get_mut("mcpServers").and_then(|v| v.as_object_mut()) {
        servers.insert(
            "csa".to_string(),
            serde_json::json!({
                "command": csa_path.to_string_lossy(),
                "args": ["mcp-server"]
            }),
        );
    } else {
        anyhow::bail!("Config file has unexpected structure (missing 'mcpServers' object)");
    }

    // Write back
    let json_str = serde_json::to_string_pretty(&config).context("Failed to serialize config")?;
    fs::write(&config_path, json_str).context("Failed to write config file")?;

    eprintln!("\n✓ Successfully configured Claude Code MCP integration");
    eprintln!("Config location: {}", config_path.display());
    eprintln!("\nRestart Claude Code to activate the integration.");

    Ok(())
}

/// Handle setup for Codex CLI MCP integration
pub(crate) fn handle_setup_codex() -> Result<()> {
    let csa_path = detect_csa_binary()?;
    let config_path = get_codex_config_path()?;

    eprintln!("Setting up MCP integration for Codex CLI...");
    eprintln!("CSA binary: {}", csa_path.display());
    eprintln!("Config file: {}", config_path.display());

    // Create parent directory if it doesn't exist
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent).context("Failed to create config directory")?;
    }

    // Read existing config or create new
    let mut content = if config_path.exists() {
        fs::read_to_string(&config_path).context("Failed to read config file")?
    } else {
        String::new()
    };

    // Check if csa server already configured
    if content.contains(r#"name = "csa""#) {
        eprintln!("\n⚠ CSA MCP server already configured in Codex config");
        eprintln!("Config location: {}", config_path.display());
        return Ok(());
    }

    // Append MCP server configuration
    let mcp_config = format!(
        r#"
[[mcp_servers]]
name = "csa"
command = "{}"
args = ["mcp-server"]
"#,
        csa_path.to_string_lossy()
    );

    content.push_str(&mcp_config);
    fs::write(&config_path, content).context("Failed to write config file")?;

    eprintln!("\n✓ Successfully configured Codex CLI MCP integration");
    eprintln!("Config location: {}", config_path.display());
    eprintln!("\nRestart Codex CLI to activate the integration.");

    Ok(())
}

/// Handle setup for OpenCode MCP integration
pub(crate) fn handle_setup_opencode() -> Result<()> {
    let csa_path = detect_csa_binary()?;
    let config_path = get_opencode_config_path()?;

    eprintln!("Setting up MCP integration for OpenCode...");
    eprintln!("CSA binary: {}", csa_path.display());
    eprintln!("Config file: {}", config_path.display());

    // Create parent directory if it doesn't exist
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent).context("Failed to create config directory")?;
    }

    // Read existing config or create new
    let mut config = if config_path.exists() {
        let content = fs::read_to_string(&config_path).context("Failed to read config file")?;
        serde_json::from_str::<JsonValue>(&content).context("Failed to parse config JSON")?
    } else {
        serde_json::json!({
            "mcpServers": {}
        })
    };

    // Add or update csa server entry
    if let Some(servers) = config.get_mut("mcpServers").and_then(|v| v.as_object_mut()) {
        servers.insert(
            "csa".to_string(),
            serde_json::json!({
                "command": csa_path.to_string_lossy(),
                "args": ["mcp-server"]
            }),
        );
    } else {
        anyhow::bail!("Config file has unexpected structure (missing 'mcpServers' object)");
    }

    // Write back
    let json_str = serde_json::to_string_pretty(&config).context("Failed to serialize config")?;
    fs::write(&config_path, json_str).context("Failed to write config file")?;

    eprintln!("\n✓ Successfully configured OpenCode MCP integration");
    eprintln!("Config location: {}", config_path.display());
    eprintln!("\nRestart OpenCode to activate the integration.");

    Ok(())
}

/// Detect csa binary path
fn detect_csa_binary() -> Result<PathBuf> {
    // Try which::which first
    if let Ok(path) = which::which("csa") {
        return Ok(path);
    }

    // Fall back to current_exe
    std::env::current_exe().context("Failed to detect csa binary path")
}

/// Get Claude Code config path (~/.claude/mcp-settings.json)
fn get_claude_code_config_path() -> Result<PathBuf> {
    let home = directories::BaseDirs::new()
        .context("Could not determine home directory")?
        .home_dir()
        .to_path_buf();
    Ok(home.join(".claude").join("mcp-settings.json"))
}

/// Get Codex config path (~/.codex/config.toml)
fn get_codex_config_path() -> Result<PathBuf> {
    let home = directories::BaseDirs::new()
        .context("Could not determine home directory")?
        .home_dir()
        .to_path_buf();
    Ok(home.join(".codex").join("config.toml"))
}

/// Get OpenCode config path (~/.config/opencode/config.json)
fn get_opencode_config_path() -> Result<PathBuf> {
    let home = directories::BaseDirs::new()
        .context("Could not determine home directory")?
        .home_dir()
        .to_path_buf();
    Ok(home.join(".config").join("opencode").join("config.json"))
}

// ── Review Gate Setup ─────────────────────────────────────────────────────────

/// Generalized pre-push review gate hook script, embedded at compile time.
const REVIEW_CHECK_TEMPLATE: &str = r#"#!/usr/bin/env bash
# Git pre-push hook: verify csa review has been run on current HEAD.
# Installed by: csa setup review-gate
#
# Fast path: stat .csa/state/review-gate/<branch_safe>-<short_sha>.pass
#   millisecond check; new commits auto-invalidate (different SHA → different filename).
# Slow path (fallback): csa review --check-verdict scans session store.

set -euo pipefail

if [ "${CSA_SKIP_REVIEW_CHECK:-0}" = "1" ]; then
  timestamp="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
  head_sha="$(git rev-parse HEAD 2>/dev/null || echo "<unknown-head>")"
  author_email="$(git config user.email 2>/dev/null || echo "<unknown-email>")"
  raw_reason="${CSA_SKIP_REVIEW_CHECK_REASON:-<unspecified>}"
  reason="$(
    printf '%s' "${raw_reason}" \
      | tr '\r\n\t' '   ' \
      | sed -E 's/[[:space:]]+/ /g; s/^ //; s/ $//'
  )"
  [ -z "${reason}" ] && reason="<unspecified>"

  mkdir -p .csa
  printf '%s %s %s %s\n' "${timestamp}" "${head_sha}" "${author_email}" "${reason}" >> .csa/review-bypass.log
  echo "WARNING: review-check bypassed via CSA_SKIP_REVIEW_CHECK=1 for ${head_sha:0:11}; logged to .csa/review-bypass.log. Reason: ${reason}" >&2
  exit 0
fi

# CSA-managed executors run their own review gates in the workflow. Skipping
# here prevents pre-push from recursively spawning csa review inside csa.
CSA_DEPTH_VALUE="${CSA_DEPTH:-0}"
if [ -n "${CSA_SESSION_ID:-}" ] || [[ "${CSA_DEPTH_VALUE}" =~ ^[0-9]+$ && "${CSA_DEPTH_VALUE}" -gt 0 ]]; then
  echo "pre-push: Review gate skipped inside CSA executor session; CSA workflow owns review enforcement."
  exit 0
fi

# Skip if csa is not installed in this repo
if ! command -v csa >/dev/null 2>&1; then
  exit 0
fi

CURRENT_HEAD="$(git rev-parse HEAD)"
CURRENT_BRANCH="$(git branch --show-current)"

# Skip for main/dev branches (direct pushes are blocked by branch protection)
if [ "${CURRENT_BRANCH}" = "main" ] || [ "${CURRENT_BRANCH}" = "dev" ]; then
  exit 0
fi

# ── Fast path: SHA-pinned marker file ────────────────────────────────────────
# Sanitize branch name the same way review_gate::sanitize_branch does:
#   '/' → '__', any non-[a-zA-Z0-9._-] → '_'
_sanitize_branch() {
  printf '%s' "$1" \
    | sed 's|/|__|g' \
    | sed 's|[^a-zA-Z0-9._-]|_|g'
}

SHORT_SHA="${CURRENT_HEAD:0:11}"
SAFE_BRANCH="$(_sanitize_branch "${CURRENT_BRANCH}")"
MARKER=".csa/state/review-gate/${SAFE_BRANCH}-${SHORT_SHA}.pass"

if [ -f "${MARKER}" ]; then
  echo "pre-push: Review gate passed (marker) for ${CURRENT_BRANCH} at ${SHORT_SHA}."
  exit 0
fi

# ── Slow path: session-store scan ────────────────────────────────────────────
if csa review --check-verdict; then
  echo "pre-push: Full-diff review verified for HEAD ${SHORT_SHA}."
  exit 0
fi

# ── Blocked — emit reverse prompt injection for agent context ─────────────────
cat >&2 <<GATE_BLOCKED
<!-- CSA:REVIEW_GATE_BLOCKED branch="${CURRENT_BRANCH}" head_sha="${CURRENT_HEAD}" -->
Push blocked: no passing review found for current HEAD.
Run: csa review --range main...HEAD --sa-mode true
Wait for PASS verdict, then retry push.
<!-- /CSA:REVIEW_GATE_BLOCKED -->
GATE_BLOCKED

echo "" >&2
echo "ERROR: Push blocked — no PASS/CLEAN full-diff csa review session recorded for ${CURRENT_BRANCH} at ${SHORT_SHA}." >&2
exit 1
"#;

/// Rate-limit interval for auto-setup checks (1 hour).
const REVIEW_GATE_CHECK_INTERVAL_SECS: u64 = 3600;
/// Timestamp file name stored in the project state dir.
const REVIEW_GATE_TIMESTAMP_FILE: &str = "review-gate-check-ts";

/// Handle `csa setup review-gate [--check]`.
pub(crate) fn handle_setup_review_gate(project_root: &Path, check: bool) -> Result<()> {
    if check {
        return report_review_gate_status(project_root);
    }
    install_review_gate(project_root, /*verbose=*/ true)
}

/// Spawn a background task to auto-setup the review gate if `[hooks].auto_setup_review_gate = true`.
///
/// Non-blocking: returns immediately. Skipped in CI environments.
pub(crate) fn spawn_review_gate_setup_if_needed(
    project_root: &Path,
    global_config: Option<&csa_config::GlobalConfig>,
) {
    let auto_setup = global_config
        .map(|c| c.hooks.auto_setup_review_gate)
        .unwrap_or(false);
    if !auto_setup {
        return;
    }
    if std::env::var_os("CI").is_some_and(|v| !v.is_empty()) {
        debug!("review-gate auto-setup: skipping (CI environment)");
        return;
    }
    if !project_root.join(".git").exists() {
        debug!("review-gate auto-setup: skipping (no .git directory)");
        return;
    }

    let project_root = project_root.to_path_buf();
    tokio::spawn(async move {
        if let Err(e) = check_and_setup_review_gate_bg(&project_root).await {
            debug!("review-gate auto-setup: background task failed: {e:#}");
        }
    });
}

/// Background async task: rate-limited auto-setup of the review gate.
async fn check_and_setup_review_gate_bg(project_root: &Path) -> anyhow::Result<()> {
    let state_dir = csa_session::get_session_root(project_root)?;
    let ts_path = state_dir.join(REVIEW_GATE_TIMESTAMP_FILE);

    if !needs_review_gate_check(&ts_path)? {
        debug!(
            "review-gate auto-setup: skipped (checked < {REVIEW_GATE_CHECK_INTERVAL_SECS}s ago)"
        );
        return Ok(());
    }
    write_review_gate_timestamp(&ts_path)?;

    info!("review-gate auto-setup: running setup");
    install_review_gate(project_root, /*verbose=*/ false)?;
    info!("review-gate auto-setup: complete");
    Ok(())
}

/// Returns true when the timestamp file is absent or older than CHECK_INTERVAL_SECS.
fn needs_review_gate_check(ts_path: &Path) -> anyhow::Result<bool> {
    let metadata = match std::fs::metadata(ts_path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(e) => return Err(e.into()),
    };
    let modified = metadata.modified()?;
    let age = SystemTime::now()
        .duration_since(modified)
        .unwrap_or(std::time::Duration::MAX);
    Ok(age.as_secs() >= REVIEW_GATE_CHECK_INTERVAL_SECS)
}

/// Write (or touch) the rate-limit timestamp file.
fn write_review_gate_timestamp(ts_path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = ts_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(ts_path, b"")?;
    Ok(())
}

/// Install all review gate components in `project_root`.
fn install_review_gate(project_root: &Path, verbose: bool) -> Result<()> {
    if which::which("lefthook").is_err() {
        anyhow::bail!(
            "lefthook not found. Install it with: mise install lefthook\n\
             Then re-run: csa setup review-gate"
        );
    }

    merge_lefthook_review_check(project_root).context("Failed to update lefthook.yml")?;
    install_review_check_script(project_root).context("Failed to install review-check.sh")?;
    fs::create_dir_all(project_root.join(".csa/state/review-gate"))
        .context("Failed to create .csa/state/review-gate/")?;
    run_lefthook_install_sync(project_root).context("Failed to run `lefthook install`")?;

    if verbose {
        eprintln!("✓ Review gate installed successfully");
        eprintln!("  lefthook.yml updated with pre-push.commands.review-check");
        eprintln!("  scripts/hooks/review-check.sh written");
        eprintln!("  .csa/state/review-gate/ created");
        eprintln!("  Git hooks activated via `lefthook install`");
    }
    Ok(())
}

/// Merge `pre-push.commands.review-check` into `lefthook.yml` (idempotent).
///
/// Preserves all existing sections and comments. Appends the `pre-push` section
/// when absent; inserts the entry after `  commands:` when the section already exists.
fn merge_lefthook_review_check(project_root: &Path) -> Result<()> {
    let lefthook_path = project_root.join("lefthook.yml");

    let existing = if lefthook_path.exists() {
        fs::read_to_string(&lefthook_path).context("Failed to read lefthook.yml")?
    } else {
        String::new()
    };

    if existing.contains("review-check:") {
        return Ok(()); // already installed — idempotent
    }

    let new_content = build_merged_lefthook(&existing);
    fs::write(&lefthook_path, new_content).context("Failed to write lefthook.yml")?;
    Ok(())
}

/// Build the merged lefthook.yml content string.
fn build_merged_lefthook(existing: &str) -> String {
    const ENTRY_LINES: [&str; 2] = [
        "    review-check:",
        "      run: scripts/hooks/review-check.sh",
    ];

    let lines: Vec<&str> = existing.lines().collect();
    let trailing_newline = existing.ends_with('\n');
    let mut in_pre_push = false;

    for (i, &line) in lines.iter().enumerate() {
        // Track top-level YAML keys (non-empty, non-comment, non-indented).
        if !line.is_empty()
            && !line.starts_with(' ')
            && !line.starts_with('\t')
            && !line.starts_with('#')
        {
            in_pre_push = line.starts_with("pre-push:");
        }

        // Found "  commands:" inside the pre-push section — insert after it.
        if in_pre_push && line.trim() == "commands:" {
            let mut result_lines: Vec<&str> = Vec::with_capacity(lines.len() + ENTRY_LINES.len());
            result_lines.extend_from_slice(&lines[..=i]);
            result_lines.extend_from_slice(&ENTRY_LINES);
            result_lines.extend_from_slice(&lines[i + 1..]);
            let mut out = result_lines.join("\n");
            if trailing_newline {
                out.push('\n');
            }
            return out;
        }
    }

    // No pre-push.commands found — append the full section.
    let mut out = existing.trim_end().to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str("pre-push:\n  commands:\n");
    for line in &ENTRY_LINES {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Write `scripts/hooks/review-check.sh` from the embedded template.
fn install_review_check_script(project_root: &Path) -> Result<()> {
    let scripts_dir = project_root.join("scripts/hooks");
    fs::create_dir_all(&scripts_dir).context("Failed to create scripts/hooks/")?;

    let script_path = scripts_dir.join("review-check.sh");

    // Do not overwrite an existing script unless we installed it (check for our marker comment).
    if script_path.exists() {
        let existing = fs::read_to_string(&script_path).unwrap_or_default();
        if !existing.contains("Installed by: csa setup review-gate") {
            // User has a custom script — leave it untouched.
            return Ok(());
        }
    }

    fs::write(&script_path, REVIEW_CHECK_TEMPLATE).context("Failed to write review-check.sh")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&script_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms)?;
    }

    Ok(())
}

/// Run `lefthook install` synchronously.
fn run_lefthook_install_sync(project_root: &Path) -> Result<()> {
    let status = std::process::Command::new("lefthook")
        .arg("install")
        .current_dir(project_root)
        .status()
        .context("Failed to spawn `lefthook install`")?;
    if !status.success() {
        anyhow::bail!("`lefthook install` exited with status {status}");
    }
    Ok(())
}

/// Report the installation status of all review gate components.
fn report_review_gate_status(project_root: &Path) -> Result<()> {
    let lefthook_ok = which::which("lefthook").is_ok();

    let lefthook_path = project_root.join("lefthook.yml");
    let lefthook_has_entry = lefthook_path.exists() && {
        fs::read_to_string(&lefthook_path)
            .map(|c| c.contains("review-check:"))
            .unwrap_or(false)
    };

    let script_ok = project_root.join("scripts/hooks/review-check.sh").exists();

    let gate_dir_ok = project_root.join(".csa/state/review-gate").exists();

    let pre_push_path = project_root.join(".git/hooks/pre-push");
    let git_hook_ok = pre_push_path.exists() && {
        fs::read_to_string(&pre_push_path)
            .map(|c| c.contains("lefthook"))
            .unwrap_or(false)
    };

    let mark = |ok: bool| if ok { "✓" } else { "✗" };
    eprintln!("Review gate status for {}:", project_root.display());
    eprintln!("  {} lefthook binary available", mark(lefthook_ok));
    eprintln!(
        "  {} lefthook.yml has pre-push.commands.review-check",
        mark(lefthook_has_entry)
    );
    eprintln!("  {} scripts/hooks/review-check.sh exists", mark(script_ok));
    eprintln!(
        "  {} .csa/state/review-gate/ directory exists",
        mark(gate_dir_ok)
    );
    eprintln!(
        "  {} .git/hooks/pre-push (lefthook-managed)",
        mark(git_hook_ok)
    );

    let all_ok = lefthook_ok && lefthook_has_entry && script_ok && gate_dir_ok && git_hook_ok;
    if all_ok {
        eprintln!("\nStatus: Fully installed");
    } else {
        eprintln!("\nStatus: Not fully installed. Run: csa setup review-gate");
    }
    Ok(())
}

#[cfg(test)]
mod review_gate_script_tests;

#[cfg(test)]
mod review_gate_tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn idempotent_when_review_check_already_present() {
        let input =
            "pre-push:\n  commands:\n    review-check:\n      run: scripts/hooks/review-check.sh\n";
        // merge_lefthook_review_check guards the idempotency check at the file level.
        let td = TempDir::new().unwrap();
        let lf = td.path().join("lefthook.yml");
        fs::write(&lf, input).unwrap();
        merge_lefthook_review_check(td.path()).unwrap();
        let content = fs::read_to_string(&lf).unwrap();
        // Only one review-check entry should exist.
        assert_eq!(content.matches("review-check:").count(), 1);
    }

    #[test]
    fn inserts_after_commands_in_existing_pre_push() {
        let input = "pre-push:\n  commands:\n    version-check:\n      run: scripts/hooks/version-check.sh\n";
        let result = build_merged_lefthook(input);
        assert!(result.contains("    review-check:"), "entry inserted");
        // review-check should appear before version-check
        let rc_pos = result.find("    review-check:").unwrap();
        let vc_pos = result.find("    version-check:").unwrap();
        assert!(rc_pos < vc_pos, "review-check before version-check");
    }

    #[test]
    fn appends_section_when_no_pre_push() {
        let input = "pre-commit:\n  commands:\n    quality-gates:\n      run: just pre-commit\n";
        let result = build_merged_lefthook(input);
        assert!(result.contains("pre-push:"), "pre-push section added");
        assert!(result.contains("review-check:"), "entry added");
    }

    #[test]
    fn creates_minimal_lefthook_from_empty() {
        let result = build_merged_lefthook("");
        assert!(result.contains("pre-push:"));
        assert!(result.contains("review-check:"));
    }

    #[test]
    fn preserves_trailing_newline() {
        let input = "no_tty: true\npre-push:\n  commands:\n    x:\n      run: x\n";
        let result = build_merged_lefthook(input);
        assert!(result.ends_with('\n'));
    }

    #[test]
    fn needs_check_true_when_no_file() {
        let td = TempDir::new().unwrap();
        let ts = td.path().join(REVIEW_GATE_TIMESTAMP_FILE);
        assert!(needs_review_gate_check(&ts).unwrap());
    }

    #[test]
    fn needs_check_false_after_recent_write() {
        let td = TempDir::new().unwrap();
        let ts = td.path().join(REVIEW_GATE_TIMESTAMP_FILE);
        fs::write(&ts, b"").unwrap();
        assert!(!needs_review_gate_check(&ts).unwrap());
    }
}
