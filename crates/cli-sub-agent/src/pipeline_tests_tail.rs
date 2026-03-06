use super::*;

/// When config has no tiers, both enforce_tier values must behave identically:
/// no tier-related errors regardless of the flag.
#[tokio::test]
async fn build_and_validate_executor_no_tiers_both_flags_equivalent() {
    let cfg = ProjectConfig {
        schema_version: CURRENT_SCHEMA_VERSION,
        project: ProjectMeta::default(),
        resources: ResourcesConfig::default(),
        acp: Default::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(), // no tiers
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        preferences: None,
        session: Default::default(),
        memory: Default::default(),
    };

    let result_true = build_and_validate_executor(
        &ToolName::Codex,
        Some("codex/openai/gpt-4o/low"),
        None,
        None,
        ConfigRefs {
            project: Some(&cfg),
            global: None,
        },
        true,
        false,
    )
    .await;

    let result_false = build_and_validate_executor(
        &ToolName::Codex,
        Some("codex/openai/gpt-4o/low"),
        None,
        None,
        ConfigRefs {
            project: Some(&cfg),
            global: None,
        },
        false,
        false,
    )
    .await;

    // Neither should fail with tier errors (tiers are empty).
    // Both should produce the same outcome (success or same non-tier error).
    for (label, result) in [("true", &result_true), ("false", &result_false)] {
        if let Err(e) = result {
            let msg = e.to_string();
            assert!(
                !msg.contains("not configured in any tier") && !msg.contains("belongs to tool"),
                "enforce_tier={label} with no tiers must not produce tier error, got: {msg}"
            );
        }
    }
    // Both must have the same Ok/Err status (empty tiers = no behavioral difference)
    assert_eq!(
        result_true.is_ok(),
        result_false.is_ok(),
        "enforce_tier=true and false must behave identically with empty tiers"
    );
}

#[test]
fn result_toml_path_contract_not_applied_without_prompt_marker() {
    let temp = tempfile::tempdir().unwrap();
    let mut result = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: "/tmp/missing/result.toml".to_string(),
        exit_code: 0,
    };

    enforce_result_toml_contract_now("normal prompt", "normal prompt", temp.path(), &mut result);

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.summary, "/tmp/missing/result.toml");
    assert!(result.stderr_output.is_empty());
}

#[test]
fn result_toml_path_contract_not_applied_when_marker_only_in_effective_prompt() {
    let temp = tempfile::tempdir().unwrap();
    let mut result = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: "/tmp/missing/result.toml".to_string(),
        exit_code: 0,
    };

    enforce_result_toml_contract_now(
        "normal prompt",
        "CSA_RESULT_TOML_PATH_CONTRACT=1",
        temp.path(),
        &mut result,
    );

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.summary, "/tmp/missing/result.toml");
    assert!(result.stderr_output.is_empty());
}

#[test]
fn result_toml_path_contract_fails_closed_when_preclear_failed() {
    let temp = tempfile::tempdir().unwrap();
    let result_path = temp.path().join("result.toml");
    fs::write(&result_path, "status = \"success\"\n").unwrap();
    let mut result = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: result_path.display().to_string(),
        exit_code: 0,
    };

    enforce_result_toml_path_contract(
        "CSA_RESULT_TOML_PATH_CONTRACT=1",
        "",
        temp.path(),
        false,
        &mut result,
    );

    assert_eq!(result.exit_code, 1);
    assert!(
        result
            .summary
            .contains("failed to clear pre-existing result.toml")
    );
    assert!(result.stderr_output.contains("contract violation"));
}

#[test]
fn result_toml_path_contract_accepts_existing_absolute_result_file() {
    let temp = tempfile::tempdir().unwrap();
    let result_path = temp.path().join("result.toml");
    fs::write(&result_path, "status = \"success\"\n").unwrap();

    let mut result = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: result_path.display().to_string(),
        exit_code: 0,
    };

    enforce_result_toml_contract_now(
        "CSA_RESULT_TOML_PATH_CONTRACT=1",
        "",
        temp.path(),
        &mut result,
    );

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.summary, result_path.display().to_string());
    assert!(result.stderr_output.is_empty());
}

