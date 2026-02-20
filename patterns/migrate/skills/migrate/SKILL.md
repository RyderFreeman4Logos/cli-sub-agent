---
name: migrate
description: Run CSA project migrations to update config and workflow files to the current version
allowed-tools: Bash, Read, Grep, Glob
triggers:
  - "migrate"
  - "/migrate"
  - "csa migrate"
  - "run migrations"
---

# Migrate: CSA Project Migration Tool

## Purpose

Apply pending migrations to bring project configuration and workflow files
up to date with the current CSA binary version. Manages the weave.lock
file that tracks applied migrations.

## Quick Reference

| Command | Description |
|---------|-------------|
| `csa migrate` | Apply all pending migrations |
| `csa migrate --dry-run` | Preview without applying |
| `csa migrate --status` | Show migration status |

## When to Use

- After upgrading CSA to a new version
- When `csa` prints "weave.lock is outdated" at startup
- When setting up a project cloned from another environment

## Execution Protocol

1. Run `csa migrate --status` to check current state
2. If migrations are pending, run `csa migrate`
3. Verify `weave.lock` was updated
4. Commit the updated `weave.lock` and any migrated files

## Adding New Migrations

Read the pattern at `patterns/migrate/PATTERN.md` for the full migration
definition format, version numbering scheme, testing requirements, and
code templates.
