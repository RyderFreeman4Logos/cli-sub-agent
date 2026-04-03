use super::*;

use std::os::unix::fs::PermissionsExt;

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).expect("write executable");
    let mut perms = fs::metadata(path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).expect("chmod");
}

#[test]
fn prepare_gemini_acp_runtime_sets_runtime_home_and_resolves_direct_launch() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session_id = format!(
        "01TESTGEMINI{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time")
            .as_nanos()
    );
    let source_home = temp.path().join("source-home");
    let source_gemini = source_home.join(".gemini");
    fs::create_dir_all(&source_gemini).expect("create source gemini dir");
    fs::create_dir_all(source_gemini.join("extensions")).expect("create source extensions");
    fs::create_dir_all(source_home.join(".config").join("gemini-cli"))
        .expect("create source config dir");
    fs::create_dir_all(source_home.join(".agents")).expect("create source agents dir");
    fs::write(source_gemini.join("oauth_creds.json"), "oauth").expect("write oauth");
    fs::write(source_gemini.join("settings.json"), "{\"theme\":\"test\"}").expect("write settings");
    fs::write(
        source_home
            .join(".config")
            .join("gemini-cli")
            .join("settings.json"),
        "{\"acp\":true}",
    )
    .expect("write xdg settings");

    let shims_dir = temp.path().join("shims");
    let real_dir = temp.path().join("real");
    let node_dir = temp.path().join("node");
    fs::create_dir_all(&shims_dir).expect("create shims dir");
    fs::create_dir_all(real_dir.join("dist")).expect("create real dist dir");
    fs::create_dir_all(&node_dir).expect("create node dir");

    let mise_path = shims_dir.join("mise");
    write_executable(&mise_path, "#!/bin/sh\nexit 1\n");
    std::os::unix::fs::symlink(&mise_path, shims_dir.join("gemini")).expect("symlink shim");

    let real_script = real_dir.join("dist").join("index.js");
    fs::write(
        &real_script,
        "#!/usr/bin/env -S node --no-warnings=DEP0040\nconsole.log('gemini');\n",
    )
    .expect("write real script");
    std::os::unix::fs::symlink(&real_script, real_dir.join("gemini"))
        .expect("symlink real gemini");

    let node_path = node_dir.join("node");
    write_executable(&node_path, "#!/bin/sh\nexit 0\n");

    let mut env = HashMap::new();
    env.insert(
        "HOME".to_string(),
        source_home.to_string_lossy().into_owned(),
    );
    env.insert(
        "PATH".to_string(),
        std::env::join_paths([&shims_dir, &real_dir, &node_dir])
            .expect("join paths")
            .to_string_lossy()
            .into_owned(),
    );
    env.insert(
        csa_core::gemini::AUTH_MODE_ENV_KEY.to_string(),
        csa_core::gemini::AUTH_MODE_OAUTH.to_string(),
    );

    let launch =
        prepare_gemini_acp_runtime(&mut env, &session_id, &["--acp".to_string()]).expect("prepare runtime");

    assert_eq!(launch.command, node_path.to_string_lossy());
    assert_eq!(launch.args[0], "--no-warnings=DEP0040");
    assert_eq!(launch.args[1], real_script.to_string_lossy());
    assert_eq!(launch.args[2], "--acp");

    let runtime_home = PathBuf::from(env.get("HOME").expect("runtime home"));
    assert_eq!(
        env.get("GEMINI_CLI_HOME"),
        Some(&runtime_home.to_string_lossy().into_owned())
    );
    assert_eq!(
        env.get("XDG_STATE_HOME"),
        Some(
            &runtime_home
                .join(".local")
                .join("state")
                .to_string_lossy()
                .into_owned()
        ),
        "runtime home should redirect XDG state for nested CSA commands"
    );
    assert_eq!(
        env.get("MISE_CACHE_DIR"),
        Some(
            &runtime_home
                .join(GEMINI_RUNTIME_MISE_CACHE_RELATIVE_PATH)
                .to_string_lossy()
                .into_owned()
        ),
        "runtime home should pin mise cache inside the writable Gemini runtime"
    );
    assert_eq!(
        env.get("MISE_STATE_DIR"),
        Some(
            &runtime_home
                .join(GEMINI_RUNTIME_MISE_STATE_RELATIVE_PATH)
                .to_string_lossy()
                .into_owned()
        ),
        "runtime home should pin mise state inside the writable Gemini runtime"
    );
    assert!(
        runtime_home.join(".gemini/oauth_creds.json").exists(),
        "runtime home should mirror oauth creds"
    );
    assert!(
        runtime_home.join(".config/gemini-cli/settings.json").exists(),
        "runtime home should mirror XDG config"
    );
    assert!(
        runtime_home.join(".gemini/extensions").exists(),
        "runtime home should preserve extension access"
    );
    assert!(
        runtime_home.join(".agents").exists(),
        "runtime home should preserve global agent skill access"
    );
    assert_eq!(
        read_selected_auth_type(&runtime_home.join(".gemini/settings.json")),
        Some(GEMINI_SELECTED_TYPE_OAUTH.to_string()),
        "phase 1 runtime must stay OAuth-first even when settings are mirrored"
    );
}

