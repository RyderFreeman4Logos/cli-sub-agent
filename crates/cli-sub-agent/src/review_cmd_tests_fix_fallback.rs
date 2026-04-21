use super::*;
use crate::test_session_sandbox::ScopedSessionSandbox;

#[cfg(unix)]
#[tokio::test]
async fn handle_review_fix_loop_uses_effective_fallback_tool() {
    use std::os::unix::fs::PermissionsExt;

    let project_dir = setup_git_repo();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    let bin_dir = project_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let codex_count_path = project_dir.path().join("codex-count.txt");

    std::fs::write(
        bin_dir.join("gemini"),
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf 'gemini-cli 1.0.0\\n'\n  exit 0\nfi\nprintf \"reason: 'QUOTA_EXHAUSTED'\\n\" >&2\nexit 1\n",
    )
    .unwrap();
    std::fs::write(
        bin_dir.join("codex"),
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf 'codex 1.0.0\\n'\n  exit 0\nfi\ncount=$(cat \"{}\" 2>/dev/null || printf '0')\ncount=$((count + 1))\nprintf '%s' \"$count\" > \"{}\"\nif [ \"$count\" -eq 1 ]; then\n  printf '%s\\n' '<!-- CSA:SECTION:summary -->' 'FAIL' '<!-- CSA:SECTION:summary:END -->' '<!-- CSA:SECTION:details -->' 'Found issue in tracked.txt.' '<!-- CSA:SECTION:details:END -->'\nelse\n  printf '%s\\n' '<!-- CSA:SECTION:summary -->' 'PASS' '<!-- CSA:SECTION:summary:END -->' '<!-- CSA:SECTION:details -->' 'Issue fixed.' '<!-- CSA:SECTION:details:END -->'\nfi\n",
            codex_count_path.display(),
            codex_count_path.display()
        ),
    )
    .unwrap();
    for binary in ["gemini", "codex"] {
        let path = bin_dir.join(binary);
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let patched_path = format!("{}:{inherited_path}", bin_dir.display());
    let _path_guard = ScopedEnvVarRestore::set("PATH", &patched_path);

    let mut config = project_config_with_enabled_tools(&["gemini-cli", "codex"]);
    config.review = Some(csa_config::ReviewConfig {
        gate_command: Some("true".to_string()),
        ..Default::default()
    });
    config.tools.get_mut("gemini-cli").unwrap().restrictions = Some(ToolRestrictions {
        allow_edit_existing_files: false,
        allow_write_new_files: false,
    });
    config.tiers.insert(
        "quality".to_string(),
        csa_config::config::TierConfig {
            description: "quality".to_string(),
            models: vec![
                "gemini-cli/google/gemini-3.1-pro-preview/xhigh".to_string(),
                "codex/openai/gpt-5.4/high".to_string(),
            ],
            strategy: csa_config::TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );
    write_review_project_config(project_dir.path(), &config);
    install_pattern(project_dir.path(), "csa-review");

    let cd = project_dir.path().display().to_string();
    let args = parse_review_args(&[
        "csa",
        "review",
        "--cd",
        &cd,
        "--tier",
        "quality",
        "--files",
        "tracked.txt",
        "--fix",
        "--max-rounds",
        "1",
    ]);

    let exit_code = handle_review(args, 0)
        .await
        .expect("fix loop should use fallback tool");
    assert_eq!(exit_code, 0);
    assert_eq!(
        std::fs::read_to_string(&codex_count_path).unwrap(),
        "2",
        "codex must handle both the fallback review round and the fix round"
    );
}
