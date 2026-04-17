#[test]
fn test_classify_gemini_acp_init_failure_detects_oom() {
    let env = HashMap::new();
    let classification = classify_gemini_acp_init_failure(
        "sandboxed ACP: ACP initialization failed: killed by signal 9 (SIGKILL)",
        &env,
    );
    assert_eq!(classification.code, "gemini_acp_init_oom");
}

#[test]
fn test_classify_gemini_acp_init_failure_detects_auth_env() {
    let original_api_key = std::env::var(csa_core::gemini::API_KEY_ENV).ok();
    let original_base_url = std::env::var(csa_core::gemini::BASE_URL_ENV).ok();
    unsafe {
        std::env::set_var(csa_core::gemini::API_KEY_ENV, "test-key");
        std::env::set_var(csa_core::gemini::BASE_URL_ENV, "http://127.0.0.1:8317");
    }

    let env = HashMap::new();
    let classification = classify_gemini_acp_init_failure(
        "ACP initialization failed: authentication failed: missing credentials",
        &env,
    );

    match original_api_key {
        Some(value) => unsafe { std::env::set_var(csa_core::gemini::API_KEY_ENV, value) },
        None => unsafe { std::env::remove_var(csa_core::gemini::API_KEY_ENV) },
    }
    match original_base_url {
        Some(value) => unsafe { std::env::set_var(csa_core::gemini::BASE_URL_ENV, value) },
        None => unsafe { std::env::remove_var(csa_core::gemini::BASE_URL_ENV) },
    }

    assert_eq!(classification.code, "gemini_acp_init_auth_env");
    assert_eq!(
        classification.missing_env_vars,
        vec![
            csa_core::gemini::API_KEY_ENV,
            csa_core::gemini::BASE_URL_ENV
        ]
    );
}

#[test]
fn test_classify_gemini_acp_init_failure_detects_mcp_extension() {
    let env = HashMap::new();
    let classification = classify_gemini_acp_init_failure(
        "ACP initialization failed: spawn /home/obj/.gemini/extensions/gemini-cli-security/mcp-server ENOENT",
        &env,
    );
    assert_eq!(classification.code, "gemini_acp_init_mcp_extension");
}

#[test]
fn test_classify_gemini_acp_init_failure_defaults_to_handshake_timeout() {
    let env = HashMap::new();
    let classification = classify_gemini_acp_init_failure(
        "ACP initialization failed: Internal error: \"server shut down unexpectedly\"",
        &env,
    );
    assert_eq!(classification.code, "gemini_acp_init_handshake_timeout");
}

#[tokio::test]
async fn test_execute_in_classifies_pre_handshake_gemini_failure_with_child_stderr() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("tempdir");
    let script_path = temp.path().join("gemini");
    std::fs::write(
        &script_path,
        r#"#!/usr/bin/env bash
set -euo pipefail
echo "spawn /home/obj/.gemini/extensions/gemini-cli-security/mcp-server ENOENT" >&2
exit 1
"#,
    )
    .expect("write fake gemini");
    let mut perms = std::fs::metadata(&script_path)
        .expect("metadata")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms).expect("chmod +x");

    let old_path = std::env::var("PATH").unwrap_or_default();
    let env = HashMap::from([(
        "PATH".to_string(),
        format!("{}:{old_path}", temp.path().display()),
    )]);

    let transport = AcpTransport::new("gemini-cli", None);
    let error = transport
        .execute_in(
            "test pre-handshake failure",
            temp.path(),
            Some(&env),
            StreamMode::BufferOnly,
            30,
        )
        .await
        .expect_err("fake gemini should fail before ACP handshake");

    let error_text = format!("{error:#}");
    assert!(
        error_text.contains("gemini_acp_init_mcp_extension"),
        "expected classified init failure, got: {error_text}"
    );
    assert!(
        error_text.contains("gemini-cli-security/mcp-server ENOENT"),
        "expected child stderr in error text, got: {error_text}"
    );
}
