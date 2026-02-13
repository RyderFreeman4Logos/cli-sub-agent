use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use std::process::Command;

/// Handle self-update command
pub(crate) fn handle_self_update(check_only: bool) -> Result<()> {
    let current_version = env!("CARGO_PKG_VERSION");
    eprintln!("Current version: v{}", current_version);

    // Fetch latest release info from GitHub API
    let release_info = fetch_latest_release()?;
    let latest_version = release_info
        .tag_name
        .strip_prefix('v')
        .unwrap_or(&release_info.tag_name);

    eprintln!("Latest version:  v{}", latest_version);

    // Compare versions
    if current_version == latest_version {
        eprintln!("Already up to date.");
        return Ok(());
    }

    if check_only {
        eprintln!(
            "\nUpdate available: v{} → v{}",
            current_version, latest_version
        );
        eprintln!("Run 'csa self-update' to install the latest version.");
        return Ok(());
    }

    // Perform update
    eprintln!("\nUpdating to v{}...", latest_version);
    perform_update(&release_info, current_version, latest_version)?;

    eprintln!(
        "\n✓ Successfully updated from v{} to v{}",
        current_version, latest_version
    );
    eprintln!("Restart your shell or run 'hash -r' to use the new version.");

    Ok(())
}

/// Fetch latest release information from GitHub API
fn fetch_latest_release() -> Result<ReleaseInfo> {
    let url = "https://api.github.com/repos/RyderFreeman4Logos/cli-sub-agent/releases/latest";

    let output = Command::new("curl")
        .args(["-sL", url])
        .output()
        .context("Failed to execute curl. Is curl installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to fetch release info: {}", stderr);
    }

    let json = String::from_utf8_lossy(&output.stdout);

    // Check if the response is a 404 error
    if let Ok(error_response) = serde_json::from_str::<GitHubError>(&json) {
        if error_response.message == "Not Found" {
            anyhow::bail!(
                "No releases found for this project.\n\
                 Please check https://github.com/RyderFreeman4Logos/cli-sub-agent/releases for updates."
            );
        }
    }

    serde_json::from_str(&json)
        .context("Failed to parse release JSON. Is the repository accessible?")
}

/// Perform the actual update
fn perform_update(
    release_info: &ReleaseInfo,
    _current_version: &str,
    _latest_version: &str,
) -> Result<()> {
    // Determine current platform target
    let target = get_target_triple()?;
    eprintln!("Detected platform: {}", target);

    // Find matching asset
    let asset_name = format!("csa-{}.tar.gz", target);
    let asset = release_info
        .assets
        .iter()
        .find(|a| a.name == asset_name)
        .with_context(|| {
            format!(
                "No binary available for platform '{}'. Available assets: {:?}",
                target,
                release_info
                    .assets
                    .iter()
                    .map(|a| &a.name)
                    .collect::<Vec<_>>()
            )
        })?;

    eprintln!("Downloading: {}", asset.name);

    // Download to temp file
    let temp_dir = tempfile::tempdir().context("Failed to create temp directory")?;
    let archive_path = temp_dir.path().join(&asset.name);

    download_file(&asset.browser_download_url, &archive_path)?;

    // Extract archive
    eprintln!("Extracting...");
    extract_tarball(&archive_path, temp_dir.path())?;

    // Find the binary in extracted files
    let extracted_binary = temp_dir.path().join("csa");
    if !extracted_binary.exists() {
        anyhow::bail!("Binary 'csa' not found in archive");
    }

    // Get current executable path
    let current_exe = std::env::current_exe().context("Failed to get current executable path")?;

    // Replace current binary
    replace_binary(&extracted_binary, &current_exe)?;

    Ok(())
}

/// Download a file using curl
fn download_file(url: &str, dest: &Path) -> Result<()> {
    let output = Command::new("curl")
        .args(["-L", "-o", dest.to_str().unwrap(), url])
        .output()
        .context("Failed to execute curl")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to download file: {}", stderr);
    }

    Ok(())
}

/// Extract a tar.gz archive
fn extract_tarball(archive: &Path, dest_dir: &Path) -> Result<()> {
    let output = Command::new("tar")
        .args([
            "-xzf",
            archive.to_str().unwrap(),
            "-C",
            dest_dir.to_str().unwrap(),
        ])
        .output()
        .context("Failed to execute tar. Is tar installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to extract archive: {}", stderr);
    }

    Ok(())
}

/// Replace current binary with new one
fn replace_binary(new_binary: &Path, current_binary: &Path) -> Result<()> {
    // Backup current binary
    let backup_path = current_binary.with_extension("old");

    // On Unix, we can rename the current binary while it's running
    #[cfg(unix)]
    {
        // Remove old backup if exists
        if backup_path.exists() {
            fs::remove_file(&backup_path).ok();
        }

        // Rename current to .old
        fs::rename(current_binary, &backup_path).context("Failed to backup current binary")?;

        // Copy new binary to current location
        fs::copy(new_binary, current_binary).context("Failed to copy new binary")?;

        // Set executable permissions
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(current_binary)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(current_binary, perms)?;

        // Try to remove backup (best effort)
        fs::remove_file(&backup_path).ok();
    }

    #[cfg(not(unix))]
    {
        anyhow::bail!("Self-update is only supported on Unix-like systems");
    }

    Ok(())
}

