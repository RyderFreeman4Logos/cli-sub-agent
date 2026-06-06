use anyhow::{Context, Result};
use serde_json::Value as JsonValue;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;
use tracing::{debug, info, warn};

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

/// Branch-protection hook script installed by `csa setup review-gate`.
const BRANCH_PROTECTION_TEMPLATE: &str = include_str!("setup_cmds/branch-protection.sh");

/// Generalized pre-push review gate hook script, embedded at compile time.
const REVIEW_CHECK_TEMPLATE: &str = include_str!("setup_cmds/review-check.sh");

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
    match review_gate_opt_in_signal(project_root) {
        Some(signal) => debug!("review-gate auto-setup: opt-in detected ({signal})"),
        None => {
            debug!("review-gate auto-setup: skipping (repo is not CSA-managed / opted in)");
            return;
        }
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
    match review_gate_opt_in_signal(project_root) {
        Some(signal) => debug!("review-gate auto-setup: opt-in detected ({signal})"),
        None => {
            debug!("review-gate auto-setup: skipping (repo is not CSA-managed / opted in)");
            return Ok(());
        }
    }

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

pub(crate) fn review_gate_opt_in_signal(project_root: &Path) -> Option<&'static str> {
    if git_path_is_tracked(project_root, "lefthook.yml") {
        return Some("tracked lefthook.yml");
    }
    if git_path_is_tracked(project_root, "scripts/hooks/review-check.sh") {
        return Some("tracked scripts/hooks/review-check.sh");
    }
    if project_root.join("patterns/csa-review").is_dir() {
        return Some("patterns/csa-review/");
    }
    None
}

fn git_path_is_tracked(project_root: &Path, relative_path: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["ls-files", "--error-unmatch", "--", relative_path])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HookTrackingStatus {
    Tracked,
    Untracked,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HookInstallDecision {
    Write,
    SkipIdentical,
    SkipTracked,
    SkipUnknown,
}

fn hook_tracking_status(project_root: &Path, relative_path: &str) -> HookTrackingStatus {
    let inside_work_tree = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rev-parse", "--is-inside-work-tree"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match inside_work_tree {
        Ok(status) if status.success() => {}
        Ok(_) | Err(_) => return HookTrackingStatus::Unknown,
    }

    match Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["ls-files", "--error-unmatch", "--", relative_path])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) if status.success() => HookTrackingStatus::Tracked,
        Ok(_) => HookTrackingStatus::Untracked,
        Err(_) => HookTrackingStatus::Unknown,
    }
}

fn should_install_hook(
    existing: Option<&[u8]>,
    would_write: &[u8],
    tracked_status: HookTrackingStatus,
) -> HookInstallDecision {
    match existing {
        None => HookInstallDecision::Write,
        Some(existing_bytes) if existing_bytes == would_write => HookInstallDecision::SkipIdentical,
        Some(_) => match tracked_status {
            HookTrackingStatus::Tracked => HookInstallDecision::SkipTracked,
            HookTrackingStatus::Untracked => HookInstallDecision::Write,
            HookTrackingStatus::Unknown => HookInstallDecision::SkipUnknown,
        },
    }
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

    merge_lefthook_review_gate(project_root).context("Failed to update lefthook.yml")?;
    install_branch_protection_script(project_root)
        .context("Failed to install branch-protection.sh")?;
    install_review_check_script(project_root).context("Failed to install review-check.sh")?;
    fs::create_dir_all(project_root.join(".csa/state/review-gate"))
        .context("Failed to create .csa/state/review-gate/")?;
    run_lefthook_install_sync(project_root).context("Failed to run `lefthook install`")?;

    if verbose {
        eprintln!("✓ Review gate installed successfully");
        eprintln!("  lefthook.yml updated with pre-push branch-protection + review-check");
        eprintln!("  scripts/hooks/branch-protection.sh written");
        eprintln!("  scripts/hooks/review-check.sh written");
        eprintln!("  .csa/state/review-gate/ created");
        eprintln!("  Git hooks activated via `lefthook install`");
    }
    Ok(())
}

/// Merge review-gate pre-push commands into `lefthook.yml` (idempotent).
///
/// Preserves all existing sections and comments. Appends the `pre-push` section
/// when absent; inserts missing entries after `  commands:` when the section exists.
fn merge_lefthook_review_gate(project_root: &Path) -> Result<()> {
    let lefthook_path = project_root.join("lefthook.yml");

    let existing = if lefthook_path.exists() {
        fs::read_to_string(&lefthook_path).context("Failed to read lefthook.yml")?
    } else {
        String::new()
    };

    let new_content = build_merged_lefthook(&existing);
    if new_content == existing {
        return Ok(());
    }
    fs::write(&lefthook_path, new_content).context("Failed to write lefthook.yml")?;
    Ok(())
}

