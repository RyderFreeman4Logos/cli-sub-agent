use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

fn csa_cmd(tmp: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_csa"));
    scrub_inherited_csa_env(&mut cmd);
    cmd.env("HOME", tmp)
        .env("XDG_STATE_HOME", tmp.join(".local/state"))
        .env("XDG_CONFIG_HOME", tmp.join(".config"))
        .env("TOKIO_WORKER_THREADS", "1");
    cmd
}

fn scrub_inherited_csa_env(cmd: &mut Command) {
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("CSA_") {
            cmd.env_remove(key);
        }
    }
}

fn write_project_config_with_tier(project_root: &Path) {
    let mut config = csa_config::ProjectConfig {
        schema_version: csa_config::config::CURRENT_SCHEMA_VERSION,
        project: csa_config::ProjectMeta {
            name: "test".to_string(),
            created_at: chrono::Utc::now(),
            max_recursion_depth: 5,
        },
        resources: csa_config::ResourcesConfig::default(),
        acp: Default::default(),
        tools: HashMap::new(),
        review: None,
        debate: None,
        tiers: HashMap::new(),
        tier_mapping: HashMap::new(),
        aliases: HashMap::new(),
        tool_aliases: HashMap::new(),
        preferences: None,
        github: None,
        session: Default::default(),
        memory: Default::default(),
        hooks: Default::default(),
        run: Default::default(),
        execution: Default::default(),
        session_wait: None,
        preflight: Default::default(),
        vcs: Default::default(),
        filesystem_sandbox: Default::default(),
    };
    config.tiers.insert(
        "default".to_string(),
        csa_config::config::TierConfig {
            description: "Test tier".to_string(),
            models: vec!["codex/gpt-5-codex/medium".to_string()],
            strategy: csa_config::TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );

    let config_path = csa_config::ProjectConfig::config_path(project_root);
    std::fs::create_dir_all(config_path.parent().expect("config dir")).expect("create config dir");
    std::fs::write(
        config_path,
        toml::to_string_pretty(&config).expect("serialize config"),
    )
    .expect("write config");
}

#[test]
fn run_direct_tool_tier_rejection_before_daemon_does_not_start_session() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_project_config_with_tier(tmp.path());

    let output = csa_cmd(tmp.path())
        .args([
            "run",
            "--sa-mode",
            "true",
            "--tool",
            "codex",
            "--allow-base-branch-working",
            "--no-idle-timeout",
            "--timeout",
            "1800",
            "inspect the repository",
        ])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa run");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected synchronous policy rejection, got {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let printed_session_id = stdout.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.len() == 26
            && trimmed
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    });
    assert!(
        !printed_session_id,
        "policy rejection must not print a daemon session id, stdout: {stdout}"
    );
    assert!(
        stderr.contains("Direct --tool is blocked when tiers are configured"),
        "user-facing cause should be shown, stderr: {stderr}"
    );
    assert!(
        stderr.contains("--tier <name> --tool <tool>"),
        "valid tool+tier retry shape should be shown, stderr: {stderr}"
    );
    assert!(
        !stderr.contains("CSA:SESSION_STARTED"),
        "policy rejection must happen before daemon session allocation, stderr: {stderr}"
    );
    assert!(
        !tmp.path().join(".local/state/cli-sub-agent").exists(),
        "policy rejection must not create session state"
    );
}

#[test]
fn plan_run_tier_rejection_mentions_supported_routing_mechanisms() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let output = csa_cmd(tmp.path())
        .args([
            "plan",
            "run",
            "--sa-mode",
            "false",
            "--tier",
            "tier-4-critical",
            "workflow.toml",
        ])
        .current_dir(tmp.path())
        .output()
        .expect("failed to run csa plan run");

    assert_eq!(
        output.status.code(),
        Some(1),
        "expected actionable plan-run tier rejection, got {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("`csa plan run` does not accept --tier"),
        "plan-run tier rejection should name the unsupported flag, stderr: {stderr}"
    );
    assert!(
        stderr.contains("--var IMPL_TIER=tier-4-critical"),
        "dev2merge/mktd retry shape should be shown, stderr: {stderr}"
    );
    assert!(
        stderr.contains("csa run --tier <name>"),
        "workflow-step retry shape should be shown, stderr: {stderr}"
    );
    assert!(
        !stderr.contains("unexpected argument '--tier'"),
        "CSA should provide guidance instead of raw clap unknown-arg output, stderr: {stderr}"
    );
}
