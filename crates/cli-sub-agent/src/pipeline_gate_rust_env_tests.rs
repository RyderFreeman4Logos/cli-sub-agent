use super::*;
use crate::test_env_lock::ScopedEnvVarRestore;
use serial_test::serial;
use std::collections::HashMap;

#[tokio::test]
#[serial]
async fn test_gate_command_normalizes_readonly_usr_local_rust_env() {
    let _env_lock = crate::test_env_lock::TEST_ENV_LOCK.lock().await;
    let _depth = ScopedEnvVarRestore::set("CSA_DEPTH", "0");
    let _target = ScopedEnvVarRestore::unset(csa_core::env::CARGO_TARGET_DIR_ENV_KEY);
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let capture = dir.path().join("gate-env.txt");
    let extra_env = HashMap::from([
        (
            "CAPTURE_ENV".to_string(),
            capture.to_string_lossy().into_owned(),
        ),
        ("HOME".to_string(), home.to_string_lossy().into_owned()),
        (
            csa_core::env::CARGO_HOME_ENV_KEY.to_string(),
            "/usr/local".to_string(),
        ),
        (
            csa_core::env::CARGO_INSTALL_ROOT_ENV_KEY.to_string(),
            "/usr/local".to_string(),
        ),
    ]);

    let result = evaluate_quality_gate(
        dir.path(),
        Some(
            "printf '%s\\n%s\\n%s\\n' \
             \"$CARGO_HOME\" \"$CARGO_INSTALL_ROOT\" \"$CARGO_TARGET_DIR\" > \"$CAPTURE_ENV\"",
        ),
        250,
        &GateMode::Full,
        0,
        Some(&extra_env),
    )
    .await
    .unwrap();

    assert!(result.passed());
    let captured = std::fs::read_to_string(&capture).unwrap();
    let lines = captured.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 3);
    assert_ne!(lines[0], "/usr/local");
    assert!(
        !csa_core::env::rust_state_path_needs_session_override(std::path::Path::new(lines[0])),
        "gate CARGO_HOME must not target read-only /usr/local: {}",
        lines[0]
    );
    assert_eq!(
        lines[1],
        dir.path()
            .join("target/cargo-install-root")
            .to_str()
            .unwrap()
    );
    assert_eq!(lines[2], dir.path().join("target").to_str().unwrap());
}

#[tokio::test]
#[serial]
async fn test_gate_command_preserves_safe_ambient_cargo_paths() {
    let _env_lock = crate::test_env_lock::TEST_ENV_LOCK.lock().await;
    let _depth = ScopedEnvVarRestore::set("CSA_DEPTH", "0");
    let dir = tempfile::tempdir().unwrap();
    let capture = dir.path().join("gate-env.txt");
    let ambient_target = dir.path().join("ambient-target");
    let ambient_install_root = dir.path().join("ambient-cargo-install-root");
    let _target =
        ScopedEnvVarRestore::set(csa_core::env::CARGO_TARGET_DIR_ENV_KEY, &ambient_target);
    let _install_root = ScopedEnvVarRestore::set(
        csa_core::env::CARGO_INSTALL_ROOT_ENV_KEY,
        &ambient_install_root,
    );
    let extra_env = HashMap::from([(
        "CAPTURE_ENV".to_string(),
        capture.to_string_lossy().into_owned(),
    )]);

    let result = evaluate_quality_gate(
        dir.path(),
        Some(
            "printf '%s\\n%s\\n' \
             \"$CARGO_TARGET_DIR\" \"$CARGO_INSTALL_ROOT\" > \"$CAPTURE_ENV\"",
        ),
        250,
        &GateMode::Full,
        0,
        Some(&extra_env),
    )
    .await
    .unwrap();

    assert!(result.passed());
    let captured = std::fs::read_to_string(&capture).unwrap();
    let lines = captured.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0], ambient_target.to_str().unwrap());
    assert_eq!(lines[1], ambient_install_root.to_str().unwrap());
}

#[tokio::test]
#[serial]
async fn test_gate_command_normalizes_ambient_usr_local_cargo_paths() {
    let _env_lock = crate::test_env_lock::TEST_ENV_LOCK.lock().await;
    let _depth = ScopedEnvVarRestore::set("CSA_DEPTH", "0");
    let dir = tempfile::tempdir().unwrap();
    let capture = dir.path().join("gate-env.txt");
    let _target = ScopedEnvVarRestore::set(csa_core::env::CARGO_TARGET_DIR_ENV_KEY, "/usr/local");
    let _install_root =
        ScopedEnvVarRestore::set(csa_core::env::CARGO_INSTALL_ROOT_ENV_KEY, "/usr/local");
    let extra_env = HashMap::from([(
        "CAPTURE_ENV".to_string(),
        capture.to_string_lossy().into_owned(),
    )]);

    let result = evaluate_quality_gate(
        dir.path(),
        Some(
            "printf '%s\\n%s\\n' \
             \"$CARGO_TARGET_DIR\" \"$CARGO_INSTALL_ROOT\" > \"$CAPTURE_ENV\"",
        ),
        250,
        &GateMode::Full,
        0,
        Some(&extra_env),
    )
    .await
    .unwrap();

    assert!(result.passed());
    let captured = std::fs::read_to_string(&capture).unwrap();
    let lines = captured.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0], dir.path().join("target").to_str().unwrap());
    assert_eq!(
        lines[1],
        dir.path()
            .join("target/cargo-install-root")
            .to_str()
            .unwrap()
    );
}
