# Batch 1 Summary

## Commits
- `754c3cc3` fix(csa-session): retire dead active wait sessions (#738)
- `0c73b885` fix(csa-session): surface unpushed session recovery steps (#747)
- `b2514cf1` fix(cli-sub-agent): skip executor publish transactions (#782)

## Per-issue detail

### #738
- **Root cause**: `csa session wait` could observe a dead session's `daemon-completion.toml` without reconciling the result into a terminal phase, leaving the session stuck in `Active`.
- **Fix**: `session wait` now refreshes or synthesizes the result on daemon completion and retires the dead session before returning failure.
- **Files touched**: `crates/cli-sub-agent/src/session_cmds_daemon.rs`, `crates/cli-sub-agent/src/session_cmds_tests.rs`, `crates/cli-sub-agent/src/session_cmds_tests_tail_wait.rs`
- **Regression tests**: `cargo test -p cli-sub-agent handle_session_wait_retires_active_session_after_dead_failure_completion_packet`

### #747
- **Root cause**: dead-session reconciliation discarded the fact that a session branch could already be ahead of origin, so Layer 0 had no durable hint that partial progress still needed publishing.
- **Fix**: reconciliation now writes `output/unpushed_commits.json` for ahead branches, and `session wait` emits a `CSA:NEXT_STEP` directive using that recovery command.
- **Files touched**: `crates/cli-sub-agent/src/session_cmds_reconcile.rs`, `crates/cli-sub-agent/src/session_cmds_daemon.rs`, `crates/cli-sub-agent/src/session_cmds_tests.rs`, `crates/cli-sub-agent/src/session_cmds_reconcile_tests_tail.rs`, `crates/cli-sub-agent/src/session_cmds_tests_tail_recovery.rs`
- **Regression tests**: `cargo test -p cli-sub-agent ensure_terminal_result_for_dead_active_session_writes_unpushed_commit_sidecar`; `cargo test -p cli-sub-agent synthesized_wait_next_step_returns_directive_for_unpushed_commit_recovery`

### #782
- **Root cause**: executor-mode commit workflows did not fully short-circuit the publish transaction, so internal sessions could still reach `git push` paths that depend on outer-layer review state.
- **Fix**: the commit workflow now gates both publish-variable bridging and the auto-PR transaction on executor-mode env detection, with docs and a shell regression test kept in sync.
- **Files touched**: `patterns/commit/PATTERN.md`, `patterns/commit/skills/commit/SKILL.md`, `patterns/commit/workflow.toml`, `crates/cli-sub-agent/src/plan_cmd_tests_commit.rs`, `Cargo.toml`, `Cargo.lock`, `weave.lock`, `output/summary.md`
- **Regression tests**: `cargo test -p cli-sub-agent commit_workflow_auto_pr_step_exits_before_push_in_executor_mode`

## Blockers encountered
- Repository pre-commit auto-stages tracked Rust files after `cargo fmt`, so issue-scoped commits required temporarily stashing unrelated tracked `.rs` changes before each commit.
- Workspace version bump files (`Cargo.toml`, `Cargo.lock`, `weave.lock`) were required by the repository gate `just check-version-bumped`; they were included with `#782` so the branch stays gate-clean.