#[cfg(unix)]
#[test]
fn result_toml_path_contract_rejects_hardlinked_session_result_file() {
    let session_dir = tempfile::tempdir().unwrap();
    let external_dir = tempfile::tempdir().unwrap();
    let external_result = external_dir.path().join("result.toml");
    fs::write(&external_result, "status = \"success\"\n").unwrap();

    let session_result = session_dir.path().join("result.toml");
    fs::hard_link(&external_result, &session_result).unwrap();
    let mut result = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: session_result.display().to_string(),
        exit_code: 0,
    };

    enforce_result_toml_contract_now(
        "CSA_RESULT_TOML_PATH_CONTRACT=1",
        "",
        session_dir.path(),
        &mut result,
    );

    assert_eq!(result.exit_code, 1);
    assert!(result.summary.contains("contract violation"));
    assert!(result.stderr_output.contains("contract violation"));
}

#[test]
fn result_toml_path_contract_coerces_missing_file_to_failure() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing").join("result.toml");
    let mut result = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: missing.display().to_string(),
        exit_code: 0,
    };

    enforce_result_toml_contract_now(
        "CSA_RESULT_TOML_PATH_CONTRACT=1",
        "",
        temp.path(),
        &mut result,
    );

    assert_eq!(result.exit_code, 1);
    assert!(result.summary.contains("contract violation"));
    assert!(result.summary.contains("result.toml"));
    assert!(result.stderr_output.contains("contract violation"));
}

#[test]
fn result_toml_path_contract_uses_untruncated_output_path_over_summary() {
    let temp = tempfile::tempdir().unwrap();
    let result_path = temp.path().join("result.toml");
    fs::write(&result_path, "status = \"success\"\n").unwrap();

    let full_path = result_path.display().to_string();
    let truncated_summary = format!("{}...", &full_path[..full_path.len().saturating_sub(6)]);
    let mut result = ExecutionResult {
        output: format!("manager result\n{full_path}\n"),
        stderr_output: String::new(),
        summary: truncated_summary,
        exit_code: 0,
    };

    enforce_result_toml_contract_now(
        "CSA_RESULT_TOML_PATH_CONTRACT=1",
        "",
        temp.path(),
        &mut result,
    );

    assert_eq!(result.exit_code, 0);
    assert!(result.stderr_output.is_empty());
}

#[test]
fn result_toml_path_contract_rejects_existing_result_file_outside_session_dir() {
    let session_dir = tempfile::tempdir().unwrap();
    let external_dir = tempfile::tempdir().unwrap();
    fs::write(
        session_dir.path().join("result.toml"),
        "status = \"success\"\n",
    )
    .unwrap();
    let external_result_path = external_dir.path().join("result.toml");
    fs::write(&external_result_path, "status = \"success\"\n").unwrap();

    let mut result = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: external_result_path.display().to_string(),
        exit_code: 0,
    };

    enforce_result_toml_contract_now(
        "CSA_RESULT_TOML_PATH_CONTRACT=1",
        "",
        session_dir.path(),
        &mut result,
    );

    assert_eq!(result.exit_code, 1);
    assert!(result.summary.contains("contract violation"));
    assert!(result.stderr_output.contains("contract violation"));
}

#[test]
fn result_toml_path_contract_fails_when_output_and_summary_are_empty() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("result.toml"), "status = \"success\"\n").unwrap();
    let mut result = ExecutionResult {
        output: " \n\t\n".to_string(),
        stderr_output: String::new(),
        summary: String::new(),
        exit_code: 0,
    };

    enforce_result_toml_contract_now(
        "CSA_RESULT_TOML_PATH_CONTRACT=1",
        "",
        temp.path(),
        &mut result,
    );

    assert_eq!(result.exit_code, 1);
    assert!(result.summary.contains("output and summary were empty"));
    assert!(result.stderr_output.contains("contract violation"));
}