#[test]
fn prepare_gemini_acp_runtime_pins_mise_dirs_under_runtime_home() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session_id = "01TESTGEMINIMISEENV0000000000001";
    let source_home = temp.path().join("source-home");
    fs::create_dir_all(source_home.join(".gemini")).expect("create source gemini dir");

    let mut env = HashMap::new();
    env.insert(
        "HOME".to_string(),
        source_home.to_string_lossy().into_owned(),
    );

    prepare_gemini_acp_runtime(&mut env, session_id, &["--acp".to_string()])
        .expect("prepare runtime");

    let runtime_home = PathBuf::from(env.get("HOME").expect("runtime home"));
    assert_eq!(
        env.get("MISE_CACHE_DIR"),
        Some(
            &runtime_home
                .join(GEMINI_RUNTIME_MISE_CACHE_RELATIVE_PATH)
                .to_string_lossy()
                .into_owned()
        )
    );
    assert_eq!(
        env.get("MISE_STATE_DIR"),
        Some(
            &runtime_home
                .join(GEMINI_RUNTIME_MISE_STATE_RELATIVE_PATH)
                .to_string_lossy()
                .into_owned()
        )
    );
    assert!(
        runtime_home
            .join(GEMINI_RUNTIME_MISE_CACHE_RELATIVE_PATH)
            .is_dir(),
        "runtime should create a dedicated mise cache dir to avoid host ~/.cache/mise writes"
    );
    assert!(
        runtime_home
            .join(GEMINI_RUNTIME_MISE_STATE_RELATIVE_PATH)
            .is_dir(),
        "runtime should create a dedicated mise state dir for Gemini ACP startup"
    );
}

#[test]
fn prepare_gemini_acp_runtime_pins_non_shim_runtime_bins_on_path() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session_id = "01TESTGEMINIPATHPINNING0000000001";
    let source_home = temp.path().join("source-home");
    fs::create_dir_all(source_home.join(".gemini")).expect("create source gemini dir");

    let shims_dir = temp.path().join("shims");
    let real_dir = temp.path().join("real");
    let node_dir = temp.path().join("node");
    let yarn_dir = temp.path().join("yarn");
    fs::create_dir_all(&shims_dir).expect("create shims dir");
    fs::create_dir_all(real_dir.join("dist")).expect("create real dist dir");
    fs::create_dir_all(&node_dir).expect("create node dir");
    fs::create_dir_all(&yarn_dir).expect("create yarn dir");

    let mise_path = shims_dir.join("mise");
    write_executable(&mise_path, "#!/bin/sh\nexit 1\n");
    std::os::unix::fs::symlink(&mise_path, shims_dir.join("gemini")).expect("symlink gemini");
    std::os::unix::fs::symlink(&mise_path, shims_dir.join("node")).expect("symlink node");

    let real_script = real_dir.join("dist").join("index.js");
    fs::write(
        &real_script,
        "#!/usr/bin/env -S node --no-warnings=DEP0040\nconsole.log('gemini');\n",
    )
    .expect("write real script");
    std::os::unix::fs::symlink(&real_script, real_dir.join("gemini"))
        .expect("symlink real gemini");
    write_executable(&node_dir.join("node"), "#!/bin/sh\nexit 0\n");
    write_executable(&yarn_dir.join("yarn"), "#!/bin/sh\nexit 0\n");

    let mut env = HashMap::new();
    env.insert(
        "HOME".to_string(),
        source_home.to_string_lossy().into_owned(),
    );
    env.insert(
        "PATH".to_string(),
        std::env::join_paths([&shims_dir, &real_dir, &yarn_dir, &node_dir])
            .expect("join paths")
            .to_string_lossy()
            .into_owned(),
    );
    env.insert("MISE_SHIM".to_string(), shims_dir.display().to_string());
    env.insert(
        "MISE_SHIMS_DIR".to_string(),
        shims_dir.display().to_string(),
    );

    prepare_gemini_acp_runtime(&mut env, session_id, &["--acp".to_string()])
        .expect("prepare runtime");

    let prepared_path = env.get("PATH").expect("prepared path");
    assert_eq!(
        resolve_first_path_entry("node", prepared_path),
        Some(node_dir.join("node")),
        "nested yarn/node launches must hit the real node binary before any shim"
    );
    assert_eq!(
        resolve_first_path_entry("yarn", prepared_path),
        Some(yarn_dir.join("yarn")),
        "runtime PATH should preserve direct yarn binaries ahead of shim-only entries"
    );
    assert_eq!(env.get("MISE_SHIM"), Some(&String::new()));
    assert_eq!(env.get("MISE_SHIMS_DIR"), Some(&String::new()));
}

