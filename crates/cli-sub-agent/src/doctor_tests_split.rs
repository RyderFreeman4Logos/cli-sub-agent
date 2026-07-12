/// Doctor must surface transport validation errors with the offending key path.
/// Uses opencode + ACP (still rejected post-#1128) because the original
/// codex+cli rejection became obsolete after the codex CLI default flip.
#[test]
fn doctor_load_rejects_invalid_tool_transport_override() {
    let td = tempfile::tempdir().expect("tempdir");
    write_project_config(
        td.path(),
        r#"
[tools.opencode]
transport = "acp"
"#,
    );

    let err = load_doctor_project_config_from(td.path()).unwrap_err();
    let message = format!("{err:#}");

    assert!(
        message.contains("tools.opencode.transport"),
        "doctor should surface the exact config key: {message}"
    );
    assert!(
        message.contains("does not support ACP transport"),
        "doctor should surface the transport contract message: {message}"
    );
}

#[tokio::test]
async fn doctor_text_reports_invalid_codex_transport_without_aborting() {
    let td = tempfile::tempdir().expect("tempdir");
    write_project_config(
        td.path(),
        r#"
[tools.codex]
transport = "stdio"
"#,
    );

    let status = inspect_doctor_project_config_from(td.path());
    let rendered = render_project_config_lines(&status).join("\n");

    assert!(
        matches!(status, DoctorProjectConfigStatus::Invalid(_)),
        "doctor should classify invalid project config without aborting: {rendered}"
    );

    run_doctor_text_from(td.path())
        .await
        .expect("doctor text should keep running when project config is invalid");

    assert!(
        rendered.contains("Config:      .csa/config.toml (invalid)"),
        "doctor text should use the existing invalid config branch: {rendered}"
    );
    assert!(
        rendered.contains("Invalid tools.codex.transport"),
        "doctor text should surface the exact invalid transport key: {rendered}"
    );
    assert!(
        rendered.contains("unknown transport \"stdio\""),
        "doctor text should surface the invalid transport value: {rendered}"
    );
}

#[test]
fn doctor_json_reports_invalid_codex_transport() {
    let td = tempfile::tempdir().expect("tempdir");
    write_project_config(
        td.path(),
        r#"
[tools.codex]
transport = "stdio"
"#,
    );

    let report = build_doctor_json(td.path());
    let config = &report["config"];
    let error = config["error"]
        .as_str()
        .expect("doctor JSON should include invalid config error text");

    assert_eq!(config["found"], serde_json::json!(true));
    assert_eq!(config["valid"], serde_json::json!(false));
    assert!(
        error.contains("Invalid tools.codex.transport"),
        "doctor JSON should surface the exact invalid transport key: {error}"
    );
    assert!(
        error.contains("unknown transport \"stdio\""),
        "doctor JSON should surface the invalid transport value: {error}"
    );
}

#[test]
fn doctor_project_config_display_ignores_invalid_user_global_config() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let td = tempfile::tempdir().expect("tempdir");
    let config_root = td.path().join("xdg-config");
    std::fs::create_dir_all(&config_root).expect("create config root");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);

    let user_config_path = ProjectConfig::user_config_path().expect("resolve user config path");
    std::fs::create_dir_all(user_config_path.parent().expect("user config dir"))
        .expect("create user config dir");
    // opencode + acp is the still-invalid combination after #1128 flipped
    // codex CLI to a legal transport. opencode has no ACP transport, so the
    // merge still produces a validation error tagged on the offending key.
    // The invalid value lives in USER config; project config stays valid so
    // doctor still reports `.csa/config.toml` as Valid in isolation.
    std::fs::write(
        &user_config_path,
        r#"
[tools.opencode]
transport = "acp"
"#,
    )
    .expect("write invalid user config");

    write_project_config(
        td.path(),
        r#"
[tools.opencode]
transport = "auto"
"#,
    );

    let merged_error = ProjectConfig::load(td.path()).expect_err("merged load should fail");
    assert!(
        format!("{merged_error:#}").contains("tools.opencode.transport"),
        "test fixture should exercise an invalid user-level transport override: {merged_error}"
    );

    let status = inspect_doctor_project_config_from(td.path());
    let rendered = render_project_config_lines(&status).join("\n");

    assert!(
        matches!(status, DoctorProjectConfigStatus::Valid(_)),
        "doctor should validate .csa/config.toml independently of broken user config: {rendered}"
    );
    assert!(
        rendered.contains("Config:      .csa/config.toml (valid)"),
        "doctor should keep the project config display valid when user config is broken: {rendered}"
    );
}