#[test]
fn result_toml_path_contract_accepts_quoted_output_path_with_trailing_markers() {
    let temp = tempfile::tempdir().unwrap();
    let result_path = temp.path().join("result.toml");
    fs::write(&result_path, "status = \"success\"\n").unwrap();
    let quoted_path = format!("`{}`", result_path.display());
    let mut result = ExecutionResult {
        output: format!(
            "<!-- CSA:SECTION:summary -->\n{quoted_path}\n<!-- CSA:SECTION:summary:END -->\n"
        ),
        stderr_output: String::new(),
        summary: "<!-- CSA:SECTION:details:END -->".to_string(),
        exit_code: 0,
    };

    enforce_result_toml_contract_now(
        "CSA_RESULT_TOML_PATH_CONTRACT=1",
        "",
        temp.path(),
        &mut result,
    );

    assert_eq!(result.exit_code, 0);
    assert!(result.stderr_output.is_empty());
}

#[test]
fn result_toml_path_contract_prefers_summary_path_when_output_has_no_path() {
    let temp = tempfile::tempdir().unwrap();
    let result_path = temp.path().join("result.toml");
    fs::write(&result_path, "status = \"success\"\n").unwrap();
    let mut result = ExecutionResult {
        output:
            "<!-- CSA:SECTION:summary -->\nNo path in output\n<!-- CSA:SECTION:summary:END -->\n"
                .to_string(),
        stderr_output: String::new(),
        summary: result_path.display().to_string(),
        exit_code: 0,
    };

    enforce_result_toml_contract_now(
        "CSA_RESULT_TOML_PATH_CONTRACT=1",
        "",
        temp.path(),
        &mut result,
    );

    assert_eq!(result.exit_code, 0);
    assert!(result.stderr_output.is_empty());
}

#[test]
fn result_toml_path_contract_accepts_user_result_artifact_path() {
    let temp = tempfile::tempdir().unwrap();
    let user_result_path = temp.path().join("output").join("user-result.toml");
    fs::create_dir_all(user_result_path.parent().unwrap()).unwrap();
    fs::write(
        &user_result_path,
        r#"status = "success"
summary = "ok"
"#,
    )
    .unwrap();

    let mut result = ExecutionResult {
        output: format!("{}\n", user_result_path.display()),
        stderr_output: String::new(),
        summary: "ok".to_string(),
        exit_code: 0,
    };

    enforce_result_toml_contract_now(
        "CSA_RESULT_TOML_PATH_CONTRACT=1",
        "",
        temp.path(),
        &mut result,
    );

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.summary, "ok");
    assert!(result.stderr_output.is_empty());
}

#[test]
fn result_toml_path_contract_accepts_verified_user_result_fallback_on_output_mismatch() {
    let temp = tempfile::tempdir().unwrap();
    let user_result_path = temp.path().join("output").join("user-result.toml");
    fs::create_dir_all(user_result_path.parent().unwrap()).unwrap();
    fs::write(
        &user_result_path,
        r#"status = "success"
summary = "ok"
"#,
    )
    .unwrap();

    let mut result = ExecutionResult {
        output: "progress line before path\n".to_string(),
        stderr_output: String::new(),
        summary: "not-a-path".to_string(),
        exit_code: 0,
    };

    enforce_result_toml_contract_now(
        "CSA_RESULT_TOML_PATH_CONTRACT=1",
        "",
        temp.path(),
        &mut result,
    );

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.summary, "not-a-path");
    assert!(
        result
            .stderr_output
            .contains("contract warning: output/summary path mismatch")
    );
}

#[test]
fn result_toml_path_contract_rejects_invalid_user_result_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let user_result_path = temp.path().join("output").join("user-result.toml");
    fs::create_dir_all(user_result_path.parent().unwrap()).unwrap();
    fs::write(&user_result_path, "not valid toml {{{{").unwrap();

    let mut result = ExecutionResult {
        output: "progress only\n".to_string(),
        stderr_output: String::new(),
        summary: "still-not-a-path".to_string(),
        exit_code: 0,
    };

    enforce_result_toml_contract_now(
        "CSA_RESULT_TOML_PATH_CONTRACT=1",
        "",
        temp.path(),
        &mut result,
    );

    assert_eq!(result.exit_code, 1);
    assert!(result.summary.contains("contract violation"));
}

#[test]
fn result_toml_path_contract_ignores_embedded_marker_substring() {
    let temp = tempfile::tempdir().unwrap();
    let mut result = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: "/tmp/missing/result.toml".to_string(),
        exit_code: 0,
    };

    enforce_result_toml_contract_now(
        "memory says contract marker: csa_result_toml_path_contract=1 in a paragraph",
        "",
        temp.path(),
        &mut result,
    );

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.summary, "/tmp/missing/result.toml");
    assert!(result.stderr_output.is_empty());
}

