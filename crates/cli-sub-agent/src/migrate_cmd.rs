//! Handler for the `csa migrate` subcommand.

use anyhow::{Context, Result};

/// Run all pending migrations and update weave.lock.
pub fn handle_migrate(dry_run: bool, status: bool) -> Result<()> {
    let project_dir = std::env::current_dir().context("cannot determine CWD")?;
    let csa_version = env!("CARGO_PKG_VERSION");
    let weave_version = env!("CARGO_PKG_VERSION");

    let registry = csa_config::MigrationRegistry::new();
    // TODO: register real migrations here as they are defined.

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
            eprintln!("Run any csa command to create weave.lock.");
        }
        Some(lock) => {
            let lock_version = &lock.versions.csa;
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

    let lock_version = lock.versions.csa.clone();
    let current: csa_config::Version = lock_version
        .parse()
        .with_context(|| format!("parsing lock version {lock_version:?}"))?;
    let target: csa_config::Version = csa_version
        .parse()
        .with_context(|| format!("parsing binary version {csa_version:?}"))?;

    let pending = registry.pending(&current, &target, &lock.migrations.applied);

    if pending.is_empty() {
        eprintln!("No pending migrations. Lock is up to date.");
        // Still update version stamp if different.
        if lock.versions.csa != csa_version {
            lock.versions.csa = csa_version.to_string();
            lock.versions.weave = weave_version.to_string();
            lock.save(project_dir)?;
            eprintln!("Updated weave.lock version to {csa_version}.");
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
    lock.versions.csa = csa_version.to_string();
    lock.versions.weave = weave_version.to_string();
    lock.save(project_dir)?;

    eprintln!(
        "All migrations applied. weave.lock updated to {csa_version}."
    );
    Ok(())
}