#[test]
fn project_config_summary_reflects_raw_project_config_not_global_disable() {
    // The `=== Project Config ===` summary must report the RAW `.csa/config.toml`
    // project config ONLY, never the effective (merged) config. Real-world case
    // from #1836: a tool disabled solely in GLOBAL config
    // (`[tools.claude-code].enabled = false`) is unconfigured at the project
    // layer, so it stays Enabled under the project header — labeling that merged
    // state as "Project Config" would misrepresent the project file. The runtime
    // enablement gate (merged config) instead lives on the EFFECTIVE surface that
    // `=== Tool Availability ===` renders, asserted below via the merged config
    // (#1752 residual / #1836).
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let td = tempfile::tempdir().expect("tempdir");
    let config_root = td.path().join("xdg-config");
    std::fs::create_dir_all(&config_root).expect("create config root");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);

    // GLOBAL (user) config disables claude-code; the project file does not.
    let user_config_path = ProjectConfig::user_config_path().expect("resolve user config path");
    std::fs::create_dir_all(user_config_path.parent().expect("user config dir"))
        .expect("create user config dir");
    std::fs::write(
        &user_config_path,
        r#"
[tools.claude-code]
enabled = false
"#,
    )
    .expect("write user config");

    // Project `.csa/config.toml` leaves claude-code unconfigured (=> enabled by
    // default at the project layer).
    write_project_config(
        td.path(),
        r#"
[tools.codex]
enabled = true
"#,
    );

    // RAW project surface: claude-code stays Enabled, never Disabled.
    let project_status = inspect_doctor_project_config_from(td.path());
    let rendered = render_project_config_lines(&project_status).join("\n");
    let enabled_line = rendered
        .lines()
        .find(|line| line.starts_with("Enabled:"))
        .unwrap_or_default();
    let disabled_line = rendered
        .lines()
        .find(|line| line.starts_with("Disabled:"))
        .unwrap_or_default();
    assert!(
        matches!(project_status, DoctorProjectConfigStatus::Valid(_)),
        "project config should parse as valid: {rendered}"
    );
    assert!(
        enabled_line.contains("claude-code"),
        "a tool disabled only in global config must stay Enabled in the raw project summary: {rendered}"
    );
    assert!(
        !disabled_line.contains("claude-code"),
        "a global-only-disabled tool must NOT appear as Disabled under the project header: {rendered}"
    );

    // EFFECTIVE surface: the merged gate (what `csa run` enforces and the
    // `=== Tool Availability ===` blocks render) does report claude-code disabled.
    let effective_status = inspect_doctor_effective_config_from(td.path());
    let effective = effective_status
        .runtime_config()
        .expect("merged config should be valid");
    assert!(
        !effective.is_tool_enabled("claude-code"),
        "the effective (merged) gate must reflect the global disable"
    );
}

#[test]
fn project_config_summary_shows_project_disabled_tool_as_disabled() {
    // Complementary direction: a tool disabled IN the project file itself is
    // genuine raw project state, so it MUST appear under Disabled in the summary.
    let project_config = project_config_with_disabled_tool("claude-code", TransportKind::Cli);
    let status = DoctorProjectConfigStatus::Valid(Box::new(project_config));

    let rendered = render_project_config_lines(&status).join("\n");
    let enabled_line = rendered
        .lines()
        .find(|line| line.starts_with("Enabled:"))
        .unwrap_or_default();
    let disabled_line = rendered
        .lines()
        .find(|line| line.starts_with("Disabled:"))
        .unwrap_or_default();

    assert!(
        disabled_line.contains("claude-code"),
        "a tool disabled in the project file must appear under Disabled in the raw summary: {rendered}"
    );
    assert!(
        !enabled_line.contains("claude-code"),
        "a project-disabled tool must not also appear under Enabled: {rendered}"
    );
}

