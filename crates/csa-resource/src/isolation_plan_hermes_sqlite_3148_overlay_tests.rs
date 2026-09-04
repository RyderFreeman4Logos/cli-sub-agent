//! Overlay-after-writable regressions for migrated Hermes profile binds (#3148).

use super::*;
use std::path::{Path, PathBuf};

#[cfg(unix)]
#[test]
fn migrated_profile_databases_remain_writable_through_bwrap_overlays() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::Builder::new()
        .prefix("hermes-profile-bwrap-write-")
        .tempdir_in("/var/tmp")
        .unwrap();
    let (_home, _env) = isolated_home(&temp);
    let hermes_home = temp.path().join("hermes-home");
    std::fs::create_dir_all(hermes_home.join("direct")).unwrap();
    std::fs::create_dir_all(hermes_home.join("profiles/nested")).unwrap();
    std::fs::create_dir_all(hermes_home.join("logs")).unwrap();
    std::fs::write(hermes_home.join("config.yaml"), "model: test\n").unwrap();
    std::fs::write(hermes_home.join("direct/config.yaml"), "direct: true\n").unwrap();
    std::fs::write(
        hermes_home.join("profiles/nested/config.yaml"),
        "nested: true\n",
    )
    .unwrap();
    let direct_connection = live_sqlite_database(&hermes_home.join("direct/state.db"), "direct");
    let nested_connection =
        live_sqlite_database(&hermes_home.join("profiles/nested/state.db"), "nested");
    let flat_connection = live_sqlite_database(&hermes_home.join("state.flat.db"), "flat");

    let plan = hermes_plan(&hermes_home).expect("Hermes plan must migrate profile databases");
    drop((direct_connection, nested_connection, flat_connection));
    let args = command_args(&plan);
    let runtime = hermes_home.join(".csa-runtime");
    let layouts = [
        (
            "direct",
            hermes_home.join("direct/state.db"),
            runtime.join("direct/state.db"),
            "direct",
        ),
        (
            "nested",
            hermes_home.join("profiles/nested/state.db"),
            runtime.join("profiles/nested/state.db"),
            "nested",
        ),
        (
            "flat",
            hermes_home.join("state.flat.db"),
            runtime.join("state.flat.db"),
            "flat",
        ),
    ];

    for (profile, legacy_db, runtime_db, value) in &layouts {
        assert_eq!(
            crate::isolation_plan::resolve_hermes_state_db(&hermes_home, Some(profile)),
            *runtime_db,
            "{profile} must resolve to the migrated DB used by xurl and recall"
        );
        let migrated = rusqlite::Connection::open(runtime_db).unwrap();
        assert_eq!(
            migrated
                .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        assert_eq!(
            migrated
                .query_row(
                    "SELECT value FROM values_table ORDER BY rowid DESC LIMIT 1",
                    [],
                    |row| { row.get::<_, String>(0) }
                )
                .unwrap(),
            *value
        );
        assert!(
            legacy_db.exists(),
            "legacy {profile} DB must remain in place"
        );

        let requested_dir = legacy_db.parent().unwrap();
        let writable = plan
            .readable_paths
            .iter()
            .find(|path| path.writable_bind() && path.requested() == requested_dir)
            .expect("migrated profile must have a pinned writable bind");
        assert_eq!(
            writable.bind_source(),
            runtime_db.parent().unwrap(),
            "{profile} writes must bind the migrated runtime directory"
        );
        let writable_pos = args
            .windows(3)
            .position(|window| {
                window[0] == "--bind-fd" && window[2] == requested_dir.to_string_lossy().as_ref()
            })
            .expect("migrated profile must have a pinned writable bind");
        if *profile != "flat" {
            let config = requested_dir.join("config.yaml");
            let config_overlay = plan
                .readable_paths
                .iter()
                .find(|path| path.overrides_writable_mount() && path.requested() == config)
                .expect("profile configuration must remain visible read-only");
            let config_pos = args
                .windows(3)
                .position(|window| {
                    window[0] == "--ro-bind-fd"
                        && window[2] == config_overlay.requested().to_string_lossy().as_ref()
                })
                .expect("profile configuration must use a pinned read-only bind");
            assert!(
                writable_pos < config_pos,
                "profile configuration must be restored read-only after its writable directory bind: {args:?}"
            );
        }
        if requested_dir == hermes_home {
            assert_eq!(profile, &"flat");
            assert_eq!(writable.bind_source(), &runtime);
            assert!(
                args.windows(3).any(|window| {
                    window[0] == "--bind-fd" && window[2] == hermes_home.to_string_lossy().as_ref()
                }),
                "flat profile must be covered by the pinned writable Hermes home bind: {args:?}"
            );
            continue;
        }
        let overlay = plan
            .readable_paths
            .iter()
            .filter(|path| {
                path.overrides_writable_mount() && requested_dir.starts_with(path.requested())
            })
            .max_by_key(|path| path.requested().components().count())
            .expect("migrated profile must remain under a read-only overlay");
        let overlay_pos = args
            .windows(3)
            .position(|window| {
                window[0] == "--ro-bind-fd"
                    && window[2] == overlay.requested().to_string_lossy().as_ref()
            })
            .expect("profile overlay must use its pinned read-only bind");
        assert!(
            overlay_pos < writable_pos,
            "writable {profile} bind must follow its parent overlay: {args:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn failed_preflight_does_not_activate_partial_runtime_sqlite_for_all_layouts() {
    let _guard = ENV_LOCK.lock().unwrap();
    let layouts: [(&str, Option<&str>, &str); 4] = [
        ("root", None, "state.db"),
        ("flat", Some("flat"), "state.flat.db"),
        ("direct", Some("direct"), "direct/state.db"),
        ("nested", Some("nested"), "profiles/nested/state.db"),
    ];
    for (label, profile, legacy_rel) in layouts {
        let temp = tempfile::Builder::new()
            .prefix(&format!("hermes-partial-runtime-{label}-"))
            .tempdir_in("/var/tmp")
            .unwrap();
        let (_home, _env) = isolated_home(&temp);
        let hermes_home = temp.path().join("hermes-home");
        std::fs::create_dir_all(hermes_home.join("logs")).unwrap();
        if let Some(parent) = hermes_home.join(legacy_rel).parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(hermes_home.join("config.yaml"), "model: test\n").unwrap();
        if label == "direct" {
            std::fs::write(hermes_home.join("direct/config.yaml"), "direct: true\n").unwrap();
        }
        if label == "nested" {
            std::fs::write(
                hermes_home.join("profiles/nested/config.yaml"),
                "nested: true\n",
            )
            .unwrap();
        }
        let legacy_db = hermes_home.join(legacy_rel);
        let source = live_sqlite_database(&legacy_db, label);
        std::os::unix::fs::symlink("/etc/passwd", hermes_home.join("poison")).unwrap();

        let error = hermes_plan(&hermes_home).expect_err(label);
        drop(source);
        assert!(
            error
                .to_string()
                .contains("hermes sandbox preflight failed"),
            "{label} late overlay failure must fail closed: {error:#}"
        );

        let resolved = crate::isolation_plan::resolve_hermes_state_db(&hermes_home, profile);
        assert_eq!(
            resolved, legacy_db,
            "{label} must keep legacy authoritative after failed preflight"
        );
        let runtime_db = hermes_home.join(".csa-runtime").join(legacy_rel);
        assert_ne!(
            resolved, runtime_db,
            "{label} must not activate a partial runtime generation"
        );
        assert!(
            !hermes_home
                .join(".csa-runtime")
                .join(".csa-runtime-ready")
                .is_file(),
            "{label} must not publish a runtime activation marker after failed preflight"
        );
    }
}

#[cfg(unix)]
#[test]
fn configured_profiles_without_state_db_have_writable_authoritative_runtime_paths() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::Builder::new()
        .prefix("hermes-profile-initial-state-")
        .tempdir_in("/var/tmp")
        .unwrap();
    let (_home, _env) = isolated_home(&temp);
    let hermes_home = temp.path().join("hermes-home");
    std::fs::create_dir_all(hermes_home.join("direct")).unwrap();
    std::fs::create_dir_all(hermes_home.join("profiles/nested")).unwrap();
    std::fs::create_dir_all(hermes_home.join("logs")).unwrap();
    std::fs::write(hermes_home.join("config.yaml"), "model: test\n").unwrap();
    std::fs::write(hermes_home.join("direct/config.yaml"), "direct: true\n").unwrap();
    std::fs::write(
        hermes_home.join("profiles/nested/config.yaml"),
        "nested: true\n",
    )
    .unwrap();

    let plan = hermes_plan(&hermes_home).expect("config-only profiles must plan");
    for (profile, requested, runtime) in [
        (
            "direct",
            hermes_home.join("direct"),
            hermes_home.join(".csa-runtime/direct"),
        ),
        (
            "nested",
            hermes_home.join("profiles/nested"),
            hermes_home.join(".csa-runtime/profiles/nested"),
        ),
    ] {
        let state_db = runtime.join("state.db");
        assert_eq!(
            crate::isolation_plan::resolve_hermes_state_db(&hermes_home, Some(profile)),
            state_db,
            "{profile} xurl and recall must resolve the runtime initial state"
        );
        let writable = plan
            .readable_paths
            .iter()
            .find(|path| path.writable_bind() && path.requested() == requested)
            .expect("config-only profile must have a pinned writable runtime bind");
        assert_eq!(writable.bind_source(), &runtime);
        assert!(
            plan.readable_paths.iter().any(|path| {
                path.overrides_writable_mount() && path.requested() == requested.join("config.yaml")
            }),
            "{profile} config must remain read-only after the writable profile bind"
        );
        let database = rusqlite::Connection::open(&state_db)
            .expect("authoritative runtime path must permit initial state.db creation");
        database
            .execute_batch("CREATE TABLE initial_state (value TEXT NOT NULL);")
            .unwrap();
    }
}

#[cfg(unix)]
struct ReservedLeafCreatedHook;

#[cfg(unix)]
impl ReservedLeafCreatedHook {
    fn set(inject: fn(&Path)) -> Self {
        super::super::super::super::readable::AFTER_RESERVED_LEAF_CREATED
            .with(|hook| hook.set(Some(inject)));
        Self
    }
}

#[cfg(unix)]
impl Drop for ReservedLeafCreatedHook {
    fn drop(&mut self) {
        super::super::super::super::readable::AFTER_RESERVED_LEAF_CREATED
            .with(|hook| hook.set(None));
    }
}

#[cfg(unix)]
fn reserved_names(root: &Path, prefix: &str) -> Vec<PathBuf> {
    let mut names = Vec::new();
    let mut dirs = vec![
        root.to_path_buf(),
        root.join(".csa-runtime"),
        root.join("logs"),
    ];
    dirs.extend(
        std::fs::read_dir(root)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir()),
    );
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(prefix))
            {
                names.push(path);
            }
        }
    }
    names
}