/// Get the target triple for the current platform
fn get_target_triple() -> Result<String> {
    let arch = std::env::consts::ARCH;
    let os = std::env::consts::OS;

    let target = match (arch, os) {
        ("x86_64", "linux") => "x86_64-unknown-linux-gnu",
        ("aarch64", "linux") => "aarch64-unknown-linux-gnu",
        ("x86_64", "macos") => "x86_64-apple-darwin",
        ("aarch64", "macos") => "aarch64-apple-darwin",
        _ => anyhow::bail!(
            "Unsupported platform: {}-{}. Please install manually from GitHub releases.",
            arch,
            os
        ),
    };

    Ok(target.to_string())
}

/// GitHub Release API response structure
#[derive(serde::Deserialize)]
struct ReleaseInfo {
    tag_name: String,
    assets: Vec<ReleaseAsset>,
}

#[derive(serde::Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

/// GitHub API error response
#[derive(serde::Deserialize)]
struct GitHubError {
    message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- get_target_triple tests ---

    #[test]
    fn get_target_triple_returns_known_platform() {
        let result = get_target_triple();
        // On CI/dev machines this should succeed for supported platforms
        match result {
            Ok(triple) => {
                // Must contain os and arch components
                assert!(
                    triple.contains("linux") || triple.contains("darwin"),
                    "expected linux or darwin in: {}",
                    triple
                );
                assert!(
                    triple.contains("x86_64") || triple.contains("aarch64"),
                    "expected x86_64 or aarch64 in: {}",
                    triple
                );
            }
            Err(e) => {
                // Only acceptable if running on an unsupported platform
                assert!(
                    e.to_string().contains("Unsupported platform"),
                    "unexpected error: {}",
                    e
                );
            }
        }
    }

    // --- ReleaseInfo deserialization tests ---

    #[test]
    fn release_info_deserialize_minimal() {
        let json = r#"{
            "tag_name": "v0.5.0",
            "assets": []
        }"#;
        let info: ReleaseInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.tag_name, "v0.5.0");
        assert!(info.assets.is_empty());
    }

    #[test]
    fn release_info_deserialize_with_assets() {
        let json = r#"{
            "tag_name": "v1.0.0",
            "assets": [
                {
                    "name": "csa-x86_64-unknown-linux-gnu.tar.gz",
                    "browser_download_url": "https://example.com/download/csa.tar.gz"
                },
                {
                    "name": "csa-aarch64-apple-darwin.tar.gz",
                    "browser_download_url": "https://example.com/download/csa-mac.tar.gz"
                }
            ]
        }"#;
        let info: ReleaseInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.tag_name, "v1.0.0");
        assert_eq!(info.assets.len(), 2);
        assert_eq!(info.assets[0].name, "csa-x86_64-unknown-linux-gnu.tar.gz");
    }

    #[test]
    fn release_info_deserialize_extra_fields_ignored() {
        let json = r#"{
            "tag_name": "v2.0.0",
            "assets": [],
            "body": "Release notes here",
            "draft": false,
            "prerelease": false
        }"#;
        let info: ReleaseInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.tag_name, "v2.0.0");
    }

    // --- GitHubError deserialization tests ---

    #[test]
    fn github_error_deserialize() {
        let json = r#"{"message": "Not Found"}"#;
        let err: GitHubError = serde_json::from_str(json).unwrap();
        assert_eq!(err.message, "Not Found");
    }

    #[test]
    fn github_error_deserialize_with_extra_fields() {
        let json =
            r#"{"message": "rate limit exceeded", "documentation_url": "https://docs.github.com"}"#;
        let err: GitHubError = serde_json::from_str(json).unwrap();
        assert_eq!(err.message, "rate limit exceeded");
    }

    // --- Version string strip prefix test ---

    #[test]
    fn version_strip_prefix_v() {
        let tag = "v1.2.3";
        let version = tag.strip_prefix('v').unwrap_or(tag);
        assert_eq!(version, "1.2.3");
    }

    #[test]
    fn version_strip_prefix_no_v() {
        let tag = "1.2.3";
        let version = tag.strip_prefix('v').unwrap_or(tag);
        assert_eq!(version, "1.2.3");
    }

    // --- Asset matching test ---

    #[test]
    fn asset_name_format_matches_expected_pattern() {
        let target = "x86_64-unknown-linux-gnu";
        let expected_name = format!("csa-{}.tar.gz", target);
        assert_eq!(expected_name, "csa-x86_64-unknown-linux-gnu.tar.gz");
    }
}
