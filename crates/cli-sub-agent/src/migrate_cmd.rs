//! Handler for the `csa migrate` subcommand.

use anyhow::{Context, Result};

/// Run all pending migrations and update weave.lock.
pub fn handle_migrate(dry_run: bool, status: bool) -> Result<()> {
    let project_dir = std::env::current_dir().context("cannot determine CWD")?;
    let csa_version = env!("CARGO_PKG_VERSION");
    let weave_version = env!("CARGO_PKG_VERSION");

    let registry = csa_config::default_registry();

    if status {
        return print_status(&project_dir, csa_version, &registry);
    }

    run_migrations(&project_dir, csa_version, weave_version, &registry, dry_run)
}

fn print_status(
    project_dir: &std::path::Path,
    csa_version: &str,
    registry: &csa_config::MigrationRegistry,
) -> Result<()> {
    let lock = csa_config::WeaveLock::load(project_dir)?;

    match lock {
        None => {
            eprintln!("No weave.lock found. Current binary version: {csa_version}");
            eprintln!("Run `csa migrate` to initialize weave.lock.");
        }
        Some(lock) => {
            let Some(versions) = lock.versions() else {
                eprintln!("weave.lock exists but has no [versions] section.");
                eprintln!("Binary version: {csa_version}");
                eprintln!("Run `csa migrate` to initialize version tracking.");
                return Ok(());
            };
            let lock_version = &versions.csa;
            let current: csa_config::Version = lock_version
                .parse()
                .with_context(|| format!("parsing lock version {lock_version:?}"))?;
            let target: csa_config::Version = csa_version
                .parse()
                .with_context(|| format!("parsing binary version {csa_version:?}"))?;

            let pending = registry.pending(&current, &target, &lock.migrations.applied);

            eprintln!("Lock version:   {lock_version}");
            eprintln!("Binary version: {csa_version}");
            eprintln!("Applied migrations: {}", lock.migrations.applied.len());
            eprintln!("Pending migrations: {}", pending.len());

            for m in &pending {
                eprintln!("  - {} ({})", m.id, m.description);
            }
        }
    }

    Ok(())
}

fn run_migrations(
    project_dir: &std::path::Path,
    csa_version: &str,
    weave_version: &str,
    registry: &csa_config::MigrationRegistry,
    dry_run: bool,
) -> Result<()> {
    let mut lock = csa_config::WeaveLock::load_or_init(project_dir, csa_version, weave_version)?;
    let had_versions_before = lock.versions().is_some();

    let lock_version = lock
        .versions_or_init(csa_version, weave_version)
        .csa
        .clone();
    let current: csa_config::Version = lock_version
        .parse()
        .with_context(|| format!("parsing lock version {lock_version:?}"))?;
    let target: csa_config::Version = csa_version
        .parse()
        .with_context(|| format!("parsing binary version {csa_version:?}"))?;

    let pending = registry.pending(&current, &target, &lock.migrations.applied);

    if pending.is_empty() {
        eprintln!("No pending migrations. Lock is up to date.");
        if sync_version_stamp(&mut lock, had_versions_before, csa_version, weave_version) {
            lock.save(project_dir)?;
            eprintln!(
                "Updated weave.lock version stamp(s) to csa {csa_version}, weave {weave_version}."
            );
        }
        return Ok(());
    }

    eprintln!("{} pending migration(s):", pending.len());
    for m in &pending {
        eprintln!("  - {} ({})", m.id, m.description);
    }

    if dry_run {
        eprintln!("(dry-run: no changes applied)");
        return Ok(());
    }

    for m in &pending {
        eprintln!("Applying: {} ...", m.id);
        csa_config::migrate::execute_migration(m, project_dir)
            .with_context(|| format!("applying migration {}", m.id))?;
        lock.record_migration(&m.id);
        eprintln!("  done.");
    }

    // Update version in lock after all migrations.
    sync_version_stamp(&mut lock, had_versions_before, csa_version, weave_version);
    lock.save(project_dir)?;

    eprintln!(
        "All migrations applied. weave.lock updated to csa {csa_version}, weave {weave_version}."
    );
    Ok(())
}

fn sync_version_stamp(
    lock: &mut csa_config::WeaveLock,
    had_versions_before: bool,
    csa_version: &str,
    weave_version: &str,
) -> bool {
    let versions = lock.versions_or_init(csa_version, weave_version);
    let changed =
        !had_versions_before || versions.csa != csa_version || versions.weave != weave_version;
    if changed {
        versions.csa = csa_version.to_string();
        versions.weave = weave_version.to_string();
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_version_stamp_updates_weave_only_drift() {
        let mut lock = csa_config::WeaveLock::new("0.1.1", "0.1.0");

        let changed = sync_version_stamp(&mut lock, true, "0.1.1", "0.1.1");

        assert!(changed);
        let versions = lock.versions().unwrap();
        assert_eq!(versions.csa, "0.1.1");
        assert_eq!(versions.weave, "0.1.1");
    }

    #[test]
    fn sync_version_stamp_initializes_missing_versions() {
        let mut lock = csa_config::WeaveLock {
            versions: None,
            migrations: Default::default(),
            package: Vec::new(),
        };

        let changed = sync_version_stamp(&mut lock, false, "0.1.1", "0.1.1");

        assert!(changed);
        let versions = lock.versions().unwrap();
        assert_eq!(versions.csa, "0.1.1");
        assert_eq!(versions.weave, "0.1.1");
    }
}
