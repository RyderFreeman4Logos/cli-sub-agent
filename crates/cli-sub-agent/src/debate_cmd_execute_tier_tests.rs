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

    let codex_invocation_log = project_dir.path().join("codex-invocations.log");
    let opencode_invocation_log = project_dir.path().join("opencode-invocations.log");
    let codex_invocation_log_str = codex_invocation_log.display().to_string();
    let opencode_invocation_log_str = opencode_invocation_log.display().to_string();

    for (binary, body) in [
        (
            "codex",
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf 'codex-cli 1.0.0\\n'\n  exit 0\nfi\ncount=0\nif [ -f \"{codex_invocation_log_str}\" ]; then\n  count=$(wc -l < \"{codex_invocation_log_str}\")\nfi\nnext=$((count + 1))\nprintf 'attempt-%s\\n' \"$next\" >> \"{codex_invocation_log_str}\"\nif [ \"$next\" -eq 1 ]; then\n  printf 'codex_429_retry_exhausted: temporary codex 429 rate limit persisted after 3 retries\\n' >&2\n  exit 1\nfi\nprintf '%s\\n' 'Verdict: APPROVE' 'Confidence: high' 'Summary: Debate succeeded via second codex tier candidate.' '- Fallback stayed within the codex preference.'\n"
            ),
        ),
        (
            "opencode",
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf 'opencode 1.0.0\\n'\n  exit 0\nfi\nprintf 'opencode should not be invoked\\n' >> \"{opencode_invocation_log_str}\"\nexit 1\n"
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

    let mut config = project_config_with_enabled_tools(&["codex", "opencode"]);
    config.review = Some(csa_config::ReviewConfig {
        gate_command: Some("true".to_string()),
        ..Default::default()
    });
    config.tiers.insert(
        "quality".to_string(),
        csa_config::config::TierConfig {
            description: "quality".to_string(),
            models: vec![
                "codex/openai/gpt-5.4/medium".to_string(),
                "codex/openai/gpt-5/high".to_string(),
                "opencode/openai/gpt-5/high".to_string(),
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
        Some("codex"),
    )
    .unwrap();

    let cd = project_dir.path().display().to_string();
    let args = parse_debate_args(&[
        "csa",
        "debate",
        "--cd",
        &cd,
        "--tool",
        "codex",
        "--tier",
        "quality",
        "--session",
        &session.meta_session_id,
        "Should we ship this migration?",
    ]);

    let exit_code = handle_debate(
        args,
        0,
        csa_core::types::OutputFormat::Json,
        &crate::startup_env::EMPTY_STARTUP_SUBTREE_ENV,
    )
    .await
    .expect("explicit codex debate tier fallback should succeed");
    assert_eq!(exit_code, 0);

    let codex_invocations = std::fs::read_to_string(&codex_invocation_log).unwrap();
    assert_eq!(codex_invocations.lines().count(), 2);
    assert!(
        !opencode_invocation_log.exists(),
        "opencode should not be invoked because the preferred codex fallback succeeds first"
    );

    let sessions_dir = csa_session::get_session_root(project_dir.path())
        .unwrap()
        .join("sessions");
    let child = std::fs::read_dir(&sessions_dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            csa_session::load_session(project_dir.path(), &entry.file_name().to_string_lossy()).ok()
        })
        .find(|candidate| {
            let linked = candidate.genealogy.parent_session_id.as_deref()
                == Some(session.meta_session_id.as_str());
            let has_success_verdict =
                csa_session::get_session_dir(project_dir.path(), &candidate.meta_session_id)
                    .ok()
                    .and_then(|dir| {
                        std::fs::read_to_string(dir.join("output/debate-verdict.json")).ok()
                    })
                    .and_then(|verdict| serde_json::from_str::<serde_json::Value>(&verdict).ok())
                    .is_some_and(|verdict| {
                        verdict["summary"] == "Debate succeeded via second codex tier candidate."
                    });
            linked && has_success_verdict
        })
        .expect("cross-model fallback must create a linked successful child session");
    assert_ne!(child.meta_session_id, session.meta_session_id);
    let session_dir =
        csa_session::get_session_dir(project_dir.path(), &child.meta_session_id).unwrap();
    let verdict_path = session_dir.join("output").join("debate-verdict.json");
    let parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&verdict_path).unwrap()).unwrap();

    assert_ne!(parsed["decision"], "unavailable");
    assert_eq!(parsed["verdict"], "APPROVE");
    assert_eq!(
        parsed["summary"],
        "Debate succeeded via second codex tier candidate."
    );
    assert!(parsed["failure_reason"].is_null());

    let result = csa_session::load_result(project_dir.path(), &child.meta_session_id)
        .unwrap()
        .expect("result.toml should exist");
    assert_eq!(result.tool, "codex");
    assert_eq!(
        result.summary,
        "Debate succeeded via second codex tier candidate."
    );
}
