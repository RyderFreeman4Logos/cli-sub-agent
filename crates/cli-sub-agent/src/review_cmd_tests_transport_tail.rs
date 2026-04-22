use super::*;

#[cfg(unix)]
#[test]
fn resolve_review_tool_auto_skips_counterpart_without_configured_binary() {
    use crate::test_env_lock::ScopedTestEnvVar;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let td = tempfile::tempdir().expect("tempdir");
    let bin_dir = td.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("create bin dir");

    let which_path = bin_dir.join("which");
    fs::write(
        &which_path,
        "#!/bin/sh\nif [ \"$1\" = \"codex-acp\" ]; then\n  exit 0\nfi\nexit 1\n",
    )
    .expect("write which stub");
    let mut perms = fs::metadata(&which_path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&which_path, perms).expect("chmod which");

    let inherited_path = std::env::var_os("PATH").unwrap_or_default();
    let patched_path = std::env::join_paths(
        std::iter::once(bin_dir.clone()).chain(std::env::split_paths(&inherited_path)),
    )
    .expect("join PATH");
    let _path_guard = ScopedTestEnvVar::set("PATH", patched_path);

    let global = GlobalConfig::default();
    let mut cfg = project_config_with_enabled_tools(&["codex"]);
    cfg.review = Some(csa_config::global::ReviewConfig {
        tool: csa_config::ToolSelection::Single("auto".to_string()),
        ..Default::default()
    });
    cfg.debate = None;
    cfg.tools
        .get_mut("codex")
        .expect("codex tool config")
        .transport = Some(csa_config::TransportKind::Cli);

    let err = resolve_review_tool(
        None,
        None,
        Some(&cfg),
        &global,
        Some("claude-code"),
        std::path::Path::new("/tmp/test-project"),
        false,
        None,  // cli_tier
        false, // force_ignore_tier_setting
    )
    .unwrap_err();

    assert!(
        format!("{err:#}").contains("AUTO review tool selection failed"),
        "expected clean auto-selection failure, got: {err:#}"
    );
}