#[test]
fn result_toml_path_contract_applies_with_markdown_marker_list_item() {
    let temp = tempfile::tempdir().unwrap();
    let mut result = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: "/tmp/missing/result.toml".to_string(),
        exit_code: 0,
    };

    enforce_result_toml_contract_now(
        "- CONTRACT MARKER: CSA_RESULT_TOML_PATH_CONTRACT=1",
        "",
        temp.path(),
        &mut result,
    );

    assert_eq!(result.exit_code, 1);
    assert!(result.summary.contains("contract violation"));
    assert!(result.stderr_output.contains("contract violation"));
}

#[cfg(unix)]
#[test]
fn result_toml_path_contract_rejects_symlinked_session_result_file() {
    use std::os::unix::fs::symlink;

    let session_dir = tempfile::tempdir().unwrap();
    let external_dir = tempfile::tempdir().unwrap();
    let external_result = external_dir.path().join("result.toml");
    fs::write(&external_result, "status = \"success\"\n").unwrap();

    let session_result = session_dir.path().join("result.toml");
    symlink(&external_result, &session_result).unwrap();
    let mut result = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: session_result.display().to_string(),
        exit_code: 0,
    };

    enforce_result_toml_contract_now(
        "CSA_RESULT_TOML_PATH_CONTRACT=1",
        "",
        session_dir.path(),
        &mut result,
    );

    assert_eq!(result.exit_code, 1);
    assert!(result.summary.contains("contract violation"));
    assert!(result.stderr_output.contains("contract violation"));
}

#[test]
fn execute_with_session_and_meta_contract_rejects_illegal_result_toml_path() {
    let session_dir = tempfile::tempdir().unwrap();
    fs::write(
        session_dir.path().join("result.toml"),
        "status = \"success\"\n",
    )
    .unwrap();
    let foreign_dir = tempfile::tempdir().unwrap();
    let foreign_result = foreign_dir.path().join("result.toml");
    fs::write(&foreign_result, "status = \"success\"\n").unwrap();
    let mut result = ExecutionResult {
        output: foreign_result.display().to_string(),
        stderr_output: String::new(),
        summary: "ok".to_string(),
        exit_code: 0,
    };

    enforce_result_toml_contract_now(
        "CSA_RESULT_TOML_PATH_CONTRACT=1",
        "",
        session_dir.path(),
        &mut result,
    );

    assert_eq!(result.exit_code, 1);
    assert!(result.summary.contains("contract violation"));
    assert!(result.stderr_output.contains("contract violation"));
}

#[test]
fn execute_with_session_and_meta_contract_preserves_existing_failure_exit_code() {
    let session_dir = tempfile::tempdir().unwrap();
    let original_summary = "upstream transport failure".to_string();
    let original_stderr = "executor crashed".to_string();
    let mut result = ExecutionResult {
        output: "/tmp/attacker/result.toml".to_string(),
        stderr_output: original_stderr.clone(),
        summary: original_summary.clone(),
        exit_code: 7,
    };

    enforce_result_toml_contract_now(
        "CSA_RESULT_TOML_PATH_CONTRACT=1",
        "",
        session_dir.path(),
        &mut result,
    );

    assert_eq!(result.exit_code, 7);
    assert_eq!(result.summary, original_summary);
    assert_eq!(result.stderr_output, original_stderr);
}

#[test]
fn result_toml_path_contract_not_applied_when_exit_code_nonzero_even_with_marker() {
    let temp = tempfile::tempdir().unwrap();
    let original_summary = "tool failed before contract output".to_string();
    let original_stderr = "network timeout".to_string();
    let mut result = ExecutionResult {
        output: "/tmp/forged/result.toml".to_string(),
        stderr_output: original_stderr.clone(),
        summary: original_summary.clone(),
        exit_code: 2,
    };

    enforce_result_toml_contract_now(
        "CSA_RESULT_TOML_PATH_CONTRACT=1",
        "",
        temp.path(),
        &mut result,
    );

    assert_eq!(result.exit_code, 2);
    assert_eq!(result.summary, original_summary);
    assert_eq!(result.stderr_output, original_stderr);
}