#[test]
fn prepare_gemini_acp_runtime_resolves_mise_shims_via_mise_which() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session_id = "01TESTGEMINIMISEWHICH0000000001";
    let source_home = temp.path().join("source-home");
    fs::create_dir_all(source_home.join(".gemini")).expect("create source gemini dir");

    let shims_dir = temp.path().join("shims");
    let real_dir = temp.path().join("real");
    let node_dir = temp.path().join("node");
    let yarn_dir = temp.path().join("yarn");
    fs::create_dir_all(&shims_dir).expect("create shims dir");
    fs::create_dir_all(real_dir.join("dist")).expect("create real dist dir");
    fs::create_dir_all(&node_dir).expect("create node dir");
    fs::create_dir_all(&yarn_dir).expect("create yarn dir");

    let real_script = real_dir.join("dist").join("index.js");
    fs::write(
        &real_script,
        "#!/usr/bin/env -S node --no-warnings=DEP0040\nconsole.log('gemini');\n",
    )
    .expect("write real script");
    std::os::unix::fs::symlink(&real_script, real_dir.join("gemini"))
        .expect("symlink real gemini");
    write_executable(&node_dir.join("node"), "#!/bin/sh\nexit 0\n");
    write_executable(&yarn_dir.join("yarn"), "#!/bin/sh\nexit 0\n");

    let mise_path = shims_dir.join("mise");
    write_executable(
        &mise_path,
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"-C\" ]; then shift 2; fi\nif [ \"$1\" = \"which\" ]; then\n  case \"$2\" in\n    gemini) printf '%s\\n' '{}' ;;\n    node) printf '%s\\n' '{}' ;;\n    yarn) printf '%s\\n' '{}' ;;\n    *) exit 1 ;;\n  esac\n  exit 0\nfi\nexit 1\n",
            real_dir.join("gemini").display(),
            node_dir.join("node").display(),
            yarn_dir.join("yarn").display(),
        ),
    );
    std::os::unix::fs::symlink(&mise_path, shims_dir.join("gemini")).expect("symlink gemini");
    std::os::unix::fs::symlink(&mise_path, shims_dir.join("node")).expect("symlink node");
    std::os::unix::fs::symlink(&mise_path, shims_dir.join("yarn")).expect("symlink yarn");

    let mut env = HashMap::new();
    env.insert(
        "HOME".to_string(),
        source_home.to_string_lossy().into_owned(),
    );
    env.insert("PATH".to_string(), shims_dir.to_string_lossy().into_owned());
    env.insert("MISE_SHIM".to_string(), shims_dir.display().to_string());
    env.insert(
        "MISE_SHIMS_DIR".to_string(),
        shims_dir.display().to_string(),
    );

    let launch =
        prepare_gemini_acp_runtime(&mut env, session_id, &["--acp".to_string()]).expect("prepare runtime");

    assert_eq!(launch.command, node_dir.join("node").to_string_lossy());
    assert_eq!(launch.args[1], real_script.to_string_lossy());
    let prepared_path = env.get("PATH").expect("prepared path");
    assert_eq!(
        resolve_first_path_entry("node", prepared_path),
        Some(node_dir.join("node")),
        "mise-which fallback should pin the real node binary ahead of shim-only PATH entries"
    );
    assert_eq!(
        resolve_first_path_entry("yarn", prepared_path),
        Some(yarn_dir.join("yarn")),
        "mise-which fallback should pin the real yarn binary ahead of shim-only PATH entries"
    );
}

