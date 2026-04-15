<!-- CSA:SECTION:summary -->
Fixed #742 by forcing `csa review` sessions to build into the off-repo session state directory instead of any inherited repo-local `CARGO_TARGET_DIR`. Added `.target-review-*/` to `.gitignore` as defense in depth.
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
- Changed `crates/cli-sub-agent/src/pipeline_session_exec.rs` so `task_type=review` always injects `CARGO_TARGET_DIR=$CSA_SESSION_DIR/target`.
- This keeps review build artifacts under `~/.local/state/cli-sub-agent/.../sessions/<id>/target/`, matching other per-session state and avoiding Rule 037 violations in the repo root.
- Non-review commands keep their existing environment behavior.
- Added unit coverage for both the review override and the non-review no-op path.
- Added `.target-review-*/` to `.gitignore`. `/.tmp/` was already present.

Manual cleanup for existing repo-root artifacts:

```bash
rm -rf .target-review-1 .target-review-2 .target-review-serial
```
<!-- CSA:SECTION:details:END -->
