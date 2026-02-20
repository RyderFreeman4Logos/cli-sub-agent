---
name = "migrate"
description = "Run and manage CSA project migrations — apply pending changes, check status, add new migrations"
allowed-tools = "Bash, Read, Grep, Glob"
tier = "tier-1-quick"
version = "0.1.0"
---

# CSA Migrate

Apply pending project migrations and manage the migration lifecycle.

## Step 1: Check Migration Status

Tool: bash
OnFail: abort

```bash
csa migrate --status
```

## Step 2: Apply Pending Migrations

Tool: bash
OnFail: abort

If status shows pending migrations, apply them:

```bash
csa migrate
```

## Step 3: Verify weave.lock

Tool: bash

Confirm weave.lock was updated and versions match the binary:

```bash
cat weave.lock
```

---

# Adding New Migrations

## Migration Definition Format

Each migration is a `Migration` struct registered in `csa-config/src/migrate.rs`.

### Required Fields

| Field | Type | Description |
|-------|------|-------------|
| `id` | `String` | Unique identifier: `"{version}-{description}"` (e.g., `"0.2.0-rename-config"`) |
| `from_version` | `Version` | Minimum version this migration applies from |
| `to_version` | `Version` | Version after migration is applied |
| `description` | `String` | Human-readable summary |
| `steps` | `Vec<MigrationStep>` | Ordered steps to execute |

### Step Types

```rust
enum MigrationStep {
    // Rename a file relative to project root.
    RenameFile { from: PathBuf, to: PathBuf },

    // Replace all occurrences of a string in a file.
    ReplaceInFile { path: PathBuf, old: String, new: String },

    // Custom migration logic.
    Custom { label: String, apply: Box<dyn Fn(&Path) -> Result<()> + Send + Sync> },
}
```

### Template: Adding a New Migration

```rust
// In crates/csa-config/src/migrate.rs

fn my_new_migration() -> Migration {
    Migration {
        id: "0.2.0-describe-change".to_string(),
        from_version: Version::new(0, 1, 2),
        to_version: Version::new(0, 2, 0),
        description: "What this migration does".to_string(),
        steps: vec![
            MigrationStep::ReplaceInFile {
                path: PathBuf::from(".csa/config.toml"),
                old: "old_key".to_string(),
                new: "new_key".to_string(),
            },
        ],
    }
}

// Then register in default_registry():
pub fn default_registry() -> MigrationRegistry {
    let mut r = MigrationRegistry::new();
    r.register(plan_to_workflow_migration());
    r.register(my_new_migration()); // <-- add here
    r
}
```

## Version Numbering Scheme

- **Patch** (0.1.x → 0.1.y): No migrations needed. weave.lock auto-updates.
- **Minor** (0.x.0 → 0.y.0): May include migrations. `csa migrate` required.
- **Major** (x.0.0 → y.0.0): Breaking changes. Migrations mandatory.

Migration IDs follow: `{to_version}-{kebab-description}`

## Testing Requirements for New Migrations

Every migration MUST have:

1. **Unit test**: Verify the step transforms data correctly on a temp dir.
2. **Idempotency test**: Running twice produces the same result.
3. **No-op test**: Already-migrated state is unchanged.
4. **Integration test**: Full cycle — create project, apply migration, verify output.

See `crates/csa-config/src/migrate.rs` test section for examples.

## Rollback Strategy

Migrations are **forward-only**. Rollback is handled by:

1. **Git**: Revert the commit that introduced the migration.
2. **weave.lock**: Remove the migration ID from `migrations.applied`.
3. **Manual**: Reverse the file changes (rename back, restore content).

There is no automated rollback mechanism. This is intentional:
- Migrations are simple, declarative, and auditable.
- Forward-only avoids the complexity of bidirectional state management.
- Git provides the safety net for any revert scenario.

## Existing Migrations

| ID | From | To | Description |
|----|------|----|-------------|
| `0.1.2-plan-to-workflow` | 0.1.1 | 0.1.2 | Rename `[plan]` to `[workflow]` in workflow TOML files |
