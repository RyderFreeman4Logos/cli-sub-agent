## Fix #795 Summary

Selected sub-fix: scoped Option A for `csa review`.

What changed:
- Added a review-layer fallback in `crates/cli-sub-agent/src/review_cmd_execute.rs`.
- When review execution fails with a `meta_session_id` but no persisted `result.toml`, CSA now synthesizes a minimal terminal `SessionResult`.
- The fallback classifies the failure into:
  - `timeout`
  - `signal`
  - `spawn_fail`
  - `tool_crash`
- The synthesized summary prefers a short `stderr.log` excerpt so SA-guard callers get structured failure output without reading raw logs directly.

Why this scope:
- The generic pipeline already handles many pre-exec, transport, post-exec, and dead-daemon reconciliation cases.
- The remaining user-visible gap was review-specific error propagation that could escape without a terminal result file.
- This keeps the fix tight while directly addressing the blocked `csa review --range main...HEAD` caller flow from #795.

Regression coverage:
- Added a test that creates a review session with no `result.toml`, injects a review failure carrying `meta_session_id=...`, synthesizes the fallback result, and verifies `handle_session_result(...)` can read it.

Verification run:
- `cargo test -p cli-sub-agent synthesize_missing_review_result_makes_session_result_readable`
- `cargo test -p cli-sub-agent review_cmd::execute::tests`
- `cargo test -p csa-session`
- `cargo test -p csa-executor`
- `cargo fmt --all`
- `just pre-commit`
