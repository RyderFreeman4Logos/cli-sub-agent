use super::*;
use crate::test_session_sandbox::ScopedSessionSandbox;
use std::path::Path;
use tempfile::tempdir;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

fn write_debate_project_config(project_root: &Path, config: &ProjectConfig) {
    let config_path = ProjectConfig::config_path(project_root);
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(config_path, toml::to_string_pretty(config).unwrap()).unwrap();
}

fn install_pattern(project_root: &Path, name: &str) {
    let skill_dir = project_root
        .join(".csa")
        .join("patterns")
        .join(name)
        .join("skills")
        .join(name);
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# test pattern\n").unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn execute_debate_advances_tier_fallback_when_explicit_tool_and_tier() {
    if which::which("bwrap").is_err() {
        eprintln!("skipping: bwrap not installed (CI gap, see #987)");
        return;
    }

    let project_dir = tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    let bin_dir = project_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    let gemini_invocation_log = project_dir.path().join("gemini-invocations.log");
    let codex_invocation_log = project_dir.path().join("codex-invocations.log");
    let gemini_invocation_log_str = gemini_invocation_log.display().to_string();
    let codex_invocation_log_str = codex_invocation_log.display().to_string();

    for (binary, body) in [
        (
            "gemini",
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf 'gemini-cli 1.0.0\\n'\n  exit 0\nfi\ncount=0\nif [ -f \"{gemini_invocation_log_str}\" ]; then\n  count=$(wc -l < \"{gemini_invocation_log_str}\")\nfi\nnext=$((count + 1))\nprintf 'attempt-%s\\n' \"$next\" >> \"{gemini_invocation_log_str}\"\nif [ \"$next\" -eq 1 ]; then\n  printf \"reason: 'QUOTA_EXHAUSTED'\\n\" >&2\n  exit 1\nfi\nprintf '%s\\n' 'Verdict: APPROVE' 'Confidence: high' 'Summary: Debate succeeded via second gemini tier candidate.' '- Fallback stayed within the gemini whitelist.'\n"
            ),
        ),
        (
            "codex-acp",
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf 'codex-acp 1.0.0\\n'\n  exit 0\nfi\nprintf 'codex should not be invoked\\n' >> \"{codex_invocation_log_str}\"\nexit 1\n"
            ),
        ),
    ] {
        let path = bin_dir.join(binary);
        std::fs::write(&path, body).unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
    }

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let patched_path = format!("{}:{inherited_path}", bin_dir.display());
    let _path_guard = EnvVarGuard::set("PATH", &patched_path);
    let _available_guard =
        EnvVarGuard::set(crate::run_helpers::TEST_ASSUME_TOOLS_AVAILABLE_ENV, "1");

    let mut config = project_config_with_enabled_tools(&["codex", "gemini-cli"]);
    config.review = Some(csa_config::ReviewConfig {
        gate_command: Some("true".to_string()),
        ..Default::default()
    });
    config.tiers.insert(
        "quality".to_string(),
        csa_config::config::TierConfig {
            description: "quality".to_string(),
            models: vec![
                "gemini-cli/google/gemini-3.1-pro-preview/medium".to_string(),
                "gemini-cli/google/gemini-3.1-pro-preview/xhigh".to_string(),
                "codex/openai/gpt-5/high".to_string(),
            ],
            strategy: csa_config::TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );
    write_debate_project_config(project_dir.path(), &config);
    install_pattern(project_dir.path(), "debate");

    let session = csa_session::create_session(
        project_dir.path(),
        Some("debate explicit-tool tier fallback"),
        None,
        Some("gemini-cli"),
    )
    .unwrap();

    let cd = project_dir.path().display().to_string();
    let args = parse_debate_args(&[
        "csa",
        "debate",
        "--cd",
        &cd,
        "--tool",
        "gemini-cli",
        "--tier",
        "quality",
        "--session",
        &session.meta_session_id,
        "Should we ship this migration?",
    ]);

    let exit_code = handle_debate(args, 0, csa_core::types::OutputFormat::Json)
        .await
        .expect("explicit gemini debate tier fallback should succeed");
    assert_eq!(exit_code, 0);

    let gemini_invocations = std::fs::read_to_string(&gemini_invocation_log).unwrap();
    assert_eq!(gemini_invocations.lines().count(), 2);
    assert!(
        !codex_invocation_log.exists(),
        "codex should not be invoked when --tool gemini-cli whitelists gemini-only candidates"
    );

    let session_dir =
        csa_session::get_session_dir(project_dir.path(), &session.meta_session_id).unwrap();
    let verdict_path = session_dir.join("output").join("debate-verdict.json");
    let parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&verdict_path).unwrap()).unwrap();

    assert_ne!(parsed["decision"], "unavailable");
    assert_eq!(parsed["verdict"], "APPROVE");
    assert_eq!(
        parsed["summary"],
        "Debate succeeded via second gemini tier candidate."
    );
    assert!(parsed["failure_reason"].is_null());

    let result = csa_session::load_result(project_dir.path(), &session.meta_session_id)
        .unwrap()
        .expect("result.toml should exist");
    assert_eq!(result.tool, "gemini-cli");
    assert_eq!(
        result.summary,
        "Debate succeeded via second gemini tier candidate."
    );
}
