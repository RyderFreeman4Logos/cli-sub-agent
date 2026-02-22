# Resource Control

CSA implements multi-layer resource isolation and memory-aware scheduling
to prevent OOM kills and system instability when running AI tools.

## Overview

AI CLI tools can consume significant memory (1-3 GB per instance). CSA
prevents resource exhaustion through:

1. **Pre-flight checks** -- verify sufficient memory before launching
2. **P95 estimation** -- use historical data to predict memory needs
3. **Resource sandbox** -- cgroup/rlimit isolation per tool process
4. **Peak tracking** -- monitor and record actual memory usage
5. **Global concurrency slots** -- limit concurrent tool instances

## Three-Layer Sandbox

CSA uses a defense-in-depth approach with three isolation mechanisms:

| Layer | Mechanism | Isolation | Availability |
|-------|-----------|-----------|-------------|
| 1 | cgroup v2 (systemd user scope) | Memory + PID limits | Linux with systemd |
| 2 | `setrlimit` (RLIMIT_AS, NPROC) | Address space + process count | POSIX systems |
| 3 | RSS monitor (background thread) | Peak memory tracking | All platforms |

### Capability Detection

CSA probes the host at startup and caches the result:

```rust
pub enum SandboxCapability {
    CgroupV2,    // Best: cgroup v2 + systemd user scope
    Setrlimit,   // Fallback: POSIX setrlimit
    None,         // No isolation available
}
```

Detection order:
1. Check `/sys/fs/cgroup/cgroup.controllers` exists (cgroup v2 unified hierarchy)
2. Check `systemd-run --user --scope` is functional
3. Fall back to `setrlimit` if available

### Configuration

```toml
# ~/.config/cli-sub-agent/config.toml or .csa/config.toml

[resources]
enforcement_mode = "BestEffort"   # Required | BestEffort | Off
memory_max_mb = 4096              # Per-tool memory limit
memory_swap_max_mb = 0            # Swap limit (cgroup only)
pids_max = 256                    # Process count limit

# Per-tool overrides
[tools.codex.resources]
memory_max_mb = 3072
enforcement_mode = "Required"
```

### Enforcement Modes

| Mode | Behavior |
|------|----------|
| `Required` | Fail if preferred sandbox is unavailable |
| `BestEffort` | Use best available mechanism, warn if degraded |
| `Off` | Disable sandbox entirely |

### cgroup v2 Scope Guard

When cgroup v2 is available, CSA creates a systemd transient scope for
each tool process:

- RAII cleanup via `CgroupScopeGuard` (scope removed on drop)
- Memory limits enforced by the kernel
- PID limits prevent fork bombs
- Orphan cleanup: `cleanup_orphan_scopes()` removes stale `csa-*.scope` units

### setrlimit Fallback

When cgroup is unavailable, CSA uses `pre_exec` to set:

- `RLIMIT_AS` -- virtual address space limit
- `RLIMIT_NPROC` -- max processes per user

Combined with `setsid()` in a single `pre_exec` closure for atomicity.

## P95 Memory Estimation

### How it works

CSA maintains a rolling window of the last 20 peak memory measurements
per tool in `usage_stats.toml`:

```toml
[history]
gemini-cli = [1024, 1152, 1088, 1920, 1200, ...]
codex = [2048, 2304, 2176, 2560, ...]
```

The **P95 (95th percentile)** is used instead of the average because it:

- Accounts for occasional high-usage spikes
- Provides a conservative estimate
- Avoids skew from outliers

### Pre-flight Check

```
required = min_free_memory_mb + P95_estimate(tool)
available = physical_free + swap_free

if available < required:
    abort with OOM risk message
```

**Priority chain for estimates:**

1. P95 from historical data (if >= 1 run exists)
2. Initial estimate from config (`resources.initial_estimates`)
3. Hardcoded fallback: 500 MB

### Example

```
Tool: codex
P95 estimate: 2560 MB (from 20 runs)
min_free_memory_mb: 4096
Available: 8192 MB (physical: 6144 + swap: 2048)

Required: 4096 + 2560 = 6656 MB
Available: 8192 MB
6656 < 8192 -> PASS
```

## Memory Monitoring

### MemoryMonitor

The `csa-resource` crate provides `MemoryMonitor` for runtime tracking:

1. Get child process PID after spawn
2. Background task samples RSS every 500ms via `sysinfo` crate
3. Peak RSS tracked via `Arc<AtomicU64>` (lock-free)
4. Monitoring stops when process exits
5. Peak value recorded to `usage_stats.toml`

### Performance

| Metric | Value |
|--------|-------|
| CPU overhead | < 0.1% (1 syscall per 500ms) |
| Memory overhead | < 1 MB |
| Peak detection accuracy | +/- 500ms |

## Global Concurrency Slots

CSA limits how many instances of each tool can run simultaneously:

```toml
[tools.codex]
max_concurrent = 5

[tools.claude-code]
max_concurrent = 3
```

Implemented via `flock`-based slot files under
`~/.local/state/cli-sub-agent/slots/`:

```
slots/
  +-- codex-0.lock
  +-- codex-1.lock
  +-- codex-2.lock
  +-- ...
```

When all slots are occupied:

- Default: fail with "no slots available"
- `--wait`: block until a slot becomes free

## Usage Statistics

### Storage

**Path:** `~/.local/state/csa/{project_path}/usage_stats.toml`

**Retention:** Last 20 records per tool (FIFO).

### Atomic Writes

Statistics are written atomically: write to `.tmp` file, then `rename()`
(POSIX atomic). This prevents corruption from concurrent writes.

### Initial Estimates

Until P95 data is available, CSA uses configured initial estimates:

| Tool | Recommended Initial (MB) |
|------|-------------------------|
| gemini-cli | 1024 |
| codex | 2048 |
| opencode | 1536 |
| claude-code | 2048 |

## ResourceGuard API

```rust
use csa_resource::{ResourceGuard, ResourceLimits};

// Create guard
let mut guard = ResourceGuard::new(limits, &stats_path);

// Pre-flight check
guard.check_availability("codex")?;

// After process completes
let peak_mb = monitor.stop().await;
guard.record_usage("codex", peak_mb);
```

## Integration Points

### Pipeline Sandbox

`pipeline_sandbox.rs` in `cli-sub-agent` calls `resolve_sandbox_options()`
to determine sandbox configuration for each tool execution. Telemetry
is recorded via `SandboxInfo` in session state.

### ACP Sandbox

`AcpConnection::spawn_sandboxed()` applies resource isolation to ACP
processes, combining cgroup/rlimit enforcement with ACP session management.

## Troubleshooting

| Problem | Solution |
|---------|----------|
| Frequent OOM prevention errors | Lower `min_free_memory_mb` or close other apps |
| Tool crashes despite passing pre-flight | Expected for P5 of runs; estimates will adapt |
| Initial runs always fail | Lower `initial_estimates` in config |
| `usage_stats.toml` corrupt | Delete it (will regenerate; loses history) |
| Memory shows 0 MB peak | Process exited before first 500ms sample |
| "No sandbox capability" warning | Install systemd or ensure setrlimit is available |

## Related

- [Configuration](configuration.md) -- `[resources]` section reference
- [Architecture](architecture.md) -- process model and isolation
- [ACP Transport](acp-transport.md) -- ACP sandbox integration