#[test]
fn doctor_text_reports_invalid_effective_config() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let td = tempfile::tempdir().expect("tempdir");
    let config_root = td.path().join("xdg-config");
    std::fs::create_dir_all(&config_root).expect("create config root");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);

    let user_config_path = ProjectConfig::user_config_path().expect("resolve user config path");
    std::fs::create_dir_all(user_config_path.parent().expect("user config dir"))
        .expect("create user config dir");
    // opencode + acp is the still-invalid combination after #1128 flipped
    // codex CLI to a legal transport. opencode has no ACP transport, so the
    // merge still produces a validation error tagged on the offending key.
    // The invalid value lives in USER config; project config stays valid so
    // doctor still reports `.csa/config.toml` as Valid in isolation.
    std::fs::write(
        &user_config_path,
        r#"
[tools.opencode]
transport = "acp"
"#,
    )
    .expect("write invalid user config");

    write_project_config(
        td.path(),
        r#"
[tools.opencode]
transport = "auto"
"#,
    );

    let effective_status = inspect_doctor_effective_config_from(td.path());
    let rendered = render_effective_config_lines(&effective_status).join("\n");
    let tool_lines = render_tool_availability_error_lines(
        &effective_status
            .tool_availability_error()
            .expect("tool availability error should be present"),
    )
    .join("\n");

    assert!(
        matches!(effective_status, DoctorEffectiveConfigStatus::Invalid(_)),
        "doctor should classify merged config failures explicitly: {rendered}"
    );
    assert!(
        rendered.contains("Effective:   merged config (invalid)"),
        "doctor text should surface the invalid effective-config branch: {rendered}"
    );
    assert!(
        rendered.contains("tools.opencode.transport"),
        "doctor text should surface the exact merged-config key: {rendered}"
    );
    assert!(
        tool_lines.contains("Tool availability unknown (effective config invalid)"),
        "doctor text should not pretend defaults are ready when merged config failed: {tool_lines}"
    );

    tokio::runtime::Runtime::new()
        .expect("create tokio runtime")
        .block_on(run_doctor_text_from(td.path()))
        .expect("doctor text should keep running when effective config is invalid");
}

#[test]
fn doctor_json_reports_invalid_effective_config() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let td = tempfile::tempdir().expect("tempdir");
    let config_root = td.path().join("xdg-config");
    std::fs::create_dir_all(&config_root).expect("create config root");
    let _home_guard = EnvVarGuard::set("HOME", td.path());
    let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &config_root);

    let user_config_path = ProjectConfig::user_config_path().expect("resolve user config path");
    std::fs::create_dir_all(user_config_path.parent().expect("user config dir"))
        .expect("create user config dir");
    // opencode + acp is the still-invalid combination after #1128 flipped
    // codex CLI to a legal transport. opencode has no ACP transport, so the
    // merge still produces a validation error tagged on the offending key.
    // The invalid value lives in USER config; project config stays valid so
    // doctor still reports `.csa/config.toml` as Valid in isolation.
    std::fs::write(
        &user_config_path,
        r#"
[tools.opencode]
transport = "acp"
"#,
    )
    .expect("write invalid user config");

    write_project_config(
        td.path(),
        r#"
[tools.opencode]
transport = "auto"
"#,
    );

    let report = build_doctor_json(td.path());
    let effective = &report["effective_config"];
    let effective_error = effective["error"]
        .as_str()
        .expect("doctor JSON should include effective-config error text");
    let tools_error = report["tools_error"]
        .as_str()
        .expect("doctor JSON should include tool availability error text");

    assert_eq!(report["config"]["valid"], serde_json::json!(true));
    assert_eq!(effective["valid"], serde_json::json!(false));
    assert!(
        effective_error.contains("tools.opencode.transport"),
        "doctor JSON should surface the exact merged-config key: {effective_error}"
    );
    assert!(
        tools_error.contains("Tool availability unknown (effective config invalid)"),
        "doctor JSON should mark tool availability unknown when merged config fails: {tools_error}"
    );
    assert_eq!(report["tools"], serde_json::json!([]));
}

#[cfg(all(unix, feature = "codex-acp"))]
#[test]
fn feature_build_doctor_text_output_reports_acp_compile_status() {
    with_stubbed_codex_on_path(|| {
        let status = check_tool_status("codex", None);
        let rendered = render_tool_status_lines(&status).join("\n");

        assert!(
            rendered.contains("ACP compiled in: yes"),
            "feature build should report that ACP support is compiled in: {rendered}"
        );
        // Even with the `codex-acp` feature compiled in, codex defaults to the
        // CLI transport (#760 / #1128) — ACP is opt-in. The doctor must report
        // the CLI transport it will really use, then surface the ACP opt-in
        // hint so the user knows they can switch.
        assert!(
            rendered.contains("Active transport: cli"),
            "feature build should still default codex to the CLI transport: {rendered}"
        );
        assert!(
            rendered.contains("ACP override: set [tools.codex].transport = \"acp\""),
            "feature build should surface the ACP opt-in hint when ACP is compiled in but CLI is active: {rendered}"
        );
    });
}
