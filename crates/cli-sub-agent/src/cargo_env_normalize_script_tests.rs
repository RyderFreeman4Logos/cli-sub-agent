use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

#[cfg(unix)]
fn install_executable(path: &Path, contents: &str) {
    use std::os::unix::fs::PermissionsExt;

    fs::create_dir_all(path.parent().expect("executable parent")).expect("create fake bin dir");
    fs::write(path, contents).unwrap_or_else(|err| panic!("write fake executable: {err}"));
    let mut permissions = fs::metadata(path)
        .unwrap_or_else(|err| panic!("fake executable metadata: {err}"))
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)
        .unwrap_or_else(|err| panic!("chmod fake executable: {err}"));
}

#[cfg(unix)]
fn install_capture_cargo(path: &Path) {
    install_executable(
        path,
        r#"#!/usr/bin/env bash
set -euo pipefail
{
    printf 'CARGO_BINARY=%s\n' "$0"
    printf 'CARGO_HOME=%s\n' "${CARGO_HOME:-}"
    printf 'CARGO_INSTALL_ROOT=%s\n' "${CARGO_INSTALL_ROOT:-}"
    printf 'CARGO_TARGET_DIR=%s\n' "${CARGO_TARGET_DIR:-}"
    printf 'RUSTUP_HOME=%s\n' "${RUSTUP_HOME:-}"
} >"${CSA_CAPTURE_ENV:?}"
"#,
    );
}

#[cfg(unix)]
fn read_capture(path: &Path) -> HashMap<String, String> {
    fs::read_to_string(path)
        .expect("fake cargo should capture env")
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            Some((key.to_string(), value.to_string()))
        })
        .collect()
}

#[test]
#[cfg(unix)]
fn cargo_env_normalize_replaces_readonly_usr_local_rust_state() {
    let repo = TempDir::new().expect("create temp repo");
    let repo_root = repo.path().canonicalize().expect("canonical temp repo");
    let capture = repo.path().join("capture.env");
    let mise_data = repo.path().join("mise-data");
    let mise_rust = mise_data.join("installs/rust/stable");
    let toolchain_bin = mise_rust.join("toolchains/stable-x86_64-unknown-linux-gnu/bin");
    let fake_cargo = toolchain_bin.join("cargo");
    fs::create_dir_all(&toolchain_bin).expect("create fake toolchain bin");
    fs::write(mise_rust.join("settings.toml"), "version = \"12\"\n")
        .expect("write fake rustup settings");
    fs::write(
        repo.path().join("rust-toolchain.toml"),
        "[toolchain]\nchannel = \"stable\"\n",
    )
    .expect("write rust-toolchain.toml");
    install_capture_cargo(&fake_cargo);

    let output = Command::new("bash")
        .arg(workspace_root().join("scripts/cargo-env-normalize.sh"))
        .arg("cargo")
        .arg("metadata")
        .current_dir(repo.path())
        .env("CSA_CAPTURE_ENV", &capture)
        .env("MISE_DATA_DIR", &mise_data)
        .env("CARGO_HOME", "/usr/local")
        .env("CARGO_INSTALL_ROOT", "/usr/local")
        .env("RUSTUP_HOME", "/usr/local")
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .expect("normalizer should execute");

    assert!(
        output.status.success(),
        "normalizer failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let captured = read_capture(&capture);
    let cargo_home = PathBuf::from(captured.get("CARGO_HOME").expect("CARGO_HOME captured"));
    assert!(
        !csa_core::env::rust_state_path_needs_session_override(&cargo_home),
        "CARGO_HOME should not target read-only /usr/local: {}",
        cargo_home.display()
    );
    assert_eq!(
        captured.get("CARGO_INSTALL_ROOT").map(String::as_str),
        Some(
            repo_root
                .join("target/cargo-install-root")
                .to_str()
                .unwrap()
        )
    );
    assert_eq!(
        captured.get("CARGO_TARGET_DIR").map(String::as_str),
        Some(repo_root.join("target").to_str().unwrap())
    );
    assert_eq!(
        captured.get("RUSTUP_HOME").map(String::as_str),
        Some(mise_rust.to_str().unwrap())
    );
    assert_eq!(
        captured.get("CARGO_BINARY").map(String::as_str),
        Some(fake_cargo.to_str().unwrap())
    );
}

#[test]
#[cfg(unix)]
fn cargo_env_normalize_preserves_explicit_writable_rust_state() {
    let repo = TempDir::new().expect("create temp repo");
    let capture = repo.path().join("capture.env");
    let fake_bin = repo.path().join("fake-bin");
    let fake_cargo = fake_bin.join("cargo");
    let cargo_home = repo.path().join("explicit-cargo-home");
    let cargo_install_root = repo.path().join("explicit-cargo-install-root");
    let cargo_target_dir = repo.path().join("explicit-target");
    let rustup_home = repo.path().join("explicit-rustup-home");
    for dir in [
        &cargo_home,
        &cargo_install_root,
        &cargo_target_dir,
        &rustup_home,
    ] {
        fs::create_dir_all(dir).expect("create explicit env dir");
    }
    install_capture_cargo(&fake_cargo);
    let path = format!(
        "{}:{}",
        fake_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = Command::new("bash")
        .arg(workspace_root().join("scripts/cargo-env-normalize.sh"))
        .arg("cargo")
        .arg("metadata")
        .current_dir(repo.path())
        .env("PATH", path)
        .env("CSA_CAPTURE_ENV", &capture)
        .env("CARGO_HOME", &cargo_home)
        .env("CARGO_INSTALL_ROOT", &cargo_install_root)
        .env("CARGO_TARGET_DIR", &cargo_target_dir)
        .env("RUSTUP_HOME", &rustup_home)
        .output()
        .expect("normalizer should execute");

    assert!(
        output.status.success(),
        "normalizer failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let captured = read_capture(&capture);
    assert_eq!(
        captured.get("CARGO_HOME").map(String::as_str),
        Some(cargo_home.to_str().unwrap())
    );
    assert_eq!(
        captured.get("CARGO_INSTALL_ROOT").map(String::as_str),
        Some(cargo_install_root.to_str().unwrap())
    );
    assert_eq!(
        captured.get("CARGO_TARGET_DIR").map(String::as_str),
        Some(cargo_target_dir.to_str().unwrap())
    );
    assert_eq!(
        captured.get("RUSTUP_HOME").map(String::as_str),
        Some(rustup_home.to_str().unwrap())
    );
    assert_eq!(
        captured.get("CARGO_BINARY").map(String::as_str),
        Some(fake_cargo.to_str().unwrap())
    );
}
