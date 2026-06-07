use super::*;
use crate::test_session_sandbox::ScopedSessionSandbox;
use std::path::Path;
use std::process::Command;

fn run_git_capture(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git command should execute");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("git stdout should be utf-8")
        .trim()
        .to_string()
}

#[cfg(unix)]
#[tokio::test]
async fn handle_review_fix_loop_uses_effective_fallback_tool() {
    use std::os::unix::fs::PermissionsExt;

    let project_dir = setup_git_repo();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    let bin_dir = project_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let opencode_count_path = project_dir.path().join("opencode-count.txt");

    // Codex stubs are used here instead of gemini-cli to avoid a bwrap +
    // CSA_TEST_DISABLE_GEMINI_DIRECT_LAUNCH interaction (#1407): build_merged_env
    // injects CSA_TEST_DISABLE_GEMINI_DIRECT_LAUNCH=1 for gemini-cli which forces the
    // launch command to the bare name "gemini" (no absolute path). Inside the bwrap
    // sandbox the stub dir is unmounted, so PATH lookup bypasses the stub and reaches
    // the real mise-managed gemini binary, triggering live API calls that hang the test.
    // Codex uses CLI transport with no runtime PATH-pinning; PATH stubs work reliably
    // when combined with no_fs_sandbox=true, which skips bwrap so the stub dir is
    // accessible. Keep no_fs_sandbox=true on both the initial review and the fix loop.
    let codex_stub = "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf 'codex-cli 1.0.0\\n'\n  exit 0\nfi\nprintf 'codex_429_retry_exhausted: temporary codex 429 rate limit persisted after 3 retries\\n' >&2\nexit 1\n";
    for binary in ["codex", "codex-acp"] {
        std::fs::write(bin_dir.join(binary), codex_stub).unwrap();
    }
    let opencode_stub = format!(
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf 'opencode 1.0.0\\n'\n  exit 0\nfi\ncount=$(cat \"{}\" 2>/dev/null || printf '0')\ncount=$((count + 1))\nprintf '%s' \"$count\" > \"{}\"\nif [ \"$count\" -eq 1 ]; then\n  printf '%s\\n' '<!-- CSA:SECTION:summary -->' 'FAIL' '<!-- CSA:SECTION:summary:END -->' '<!-- CSA:SECTION:details -->' 'Found issue in tracked.txt.' '<!-- CSA:SECTION:details:END -->'\nelse\n  printf '%s\\n' '<!-- CSA:SECTION:summary -->' 'PASS' '<!-- CSA:SECTION:summary:END -->' '<!-- CSA:SECTION:details -->' 'Issue fixed.' '<!-- CSA:SECTION:details:END -->'\nfi\n",
        opencode_count_path.display(),
        opencode_count_path.display()
    );
    std::fs::write(bin_dir.join("opencode"), &opencode_stub).unwrap();
    for binary in ["codex", "codex-acp", "opencode"] {
        let path = bin_dir.join(binary);
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let patched_path = format!("{}:{inherited_path}", bin_dir.display());
    let _path_guard = ScopedEnvVarRestore::set("PATH", &patched_path);

    let mut config = project_config_with_enabled_tools(&["codex", "opencode"]);
    config.review = Some(csa_config::ReviewConfig {
        gate_command: Some("true".to_string()),
        ..Default::default()
    });
    config.tools.get_mut("codex").unwrap().restrictions = Some(ToolRestrictions {
        allow_edit_existing_files: false,
        allow_write_new_files: false,
    });
    config.tools.get_mut("codex").unwrap().transport = Some(csa_config::TransportKind::Cli);
    config.tiers.insert(
        "quality".to_string(),
        csa_config::config::TierConfig {
            description: "quality".to_string(),
            models: vec![
                "codex/openai/gpt-5.4/high".to_string(),
                "opencode/anthropic/claude-sonnet-4-5-20250929/default".to_string(),
            ],
            strategy: csa_config::TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );

    let global = GlobalConfig::default();
    let initial = execute_review_for_tests(
        ToolName::Codex,
        "scope=files:tracked.txt mode=review-and-fix security=auto".to_string(),
        None,
        None,
        Some("codex/openai/gpt-5.4/high".to_string()),
        Some("quality".to_string()),
        true,
        None,
        "review: fix-loop-effective-fallback-tool".to_string(),
        project_dir.path(),
        Some(&config),
        &global,
        ReviewRoutingMetadata {
            project_profile: ProjectProfile::Unknown,
            detection_method: "auto",
        },
        csa_process::StreamMode::BufferOnly,
        crate::pipeline::DEFAULT_IDLE_TIMEOUT_SECONDS,
        None,
        false,
        false,
        false,
        true,
        false,
        &[],
        &[],
        Some(false), // error_marker_scan_override: force scan OFF for marker-bearing fixtures (#1745)
    )
    .await
    .expect("initial review should fall back to opencode");
    assert_eq!(initial.executed_tool, ToolName::Opencode);

    let exit_code = super::fix::run_fix_loop(super::fix::FixLoopContext {
        effective_tool: initial.executed_tool,
        config: Some(&config),
        global_config: &global,
        review_model: None,
        effective_tier_model_spec: initial.routed_to.clone(),
        review_thinking: None,
        review_routing: ReviewRoutingMetadata {
            project_profile: ProjectProfile::Unknown,
            detection_method: "auto",
        },
        stream_mode: csa_process::StreamMode::BufferOnly,
        idle_timeout_seconds: crate::pipeline::DEFAULT_IDLE_TIMEOUT_SECONDS,
        initial_response_timeout_seconds: None,
        force_override_user_config: false,
        force_ignore_tier_setting: false,
        no_failover: false,
        build_jobs: None,
        fast_but_more_cost: false,
        no_fs_sandbox: true,
        error_marker_scan_override: None,
        extra_writable: &[],
        extra_readable: &[],
        timeout: None,
        diff_report: super::diff_size::ReviewDiffReport {
            diff_size: None,
            large_diff_warning: None,
        },
        project_root: project_dir.path(),
        scope: "files:tracked.txt".to_string(),
        decision: ReviewDecision::Fail.as_str().to_string(),
        verdict: "HAS_ISSUES".to_string(),
        review_mode: None,
        max_rounds: 1,
        initial_session_id: initial.execution.meta_session_id.clone(),
        codex_single: false,
        review_iterations: 0,
        current_depth: 0,
        startup_env: &crate::startup_env::EMPTY_STARTUP_SUBTREE_ENV,
    })
    .await
    .expect("fix loop should use fallback tool");
    assert_eq!(exit_code, 0);
    assert_eq!(
        std::fs::read_to_string(&opencode_count_path).unwrap(),
        "2",
        "opencode must handle both the fallback review round and the fix round"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn handle_review_fix_codex_single_resumes_with_edit_prompt_and_changes_files() {
    use std::os::unix::fs::PermissionsExt;

    let project_dir = setup_git_repo();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    let bin_dir = project_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let count_path = project_dir.path().join("codex-count.txt");
    let args_log = project_dir.path().join("codex-args.log");
    let prompt_log = project_dir.path().join("codex-fix-prompt.txt");
    let tracked_path = project_dir.path().join("tracked.txt");
    let initial_head = run_git_capture(project_dir.path(), &["rev-parse", "HEAD"]);
    let codex_stub = format!(
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  printf 'codex-cli 1.0.0\n'
  exit 0
fi
count=$(cat "{count}" 2>/dev/null || printf '0')
count=$((count + 1))
printf '%s' "$count" > "{count}"
printf 'run %s\n' "$count" >> "{args_log}"
printf '%s\n' "$@" >> "{args_log}"
last_arg=
for arg in "$@"; do
  last_arg="$arg"
done
if [ "$count" -eq 1 ]; then
  printf '%s\n' \
    '<!-- CSA:SECTION:summary -->' \
    'FAIL: tracked.txt still contains the stale value.' \
    '<!-- CSA:SECTION:summary:END -->' \
    '<!-- CSA:SECTION:details -->' \
    'High correctness finding BUG-1884 in tracked.txt:1.' \
    '```findings.toml' \
    '[[findings]]' \
    'id = "BUG-1884"' \
    'severity = "high"' \
    'description = "tracked.txt still says baseline."' \
    '[[findings.file_ranges]]' \
    'path = "tracked.txt"' \
    'start = 1' \
    '```' \
    '<!-- CSA:SECTION:details:END -->' \
    '{{"session_id":"thread-1884"}}'
  exit 0
fi
printf '%s' "$last_arg" > "{prompt_log}"
case " $* " in
  *" resume "*"thread-1884"*) ;;
  *) printf 'missing codex resume args\n' >&2; exit 7 ;;
esac
grep -q 'Codex single-review fix pass 1/1' "{prompt_log}" || exit 8
grep -q 'Do not re-report the findings' "{prompt_log}" || exit 9
grep -q 'BUG-1884' "{prompt_log}" || exit 10
printf 'fixed\n' > "{tracked}"
git -C "{project}" add tracked.txt || exit 11
git -C "{project}" commit -m 'fix: update tracked fixture' || exit 12
printf '%s\n' \
  '<!-- CSA:SECTION:summary -->' \
  'PASS: fixed tracked.txt.' \
  '<!-- CSA:SECTION:summary:END -->' \
  '<!-- CSA:SECTION:details -->' \
  'Applied the requested fix and verified the result.' \
  '<!-- CSA:SECTION:details:END -->' \
  '{{"session_id":"thread-1884"}}'
"#,
        count = count_path.display(),
        args_log = args_log.display(),
        prompt_log = prompt_log.display(),
        project = project_dir.path().display(),
        tracked = tracked_path.display(),
    );
    for binary in ["codex", "codex-acp"] {
        let path = bin_dir.join(binary);
        std::fs::write(&path, &codex_stub).unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
    }

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let patched_path = format!("{}:{inherited_path}", bin_dir.display());
    let _path_guard = ScopedEnvVarRestore::set("PATH", &patched_path);

    let mut config = project_config_with_enabled_tools(&["codex"]);
    config.review = Some(csa_config::ReviewConfig {
        gate_command: Some("true".to_string()),
        ..Default::default()
    });
    config.tools.get_mut("codex").unwrap().transport = Some(csa_config::TransportKind::Cli);
    write_review_project_config(project_dir.path(), &config);
    install_pattern(project_dir.path(), "csa-review");

    let cd = project_dir.path().display().to_string();
    let args = parse_review_args(&[
        "csa",
        "review",
        "--cd",
        &cd,
        "--files",
        "tracked.txt",
        "--tool",
        "codex",
        "--single",
        "--fix",
        "--max-rounds",
        "1",
        "--no-fs-sandbox",
    ]);

    let exit_code = handle_review(args, 0, &crate::startup_env::EMPTY_STARTUP_SUBTREE_ENV)
        .await
        .expect("codex single fix loop should converge");
    assert_eq!(exit_code, 0);
    assert_eq!(std::fs::read_to_string(&count_path).unwrap(), "2");
    assert_eq!(std::fs::read_to_string(&tracked_path).unwrap(), "fixed\n");
    let final_head = run_git_capture(project_dir.path(), &["rev-parse", "HEAD"]);
    assert_ne!(
        final_head, initial_head,
        "codex fix pass should commit the fix"
    );
    assert!(
        run_git_capture(
            project_dir.path(),
            &["status", "--short", "--", "tracked.txt"]
        )
        .is_empty(),
        "codex fix pass should commit the touched file"
    );

    let sessions = csa_session::list_sessions(project_dir.path(), None).unwrap();
    assert_eq!(sessions.len(), 1, "fix loop should resume the same session");
    let session_dir =
        csa_session::get_session_dir(project_dir.path(), &sessions[0].meta_session_id).unwrap();
    let meta: csa_session::state::ReviewSessionMeta = serde_json::from_str(
        &std::fs::read_to_string(session_dir.join("review_meta.json")).unwrap(),
    )
    .unwrap();
    let findings: csa_session::FindingsFile = toml::from_str(
        &std::fs::read_to_string(session_dir.join("output").join("findings.toml")).unwrap(),
    )
    .unwrap();

    assert_eq!(meta.decision, ReviewDecision::Pass.as_str());
    assert!(meta.fix_attempted);
    assert_eq!(meta.fix_rounds, 1);
    assert!(findings.findings.is_empty());
}
