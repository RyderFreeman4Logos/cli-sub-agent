use anyhow::{Context, Result};
use serde_json::Value as JsonValue;
use std::fs;
use std::path::PathBuf;

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