#[test]
fn prepare_gemini_acp_runtime_rewrites_runtime_auth_selection_for_api_key_phase() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session_id = "01TESTGEMINIAUTHSWITCH0000000001";
    let source_home = temp.path().join("source-home");
    fs::create_dir_all(source_home.join(".gemini")).expect("create source gemini dir");

    let mut env = HashMap::new();
    env.insert(
        "HOME".to_string(),
        source_home.to_string_lossy().into_owned(),
    );
    env.insert(
        csa_core::gemini::AUTH_MODE_ENV_KEY.to_string(),
        csa_core::gemini::AUTH_MODE_OAUTH.to_string(),
    );

    prepare_gemini_acp_runtime(&mut env, session_id, &["--acp".to_string()])
        .expect("prepare oauth runtime");
    let runtime_home = PathBuf::from(env.get("HOME").expect("runtime home"));
    assert_eq!(
        read_selected_auth_type(&runtime_home.join(".gemini/settings.json")),
        Some(GEMINI_SELECTED_TYPE_OAUTH.to_string()),
        "first attempt should write OAuth auth selection even without source settings"
    );

    env.insert(
        csa_core::gemini::AUTH_MODE_ENV_KEY.to_string(),
        csa_core::gemini::AUTH_MODE_API_KEY.to_string(),
    );
    env.insert(
        csa_core::gemini::API_KEY_ENV.to_string(),
        "fallback-key".to_string(),
    );

    prepare_gemini_acp_runtime(&mut env, session_id, &["--acp".to_string()])
        .expect("prepare api key runtime");
    assert_eq!(
        read_selected_auth_type(&runtime_home.join(".gemini/settings.json")),
        Some(GEMINI_SELECTED_TYPE_API_KEY.to_string()),
        "phase 2 runtime must override selected auth type to Gemini API key"
    );
    assert_eq!(
        read_selected_auth_type(&runtime_home.join(".config/gemini-cli/settings.json")),
        Some(GEMINI_SELECTED_TYPE_API_KEY.to_string()),
        "phase 2 runtime must keep XDG settings aligned with the selected auth type"
    );
}

#[test]
fn gemini_runtime_home_from_env_prefers_seeded_runtime_paths() {
    let mut env = HashMap::new();
    let runtime_home = std::env::temp_dir()
        .join(GEMINI_RUNTIME_ROOT_DIR)
        .join("01TESTGEMINIRUNTIMEHOME0000000001");
    env.insert(
        "GEMINI_CLI_HOME".to_string(),
        runtime_home.to_string_lossy().into_owned(),
    );
    env.insert("HOME".to_string(), "/home/example".to_string());

    assert_eq!(gemini_runtime_home_from_env(&env), Some(runtime_home));
}

#[test]
fn gemini_runtime_home_from_env_rejects_regular_home_paths() {
    let mut env = HashMap::new();
    env.insert("HOME".to_string(), "/home/example".to_string());
    env.insert(
        "XDG_CONFIG_HOME".to_string(),
        "/home/example/.config".to_string(),
    );

    assert_eq!(gemini_runtime_home_from_env(&env), None);
}

fn read_selected_auth_type(settings_path: &Path) -> Option<String> {
    let raw = fs::read_to_string(settings_path).ok()?;
    let parsed: Value = serde_json::from_str(&raw).ok()?;
    parsed
        .get("security")?
        .get("auth")?
        .get("selectedType")?
        .as_str()
        .map(ToString::to_string)
}

fn resolve_first_path_entry(name: &str, path_env: &str) -> Option<PathBuf> {
    std::env::split_paths(OsStr::new(path_env))
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}
