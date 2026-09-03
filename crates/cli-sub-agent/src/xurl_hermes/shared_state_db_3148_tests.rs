use std::collections::HashMap;
use std::path::{Path, PathBuf};

use csa_resource::filesystem_sandbox::FilesystemCapability;
use csa_resource::isolation_plan::{EnforcementMode, IsolationPlan, IsolationPlanBuilder};

use super::db;
use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};

fn hermes_plan(hermes_home: &Path) -> IsolationPlan {
    let execution_env = HashMap::from([(
        "HERMES_HOME".to_string(),
        hermes_home.to_string_lossy().into_owned(),
    )]);
    IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_execution_env(Some(&execution_env))
        .with_tool_defaults(
            "hermes",
            Path::new("/tmp/project"),
            Path::new("/tmp/session"),
        )
        .build()
        .expect("Hermes sandbox plan must build")
}

fn sandbox_runtime_home(plan: &IsolationPlan, hermes_home: &Path) -> PathBuf {
    plan.readable_paths
        .iter()
        .find(|path| path.requested() == hermes_home)
        .map(|path| path.bind_source().to_path_buf())
        .expect("sandboxed Hermes home must have a pinned bind source")
}

fn write_live_state_db(path: &Path, value: &str) {
    let conn = rusqlite::Connection::open(path).expect("create live state db");
    conn.execute_batch("CREATE TABLE values_table (value TEXT NOT NULL);")
        .expect("create state table");
    conn.execute(
        "INSERT INTO values_table (value) VALUES (?1)",
        rusqlite::params![value],
    )
    .expect("write state value");
}

fn latest_state_value(path: &Path) -> String {
    rusqlite::Connection::open(path)
        .expect("open state db")
        .query_row(
            "SELECT value FROM values_table ORDER BY rowid DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .expect("read state value")
}

#[cfg(unix)]
#[test]
fn sandbox_start_restore_threads_and_recall_share_physical_state_db_including_profile() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let temp = tempfile::Builder::new()
        .prefix("hermes-shared-state-db-")
        .tempdir_in("/var/tmp")
        .expect("tempdir");
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).expect("create isolated HOME");
    let _home = ScopedEnvVarRestore::set("HOME", &home);
    let _hermes_home = ScopedEnvVarRestore::unset("HERMES_HOME");

    let hermes_home = temp.path().join("hermes");
    std::fs::create_dir_all(hermes_home.join("logs")).expect("create Hermes logs");
    std::fs::create_dir_all(hermes_home.join("work")).expect("create legacy profile");
    std::fs::write(hermes_home.join("config.yaml"), "model: test\n").expect("write config");
    write_live_state_db(&hermes_home.join("state.db"), "legacy-root");
    write_live_state_db(&hermes_home.join("work/state.db"), "legacy-profile");

    let start = hermes_plan(&hermes_home);
    let start_runtime = sandbox_runtime_home(&start, &hermes_home);
    let profile_db = start_runtime.join("work").join("state.db");
    assert_eq!(latest_state_value(&profile_db), "legacy-profile");

    let restore = hermes_plan(&hermes_home);
    let restore_runtime = sandbox_runtime_home(&restore, &hermes_home);
    let restore_db = restore_runtime.join("work").join("state.db");

    let threads = db::resolve_paths(None, Some(&hermes_home), Some("work"))
        .expect("threads must resolve Hermes state.db");
    let recall = db::resolve_paths(None, Some(&hermes_home), Some("work"))
        .expect("recall must resolve Hermes state.db");

    assert_eq!(
        start_runtime, restore_runtime,
        "sandbox start and restore must bind the same runtime home"
    );
    assert_eq!(
        restore_db, profile_db,
        "restore must keep the sandbox profile database"
    );
    assert_eq!(
        threads.state_db, profile_db,
        "threads must open the sandbox profile database"
    );
    assert_eq!(
        recall.state_db, profile_db,
        "recall must open the sandbox profile database"
    );
    assert!(
        hermes_home.join("state.db").is_file(),
        "legacy state.db must remain"
    );
    assert_eq!(
        latest_state_value(&hermes_home.join("work/state.db")),
        "legacy-profile"
    );
}
