# CSA Architecture Reference

These are condensed architecture summaries for CSA subsystems. For full details,
see the linked reference documents.

## Core Architecture

Fractal recursion (max depth 5 via `CSA_DEPTH`), ULID-based sessions in `~/.local/state/cli-sub-agent/`, TOML serialization, `flock(2)` locking, `setsid` + negative-PID signal propagation, two-phase termination (SIGTERM→SIGKILL). ACP transport (JSON-RPC 2.0 via stdio) for claude-code/codex; legacy CLI for gemini-cli/opencode. Context env vars: `CSA_SESSION_ID`, `CSA_DEPTH`, `CSA_PROJECT_ROOT`, `CSA_SESSION_DIR`. Auto-strips `CLAUDECODE`/`CLAUDE_CODE_ENTRYPOINT` on child spawn.
→ `.agents/project-rules-ref/architecture.md`

## MCP Hub

Shared daemon (`csa mcp-hub serve`) over Unix socket, fanning out to backend MCP servers. `BackendTransport`: Stdio + Http (rmcp). `McpTransport` tagged union (`stdio`/`http`/`sse`). HTTPS enforced, SSRF pre-flight DNS check. Per-server FIFO dispatch queue (capacity 64). Stateful pooling with `project_root`+`toolchain_hash` keys. Socket activation via systemd.
→ `.agents/project-rules-ref/mcp-hub.md`

## Resource Sandbox

Dual-axis isolation model: `ResourceCapability` (CgroupV2/Setrlimit/None) for memory/PID limits + `FilesystemCapability` (Bwrap/Landlock/None) for filesystem access control. `IsolationPlan` unifies both axes and covers all three spawn paths (ACP, legacy CLI, MCP hub). Resource axis: cgroup v2 scopes → setrlimit → OOM score adj fallback. Filesystem axis: bwrap (user namespace, best isolation) → Landlock LSM (kernel-level, no namespace needed) → none. `MemoryBalloon` pre-warms RAM via `mmap MAP_POPULATE`. `ResourceGuard::check_availability()`: `available_memory + available_swap >= reserve_mb` (4096MB default). Claude-code: `MemorySwapMax=0` prevents swap thrash. `cleanup_orphan_scopes()` removes stale `csa-*.scope` units. Config: `[filesystem_sandbox]` section in `.csa/config.toml` with `enforcement_mode` (required/best-effort/off) and `extra_writable` paths. CLI: `--no-fs-sandbox` flag disables filesystem isolation. `csa doctor` reports both resource and filesystem capability detection. Per-tool FS sandbox: `[tools.<name>.filesystem_sandbox]` with `writable_paths` (REPLACE semantics — project root becomes read-only) and `enforcement_mode` override. Path validation rejects paths outside project root, home dir, and `/tmp`. Review/debate: `readonly_sandbox` config makes project root read-only. EACCES diagnostics emit hints when sandbox causes permission errors.
→ `.agents/project-rules-ref/resource-sandbox.md`

## Session Management

`Genealogy` parent-child tracking. Two fork methods: `Native` (ACP session reload, saves 30-60K tokens) and `Soft` (context summary injection). `ReturnPacket` protocol for fork-call return. Fork rate limit: 10/minute per parent. `SessionPhase` state machine: Active↔Available↔Retired. Seed sessions for warm fork. Idle timeout → liveness poll → two-phase kill. Defaults: idle 250s, dead 600s, grace 5s. **Process lifetime**: ALL child processes (including `nohup`/`disown`) are killed on session end via process group kill + cgroup scope cleanup. To outlive CSA, start processes from the caller (`systemd-run --user --scope`).
→ `.agents/project-rules-ref/session-management.md`

## Debate & Review

Heterogeneous model selection via `ModelFamily` enum ensures cognitive diversity. Consensus: `majority`/`unanimous`/`weighted` via `agent-teams-rs`. `detect_rate_limit` + `decide_failover` for 429 errors. Pattern resolution from `patterns/csa-review/` and `patterns/debate/`. Four-value review verdict: `ReviewDecision` enum (Pass/Fail/Skip/Uncertain) with backward-compatible aliases (CLEAN→Pass, HAS_ISSUES→Fail). Multi-layer quality gates (`GateStep` pipeline: L1 lint → L2 type → L3 test) run before AI review. Contract Alignment dimension checks spec intent, boundary compliance, and completion criteria when `--spec` is provided.
→ `.agents/project-rules-ref/debate-review.md`

