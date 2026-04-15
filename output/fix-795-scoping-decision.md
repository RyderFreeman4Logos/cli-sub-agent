## Scoping Decision

Selected fix: Option A, but scoped to the `csa review` execution boundary only.

Why this option:
- The user-facing failure is specifically `csa review --range main...HEAD` leaving no `result.toml`, which blocks SA-guard callers.
- Existing code already synthesizes or persists terminal results in several generic paths:
  - pre-exec failures in `session_guard.rs`
  - transport failures in `pipeline_execute.rs`
  - post-exec failures in `pipeline_post_exec.rs`
  - dead-daemon reconciliation in `session_cmds_reconcile.rs`
- The remaining gap appears to be review-layer error propagation where a session ID exists in the error chain but no terminal `result.toml` was persisted.

Why not full global Option A in this change:
- A fully generic implementation would likely require broadening the shared session result schema and/or reconciliation surface across more than 3 files.
- That exceeds the requested tight-scope change discipline for this task.

Planned implementation:
- In `review_cmd_execute.rs`, if review execution returns an error with `meta_session_id=...` and the session has no `result.toml`, synthesize a minimal failure result there.
- The synthetic result will use the existing standard `SessionResult` schema so `csa session result` can read it immediately.
- The summary will include a compact classified failure reason plus a short stderr excerpt when available.

Expected touch surface:
- `crates/cli-sub-agent/src/review_cmd_execute.rs`
- `crates/cli-sub-agent/src/review_cmd_tests_tail.rs`
- `output/fix-795-summary.md`

Non-goals:
- No new config keys.
- No changes to generic session result rendering.
- No attempt to solve all non-review commands in this commit.