#[cfg(unix)]
#[tokio::test]
async fn execute_with_session_and_meta_rejects_illegal_result_path_in_real_flow() {
    use std::os::unix::fs::PermissionsExt;

    let _env_lock = PIPELINE_ENV_LOCK
        .lock()
        .expect("pipeline env lock poisoned");
    let _csa_session_id_guard = ScopedEnvVarRestore::unset("CSA_SESSION_ID");

    let temp = tempfile::tempdir().unwrap();
    let project_root = temp.path();
    let bin_dir = project_root.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let fake_gemini = bin_dir.join("gemini");
    fs::write(
        &fake_gemini,
        "#!/bin/sh\nprintf '%s\\n' \"$CSA_FAKE_OUTPUT_PATH\"\n",
    )
    .unwrap();
    let mut perms = fs::metadata(&fake_gemini).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&fake_gemini, perms).unwrap();

    let foreign_dir = tempfile::tempdir().unwrap();
    let foreign_result = foreign_dir.path().join("result.toml");
    fs::write(&foreign_result, "status = \"success\"\n").unwrap();

    let mut extra_env = HashMap::new();
    let inherited_path = std::env::var("PATH").unwrap_or_default();
    extra_env.insert(
        "PATH".to_string(),
        format!("{}:{inherited_path}", bin_dir.display()),
    );
    extra_env.insert(
        "CSA_FAKE_OUTPUT_PATH".to_string(),
        foreign_result.display().to_string(),
    );

    let executor = Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    };
    let execution = execute_with_session_and_meta(
        &executor,
        &ToolName::GeminiCli,
        "CSA_RESULT_TOML_PATH_CONTRACT=1",
        csa_core::types::OutputFormat::Json,
        None,
        Some("contract-e2e".to_string()),
        None,
        project_root,
        None,
        Some(&extra_env),
        None,
        None,
        None,
        csa_process::StreamMode::BufferOnly,
        DEFAULT_IDLE_TIMEOUT_SECONDS,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(execution.execution.exit_code, 1);
    assert!(execution.execution.summary.contains("contract violation"));
    assert!(
        execution
            .execution
            .stderr_output
            .contains("contract violation")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn execute_with_session_and_meta_explicit_only_ignores_inherited_parent_session() {
    use std::os::unix::fs::PermissionsExt;

    let _env_lock = PIPELINE_ENV_LOCK
        .lock()
        .expect("pipeline env lock poisoned");
    let _csa_session_id_guard =
        ScopedEnvVarRestore::set("CSA_SESSION_ID", "01K00000000000000000000000");

    let temp = tempfile::tempdir().unwrap();
    let project_root = temp.path();
    let bin_dir = project_root.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let fake_gemini = bin_dir.join("gemini");
    fs::write(&fake_gemini, "#!/bin/sh\nprintf 'review-ok\\n'\n").unwrap();
    let mut perms = fs::metadata(&fake_gemini).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&fake_gemini, perms).unwrap();

    let mut extra_env = HashMap::new();
    let inherited_path = std::env::var("PATH").unwrap_or_default();
    extra_env.insert(
        "PATH".to_string(),
        format!("{}:{inherited_path}", bin_dir.display()),
    );

    let executor = Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    };

    let execution = execute_with_session_and_meta_with_parent_source(
        &executor,
        &ToolName::GeminiCli,
        "review prompt",
        csa_core::types::OutputFormat::Json,
        None,
        Some("review-session".to_string()),
        None,
        project_root,
        None,
        Some(&extra_env),
        Some("review"),
        None,
        None,
        csa_process::StreamMode::BufferOnly,
        DEFAULT_IDLE_TIMEOUT_SECONDS,
        None,
        None,
        None,
        ParentSessionSource::ExplicitOnly,
    )
    .await
    .unwrap();

    assert_eq!(execution.execution.exit_code, 0);

    let saved_session =
        csa_session::load_session(project_root, &execution.meta_session_id).unwrap();
    assert!(
        saved_session.genealogy.parent_session_id.is_none(),
        "explicit-only mode must not inherit CSA_SESSION_ID"
    );
}