## VCS Abstraction

`VcsBackend` trait in `csa-core::vcs` abstracts git and jj (Jujutsu) operations. `detect_vcs_kind()` auto-detects: `.jj/` → Jj, `.git/` → Git. Concrete implementations `GitBackend`/`JjBackend` in `csa-session::vcs_backends`. Session creation auto-detects `change_id` (jj change-id or git HEAD). `--spec` flag on `csa run` and `csa review` for agent-spec contract files (`.toml` or `.spec` extension).

## TODO References

Progressive disclosure: TODO.md summary + `references/` detail files with `index.toml`. Token estimation: <32KB → `chars/3`, ≥32KB → tiktoken-rs. Transcript import via `xurl` (7 providers) with redaction pipeline. CLI: `csa todo ref list|show|add|import-transcript`.
→ `.agents/project-rules-ref/todo-references.md`

## Workspace Architecture

14-crate Cargo workspace in 7 layers. L0: `csa-core` (domain types), `csa-lock` (flock). L1: `csa-config`, `csa-resource`. L2: `csa-session`, `csa-scheduler`, `csa-todo`. L3: `csa-acp`, `csa-process`. L4: `csa-executor`, `csa-hooks`. L5: `csa-mcp-hub`, `weave`. L6: `cli-sub-agent` (binary).
→ `.agents/project-rules-ref/workspace.md`

## Code Style & Conventions

Rust edition 2024, rust-version 1.88. `anyhow` (app) + `thiserror` (lib). `tokio` async, `tracing` logging, `serde` serialization, ULID sessions. Sum types (Enum with data) over dynamic dispatch for closed sets. 800-line soft module limit. Every `unsafe` has `// SAFETY:` comment. `cc-sdk` opt-in, `rmcp-sdk` default on.
→ `.agents/project-rules-ref/code-style.md`

## Configuration

Project: `.csa/config.toml` (project metadata, tiers, resources, MCP). Global: `~/.config/cli-sub-agent/config.toml` (API keys, concurrency, defaults). Project overrides global via merge. Key sections: `[tools]`, `[tiers]`, `[resources]`, `[review]`, `[debate]`, `[session]`, `[mcp_servers]`, `[setting_sources]`, `[pr_review]`, `[gc]`.
→ `.agents/project-rules-ref/config.md`

**tier-routing** — When `[tiers]` are configured, direct `--tool`/`--model`/`--thinking` is blocked by default across `csa run`, `csa review`, and `csa debate`. Callers must use `--tier <name>` to select a tier by name, which resolves tool/model/thinking from the tier definition. Override with `--force-ignore-tier-setting` (alias: `--force-tier`) to bypass. The existing `--force` flag also bypasses tier enforcement. When no tiers are configured (empty `[tiers]`), all existing behavior is preserved. Priority chain: CLI `--tier` > config tier > CLI `--tool` (with force) > config tool > auto-select.

## Patterns & Skills

Patterns in `patterns/` (weave packages). Skills in `.claude/skills/` (SKILL.md + .skill.toml). **MANDATORY**: every pattern needs companion skill symlink; pattern changes MUST pass `weave compile`; workflow variables MUST sync between PATTERN.md and workflow.toml (PR #257: 17 review rounds from orphaned variable). **dev2merge** is the primary deterministic pipeline (`csa plan run patterns/dev2merge/workflow.toml`): branch validation → FAST_PATH detection → L1/L2 gates → mktd → mktsk → cumulative review → push gate → PR → pr-bot → merge. All steps enforced via `on_fail = "abort"` and git pre-push hook (`scripts/hooks/pre-push`).
→ `.agents/project-rules-ref/patterns-skills.md`

## Implementation Status

Version 0.1.54. 4 tools (claude-code, codex, gemini-cli, opencode). ACP + legacy transport. Native fork (claude-code), soft fork (others), codex PTY fork behind feature gate. Fork-call-return protocol. Resource sandbox. MCP hub. Seed sessions. Structured output sections. TODO/workflow with spec intent flow. Config at `.csa/config.toml` + `~/.config/cli-sub-agent/config.toml`.
→ `.agents/project-rules-ref/implementation-status.md`
