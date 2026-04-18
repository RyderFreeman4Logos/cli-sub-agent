Changed files:
- [crates/cli-sub-agent/src/run_cmd_fork.rs](/home/obj/project/github/RyderFreeman4Logos/cli-sub-agent/crates/cli-sub-agent/src/run_cmd_fork.rs:298): native fork pre-create now uses `create_session_fresh()` instead of daemon-aware `create_session()`, so fork children always get a fresh CSA ULID even when `CSA_DAEMON_SESSION_ID` is set in the daemon process.
- [crates/cli-sub-agent/src/run_cmd_fork.rs](/home/obj/project/github/RyderFreeman4Logos/cli-sub-agent/crates/cli-sub-agent/src/run_cmd_fork.rs:406): added regression test `pre_created_native_fork_session_ignores_daemon_parent_session_id`, which simulates the daemon env leak, asserts child/parent session IDs differ, runs `cleanup_pre_created_fork_session()`, and checks the parent session directory/state still exist while only the child is removed.
- [Cargo.toml](/home/obj/project/github/RyderFreeman4Logos/cli-sub-agent/Cargo.toml:5) and [Cargo.lock](/home/obj/project/github/RyderFreeman4Logos/cli-sub-agent/Cargo.lock:533): patch bump `0.1.335 -> 0.1.336`.

Failure path traced:
- Parent daemon env is seeded in [session_cmds_daemon.rs](/home/obj/project/github/RyderFreeman4Logos/cli-sub-agent/crates/cli-sub-agent/src/session_cmds_daemon.rs:85).
- `create_session()` in [crates/csa-session/src/manager.rs](/home/obj/project/github/RyderFreeman4Logos/cli-sub-agent/crates/csa-session/src/manager.rs:114) consults `preassigned_daemon_session_id()` and will reuse that ULID in daemon context.
- Native fork pre-create previously called that API from [run_cmd_fork.rs](/home/obj/project/github/RyderFreeman4Logos/cli-sub-agent/crates/cli-sub-agent/src/run_cmd_fork.rs:298).
- On fork failure/failover, [cleanup_pre_created_fork_session()](/home/obj/project/github/RyderFreeman4Logos/cli-sub-agent/crates/cli-sub-agent/src/run_cmd_fork.rs:390) deletes the recorded pre-created session ID. If that ID equals the parent, the parent session directory disappears silently.

Tests and verification:
- `cargo test -p cli-sub-agent pre_created_native_fork_session_ignores_daemon_parent_session_id`
- `just pre-commit`
- `just test`

Follow-up:
- I did not add extra wait-path diagnostics because the root cause is concrete and fixed at source. If desired later, a separate hardening pass could still improve `session wait ... not found` hints for already-lost historical sessions.