/// Build the merged lefthook.yml content string.
fn build_merged_lefthook(existing: &str) -> String {
    let needs_branch_protection = !pre_push_contains_command(existing, "branch-protection");
    let needs_review_check = !pre_push_contains_command(existing, "review-check");
    if !needs_branch_protection && !needs_review_check {
        return existing.to_string();
    }
    let entry_lines = missing_pre_push_entry_lines(needs_branch_protection, needs_review_check);

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
            let mut result_lines: Vec<&str> = Vec::with_capacity(lines.len() + entry_lines.len());
            result_lines.extend_from_slice(&lines[..=i]);
            result_lines.extend(entry_lines.iter().copied());
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
    for line in &entry_lines {
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn pre_push_contains_command(existing: &str, command_name: &str) -> bool {
    let needle = format!("{command_name}:");
    let mut in_pre_push = false;

    for line in existing.lines() {
        if !line.is_empty()
            && !line.starts_with(' ')
            && !line.starts_with('\t')
            && !line.starts_with('#')
        {
            in_pre_push = line.starts_with("pre-push:");
        }

        if in_pre_push && line.trim() == needle {
            return true;
        }
    }

    false
}

fn missing_pre_push_entry_lines(
    needs_branch_protection: bool,
    needs_review_check: bool,
) -> Vec<&'static str> {
    let mut entry_lines = Vec::new();
    if needs_branch_protection {
        entry_lines.extend_from_slice(&[
            "    branch-protection:",
            "      run: scripts/hooks/branch-protection.sh",
        ]);
    }
    if needs_review_check {
        entry_lines.extend_from_slice(&[
            "    review-check:",
            "      run: scripts/hooks/review-check.sh",
        ]);
    }
    entry_lines
}

/// Write `scripts/hooks/branch-protection.sh` from the embedded template.
fn install_branch_protection_script(project_root: &Path) -> Result<()> {
    install_managed_hook_script(
        project_root,
        "scripts/hooks/branch-protection.sh",
        BRANCH_PROTECTION_TEMPLATE,
        "branch-protection.sh",
    )
}

/// Write `scripts/hooks/review-check.sh` from the embedded template.
fn install_review_check_script(project_root: &Path) -> Result<()> {
    install_managed_hook_script(
        project_root,
        "scripts/hooks/review-check.sh",
        REVIEW_CHECK_TEMPLATE,
        "review-check.sh",
    )
}

fn install_managed_hook_script(
    project_root: &Path,
    relative_path: &str,
    template: &str,
    hook_name: &str,
) -> Result<()> {
    let script_path = project_root.join(relative_path);
    let existing = match fs::read(&script_path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            warn!(
                path = %script_path.display(),
                error = %error,
                "review-gate hook install: failed to read existing hook; leaving it untouched"
            );
            return Ok(());
        }
    };
    let tracked_status = existing
        .as_ref()
        .map_or(HookTrackingStatus::Untracked, |_| {
            hook_tracking_status(project_root, relative_path)
        });

    match should_install_hook(existing.as_deref(), template.as_bytes(), tracked_status) {
        HookInstallDecision::Write => {}
        HookInstallDecision::SkipIdentical => return Ok(()),
        HookInstallDecision::SkipTracked => {
            warn!(
                path = %script_path.display(),
                "review-gate hook install: respecting git-tracked hook; CSA will not manage or overwrite it"
            );
            return Ok(());
        }
        HookInstallDecision::SkipUnknown => {
            warn!(
                path = %script_path.display(),
                "review-gate hook install: could not determine whether existing hook is git-tracked; leaving it untouched"
            );
            return Ok(());
        }
    }

    if let Some(parent) = script_path.parent() {
        fs::create_dir_all(parent).context("Failed to create scripts/hooks/")?;
    }
    fs::write(&script_path, template).with_context(|| format!("Failed to write {hook_name}"))?;

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
    let status = Command::new("lefthook")
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
    let lefthook_content = if lefthook_path.exists() {
        fs::read_to_string(&lefthook_path).unwrap_or_default()
    } else {
        String::new()
    };
    let lefthook_has_entry = pre_push_contains_command(&lefthook_content, "branch-protection")
        && pre_push_contains_command(&lefthook_content, "review-check");

    let branch_script_ok = project_root
        .join("scripts/hooks/branch-protection.sh")
        .exists();
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
        "  {} lefthook.yml has pre-push branch-protection + review-check",
        mark(lefthook_has_entry)
    );
    eprintln!(
        "  {} scripts/hooks/branch-protection.sh exists",
        mark(branch_script_ok)
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

    let all_ok = lefthook_ok
        && lefthook_has_entry
        && branch_script_ok
        && script_ok
        && gate_dir_ok
        && git_hook_ok;
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
mod review_gate_tests;