#[cfg(unix)]
fn mark_reserved_leaf_created(path: &Path) {
    let prefix = std::env::var("CSA_RESERVED_NAME_DEATH_PREFIX").expect("reserved prefix");
    let Some(name) = path.file_name() else {
        return;
    };
    if !name.to_string_lossy().starts_with(&prefix) {
        return;
    }
    let root = PathBuf::from(std::env::var_os("CSA_RESERVED_NAME_DEATH_ROOT").expect("death root"));
    std::fs::write(root.join("named-created"), []).unwrap();
    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}

#[cfg(unix)]
fn reserved_name_process_death_recovers(prefix: &str, setup: impl Fn(&Path)) {
    if let Some(root) = std::env::var_os("CSA_RESERVED_NAME_DEATH_ROOT") {
        let root = PathBuf::from(root);
        let _hook = ReservedLeafCreatedHook::set(mark_reserved_leaf_created);
        let _ = hermes_plan(&root.join("hermes-home"));
        return;
    }

    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::Builder::new()
        .prefix("hermes-reserved-death-")
        .tempdir_in("/var/tmp")
        .unwrap();
    let (_home, _env) = isolated_home(&temp);
    let hermes_home = temp.path().join("hermes-home");
    std::fs::create_dir_all(hermes_home.join("logs")).unwrap();
    setup(&hermes_home);
    let test_name = "isolation_plan::tests::hermes_review_3148_tests::sqlite_3148_tests::sqlite_3148_overlay_tests::reserved_name_process_death_recovers_absent_config";
    let test_name = match prefix {
        ".csa-absent-config.yaml-" => test_name,
        ".csa-absent-profiles-" => {
            "isolation_plan::tests::hermes_review_3148_tests::sqlite_3148_tests::sqlite_3148_overlay_tests::reserved_name_process_death_recovers_absent_profiles"
        }
        ".csa-write-probe-" => {
            "isolation_plan::tests::hermes_review_3148_tests::sqlite_3148_tests::sqlite_3148_overlay_tests::reserved_name_process_death_recovers_write_probe"
        }
        _ => panic!("unexpected reserved prefix {prefix}"),
    };
    let mut child = super::KillOnDropChild::new(
        std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg(test_name)
            .arg("--nocapture")
            .env("CSA_RESERVED_NAME_DEATH_ROOT", temp.path())
            .env("CSA_RESERVED_NAME_DEATH_PREFIX", prefix)
            .spawn()
            .unwrap(),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if temp.path().join("named-created").exists() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{prefix} child must pause after reserved name creation"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let visible = reserved_names(&hermes_home, prefix);
    assert!(
        !visible.is_empty(),
        "{prefix} reserved name must still be visible before kill: {visible:?}"
    );
    child.kill_and_reap();
    assert!(
        !reserved_names(&hermes_home, prefix).is_empty(),
        "{prefix} process death must leave the reserved name"
    );
    hermes_plan(&hermes_home)
        .unwrap_or_else(|error| panic!("{prefix} recovery preflight must succeed: {error:#}"));
    assert!(
        reserved_names(&hermes_home, prefix).is_empty(),
        "{prefix} recovery must remove the reserved name"
    );
}

#[cfg(unix)]
#[test]
fn reserved_name_process_death_recovers_absent_config() {
    reserved_name_process_death_recovers(".csa-absent-config.yaml-", |_| {});
}

#[cfg(unix)]
#[test]
fn reserved_name_process_death_recovers_absent_profiles() {
    reserved_name_process_death_recovers(".csa-absent-profiles-", |_| {});
}

#[cfg(unix)]
#[test]
fn reserved_name_process_death_recovers_write_probe() {
    reserved_name_process_death_recovers(".csa-write-probe-", |hermes_home| {
        std::fs::write(hermes_home.join("config.yaml"), "model: test\n").unwrap();
        std::fs::create_dir(hermes_home.join("profiles")).unwrap();
    });
}

#[cfg(unix)]
fn wait_for_marker(path: &Path, deadline: std::time::Instant, label: &str) {
    loop {
        if path.exists() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{label} marker must appear: {}",
            path.display()
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[cfg(unix)]
#[test]
fn reserved_name_recovery_preserves_live_owner_then_recovers_after_death() {
    const PREFIX: &str = ".csa-absent-config.yaml-";
    const TEST_NAME: &str = "isolation_plan::tests::hermes_review_3148_tests::sqlite_3148_tests::sqlite_3148_overlay_tests::reserved_name_recovery_preserves_live_owner_then_recovers_after_death";
    if let Some(root) = std::env::var_os("CSA_RESERVED_NAME_LIVE_ROOT") {
        let root = PathBuf::from(root);
        match std::env::var("CSA_RESERVED_NAME_LIVE_ROLE").as_deref() {
            Ok("owner") => {
                let _hook = ReservedLeafCreatedHook::set(mark_reserved_leaf_created);
                let _ = hermes_plan(&root.join("hermes-home"));
            }
            Ok("recoverer") => {
                let release = root.join("recover-release");
                wait_for_marker(
                    &release,
                    std::time::Instant::now() + std::time::Duration::from_secs(5),
                    "recoverer release",
                );
                std::fs::write(root.join("recoverer-entered"), []).unwrap();
                hermes_plan(&root.join("hermes-home"))
                    .unwrap_or_else(|error| panic!("recoverer preflight must succeed: {error:#}"));
                std::fs::write(root.join("recoverer-done"), []).unwrap();
            }
            other => panic!("unexpected live reserved-name role: {other:?}"),
        }
        return;
    }

    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::Builder::new()
        .prefix("hermes-reserved-live-owner-")
        .tempdir_in("/var/tmp")
        .unwrap();
    let (_home, _env) = isolated_home(&temp);
    let hermes_home = temp.path().join("hermes-home");
    std::fs::create_dir_all(hermes_home.join("logs")).unwrap();
    let mut owner = super::KillOnDropChild::new(
        std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg(TEST_NAME)
            .arg("--nocapture")
            .env("CSA_RESERVED_NAME_LIVE_ROOT", temp.path())
            .env("CSA_RESERVED_NAME_LIVE_ROLE", "owner")
            .env("CSA_RESERVED_NAME_DEATH_ROOT", temp.path())
            .env("CSA_RESERVED_NAME_DEATH_PREFIX", PREFIX)
            .spawn()
            .unwrap(),
    );
    wait_for_marker(
        &temp.path().join("named-created"),
        std::time::Instant::now() + std::time::Duration::from_secs(5),
        "owner reserved name",
    );
    assert!(
        !reserved_names(&hermes_home, PREFIX).is_empty(),
        "live owner reserved name must be visible"
    );

    let mut recoverer = super::KillOnDropChild::new(
        std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg(TEST_NAME)
            .arg("--nocapture")
            .env("CSA_RESERVED_NAME_LIVE_ROOT", temp.path())
            .env("CSA_RESERVED_NAME_LIVE_ROLE", "recoverer")
            .spawn()
            .unwrap(),
    );
    std::fs::write(temp.path().join("recover-release"), []).unwrap();
    wait_for_marker(
        &temp.path().join("recoverer-entered"),
        std::time::Instant::now() + std::time::Duration::from_secs(5),
        "recoverer entered",
    );
    let steal_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while std::time::Instant::now() < steal_deadline {
        assert!(
            !temp.path().join("recoverer-done").exists(),
            "recovery must not finish while a live owner still holds the reserved name"
        );
        assert!(
            !reserved_names(&hermes_home, PREFIX).is_empty(),
            "recovery must not unlink a live owner's reserved name"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    owner.kill_and_reap();
    wait_for_marker(
        &temp.path().join("recoverer-done"),
        std::time::Instant::now() + std::time::Duration::from_secs(5),
        "recoverer done after owner death",
    );
    recoverer.kill_and_reap();
    assert!(
        reserved_names(&hermes_home, PREFIX).is_empty(),
        "recovery after process death must remove the reserved name"
    );
    hermes_plan(&hermes_home)
        .unwrap_or_else(|error| panic!("subsequent preflight must succeed: {error:#}"));
}
