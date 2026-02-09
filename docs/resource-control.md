# Resource Control

CSA implements memory-aware scheduling to prevent OOM (Out Of Memory) kills and system instability when launching AI tools.

## Overview

AI CLI tools (especially those running large language models) can consume significant memory. CSA prevents resource exhaustion by:

1. **Pre-flight Checks:** Verify sufficient memory before launching tools
2. **Historical Estimation:** Use P95 statistics from past runs
3. **Peak Tracking:** Monitor and record actual memory usage
4. **Adaptive Learning:** Improve estimates over time

## Resource Monitoring Architecture

```
┌─────────────────────────────────────────────────┐
│         User invokes: csa run "task"            │
└────────────────┬────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────┐
│  Load usage_stats.toml (P95 estimates)          │
└────────────────┬────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────┐
│  Check Available Memory (sysinfo crate)         │
│  - available_memory()                           │
│  - free_swap()                                  │
└────────────────┬────────────────────────────────┘
                 │
                 ▼
    ┌────────────┴────────────┐
    │ Enough memory?          │
    └─────┬────────────────┬──┘
          │                │
         YES              NO
          │                │
          │                ▼
          │   ┌─────────────────────────────┐
          │   │ Abort with OOM risk warning │
          │   └─────────────────────────────┘
          │
          ▼
┌─────────────────────────────────────────────────┐
│  Spawn Tool Process                             │
│  - Get PID                                      │
└────────────────┬────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────┐
│  Monitor Memory Usage (MemoryMonitor)           │
│  - Sample process RSS every 500ms               │
│  - Track peak memory                            │
└────────────────┬────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────┐
│  Wait for Process Completion                    │
└────────────────┬────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────┐
│  Record Peak Memory (usage_stats.toml)          │
│  - Update P95 estimate                          │
│  - Keep last 20 records                         │
└─────────────────────────────────────────────────┘
```

## P95 Memory Estimation

### What is P95?

**P95 (95th Percentile):** The value below which 95% of observations fall.

**Why P95 instead of average?**
- Averages are skewed by outliers
- P95 provides conservative estimate
- Accounts for occasional high usage spikes

**Example:**

```
Last 20 runs of gemini-cli:
[1024, 1152, 1088, 1920, 1200, 1184, 1216, 1088, 1344, 1152,
 1024, 1136, 1408, 1216, 1184, 1280, 1152, 1088, 1216, 1184]

Sorted:
[1024, 1024, 1088, 1088, 1088, 1136, 1152, 1152, 1152, 1184,
 1184, 1184, 1200, 1216, 1216, 1216, 1280, 1344, 1408, 1920]

P95 calculation:
- Total count: 20
- P95 index: ceil(20 × 0.95) = 19
- P95 value: sorted[18] = 1408 MB  (0-indexed)
```

**Result:** CSA will reserve 1408 MB for gemini-cli instead of average (1222 MB)

### Algorithm Implementation

```rust
pub fn get_p95_estimate(&self, tool: &str) -> Option<u64> {
    let records = self.history.get(tool)?;
    if records.is_empty() {
        return None;
    }

    let mut sorted = records.clone();
    sorted.sort_unstable();

    let idx = ((sorted.len() as f64) * 0.95).ceil() as usize;
    let idx = idx.min(sorted.len()).saturating_sub(1);
    Some(sorted[idx])
}
```

**Edge Cases:**
- **Single record:** Returns that value (100th percentile)
- **Empty history:** Returns `None` (falls back to initial estimate)
- **Fewer than 20 records:** Still calculates P95 from available data

## Pre-Flight Resource Check

### Check Formula

```
required_memory = min_free_memory_mb + P95_estimate(tool)
available_memory = (physical_free + swap_free) / 1024 / 1024  # bytes → MB

if available_memory < required_memory:
    abort with OOM risk message
```

**Components:**

1. **`min_free_memory_mb`:** Safety buffer to keep system stable (default: 4096 MB, combined physical + swap)
2. **`P95_estimate(tool)`:** Historical memory usage or initial estimate
3. **`available_memory`:** Current combined free memory (physical RAM + swap) reported by OS

### Example Calculation

**Configuration:**
```toml
[resources]
min_free_memory_mb = 4096

[resources.initial_estimates]
codex = 2048
```

**Scenario:**
- Tool: `codex`
- Historical P95: 2560 MB (from 20 runs)
- Available memory: 8192 MB (physical: 6144 MB + swap: 2048 MB)

**Check:**
```
required = 4096 + 2560 = 6656 MB
available = 6144 + 2048 = 8192 MB
6656 < 8192 → PASS ✓
```

**Scenario 2 (Low Memory):**
- Available memory: 5000 MB (physical: 3500 MB + swap: 1500 MB)

**Check:**
```
required = 4096 + 2560 = 6656 MB
available = 3500 + 1500 = 5000 MB
6656 > 5000 → FAIL ✗

Error: OOM Risk Prevention: Not enough memory to launch 'codex'.
Available: 5000 MB (physical 3500 + swap 1500), Min Buffer: 4096 MB, Est. Tool Usage: 2560 MB (P95)
(Try closing other apps or wait for running agents to finish)
```

### Combined Memory Check

**Single Check:**

The memory check now uses combined physical + swap free memory as a single threshold, providing a more accurate representation of available system resources.

**Rationale:** Checking physical and swap separately can be misleading. The system uses both resources fluidly, so a combined check better reflects actual memory availability.

## Memory Monitoring

### MemoryMonitor Implementation

**Crate:** `csa-resource/monitor.rs`

**Mechanism:**
1. Get child process PID
2. Start background task to sample memory every 500ms
3. Track peak RSS (Resident Set Size)
4. Stop monitoring when process exits

**Pseudo-code:**

```rust
pub struct MemoryMonitor {
    peak_memory_mb: Arc<AtomicU64>,
    handle: JoinHandle<()>,
}

impl MemoryMonitor {
    pub async fn start(pid: u32) -> Self {
        let peak = Arc::new(AtomicU64::new(0));
        let peak_clone = peak.clone();

        let handle = tokio::spawn(async move {
            let mut sys = System::new();
            loop {
                sys.refresh_process(pid);
                if let Some(process) = sys.process(pid) {
                    let rss_mb = process.memory() / 1024 / 1024;
                    peak_clone.fetch_max(rss_mb, Ordering::Relaxed);
                } else {
                    break;  // Process exited
                }
                sleep(Duration::from_millis(500)).await;
            }
        });

        Self { peak_memory_mb: peak, handle }
    }

    pub async fn stop(self) -> u64 {
        self.handle.await;
        self.peak_memory_mb.load(Ordering::Relaxed)
    }
}
```

**Sampling Interval:** 500ms (configurable in code)

**Memory Metric:** RSS (Resident Set Size) - actual physical memory used

**Concurrency:** Uses `Arc<AtomicU64>` for lock-free peak tracking

## Usage Statistics Storage

### File Format

**Path:** `~/.local/state/csa/{project_path}/usage_stats.toml`

**Schema:**

```toml
[history]
gemini-cli = [1024, 1152, 1088, 1920, 1200, 1184, 1216, 1088, 1344, 1152, 1024, 1136, 1408, 1216, 1184, 1280, 1152, 1088, 1216, 1184]
codex = [2048, 2304, 2176, 2560, 2432, 2240, 2368, 2176, 2496, 2240, 2048, 2208, 2624, 2368, 2304, 2496, 2240, 2176, 2368, 2304]
opencode = [1536, 1792, 1664, ...]
claude-code = [2048, 2304, ...]
```

**Field:** `history.<tool_name>`

**Type:** Array of integers (memory in MB)

**Retention:** Last 20 records per tool

**Oldest Entry Removal:**

```rust
pub fn record(&mut self, tool: &str, usage_mb: u64) {
    let entry = self.history.entry(tool.to_string()).or_default();
    entry.push(usage_mb);
    if entry.len() > 20 {
        entry.remove(0);  // Remove oldest
    }
}
```

**Example Progression:**

```
Initial (0 records):
  history.codex = []

After 1 run:
  history.codex = [2048]

After 20 runs:
  history.codex = [2048, 2304, ..., 2560]  (20 entries)

After 21 runs:
  history.codex = [2304, ..., 2560, 2432]  (20 entries, oldest removed)
```

### Atomic Writes

**Purpose:** Prevent corruption during concurrent writes

**Implementation:**

```rust
pub fn save(&self, stats_path: &Path) -> Result<()> {
    let content = toml::to_string_pretty(self)?;

    // Write to temporary file
    let tmp_path = stats_path.with_extension("tmp");
    std::fs::write(&tmp_path, content)?;

    // Atomic rename
    std::fs::rename(&tmp_path, stats_path)?;

    Ok(())
}
```

**Atomicity Guarantee:**
- `rename()` is atomic on POSIX systems
- Partial writes never visible
- Safe for concurrent reads/writes

### Initialization

**First Run (No History):**

```rust
pub fn load(stats_path: &Path) -> Result<Self> {
    if !stats_path.exists() {
        return Ok(Self::default());  // Empty history
    }
    let content = std::fs::read_to_string(stats_path)?;
    Ok(toml::from_str(&content)?)
}
```

**Fallback Behavior:**

```rust
let estimated_usage = self
    .stats
    .get_p95_estimate(tool_name)
    .unwrap_or_else(|| {
        *self.limits.initial_estimates.get(tool_name).unwrap_or(&500)
    });
```

**Priority:**
1. P95 from historical data (if ≥1 run)
2. Initial estimate from config
3. Hardcoded fallback: 500 MB

## ResourceGuard API

### Creating a Guard

```rust
use csa_resource::{ResourceGuard, ResourceLimits};

let limits = ResourceLimits {
    min_free_memory_mb: 4096, // combined physical + swap threshold
    initial_estimates: [
        ("gemini-cli".to_string(), 1024),
        ("codex".to_string(), 2048),
    ].iter().cloned().collect(),
};

let stats_path = session_root.join("usage_stats.toml");
let mut guard = ResourceGuard::new(limits, &stats_path);
```

### Check Availability

```rust
// Before launching tool
guard.check_availability("codex")?;  // Fails if insufficient memory
```

**Error Example:**

```
Error: OOM Risk Prevention: Not enough memory to launch 'codex'.
Available: 3072 MB, Min Buffer: 2048 MB, Est. Tool Usage: 2560 MB (P95)
(Try closing other apps or wait for running agents to finish)
```

### Record Usage

```rust
// After process completes
let peak_memory_mb = monitor.stop().await;
guard.record_usage("codex", peak_memory_mb);
```

**Side Effect:** Updates `usage_stats.toml` atomically

## Configuration

### Resource Section

```toml
[resources]
min_free_memory_mb = 4096      # Default: 4096 (combined physical + swap)

[resources.initial_estimates]
gemini-cli = 1024    # MB
codex = 2048
opencode = 1536
claude-code = 2048
```

### Tuning Guidelines

**Conservative (Low-Memory Systems):**
```toml
[resources]
min_free_memory_mb = 6144      # Keep more buffer (combined physical + swap)
```

**Aggressive (High-Memory Systems):**
```toml
[resources]
min_free_memory_mb = 2048      # Allow tighter margins (combined physical + swap)
```

**Initial Estimates:**
- Set based on tool documentation
- Will be overridden by P95 after 20+ runs
- Conservative initial values prevent early OOM

**Recommended Initial Estimates:**

| Tool | Typical Range | Recommended Initial |
|------|---------------|---------------------|
| gemini-cli | 800-1500 MB | 1024 MB |
| codex | 1500-3000 MB | 2048 MB |
| opencode | 1200-2000 MB | 1536 MB |
| claude-code | 1500-3000 MB | 2048 MB |

## Memory Monitoring Lifecycle

### Full Example

```rust
// 1. Pre-flight check
let mut guard = ResourceGuard::new(limits, &stats_path);
guard.check_availability("codex")?;

// 2. Spawn process
let mut child = spawn_tool(cmd).await?;
let pid = child.id().expect("No PID");

// 3. Start monitoring
let monitor = MemoryMonitor::start(pid).await;

// 4. Wait for completion
let result = wait_and_capture(child).await?;

// 5. Stop monitoring and get peak
let peak_memory_mb = monitor.stop().await;

// 6. Record usage
guard.record_usage("codex", peak_memory_mb);

// 7. Usage stats updated for next run
```

### Monitoring Overhead

**CPU:** Negligible (1 syscall per 500ms)

**Memory:** < 1 MB (background task + atomic counter)

**Accuracy:** ±500ms latency (may miss very short spikes)

## OOM Prevention Strategies

### 1. Pre-Flight Checks (Current)

**Pros:**
- Fast (no overhead during execution)
- Simple implementation
- Prevents most OOM scenarios

**Cons:**
- Cannot prevent OOM from other processes starting during execution
- Relies on historical data (cold start issues)

### 2. Runtime Limits (Future Enhancement)

**Possible Extensions:**
- `cgroups` memory limits (Linux)
- `ulimit` resource limits (POSIX)
- Kill tool if exceeds threshold

**Trade-offs:**
- More complex implementation
- May kill tools during legitimate spikes
- Requires root or special permissions

### 3. Quota System (Future Enhancement)

**Concept:**
- Total memory quota for all CSA processes
- Serialize tool launches if quota exceeded
- Queue management for pending tasks

**Use Case:** Multi-user systems or CI environments

## Best Practices

1. **Monitor historical data:** Check `usage_stats.toml` periodically to understand patterns
2. **Adjust initial estimates:** Update after observing actual usage
3. **Leave headroom:** Set `min_free_memory_mb` to 20-30% of total RAM
4. **Close other apps:** When launching memory-intensive tools
5. **Use swap wisely:** Don't rely on swap for primary memory (thrashing)
6. **Review outliers:** Investigate runs that significantly exceed P95

## Troubleshooting

**Problem:** Frequent OOM prevention errors despite sufficient total RAM

**Cause:** Other applications consuming memory

**Solution:** Increase `min_free_memory_mb` or close other apps before running CSA

---

**Problem:** Tool crashes with OOM despite passing pre-flight check

**Cause:** Memory usage spiked beyond P95 estimate

**Solution:** This is expected for P5 of runs. Historical data will update to reflect new peak.

---

**Problem:** Initial runs always fail with OOM prevention

**Cause:** Initial estimates too high for your system

**Solution:** Lower `initial_estimates` in config based on available RAM

---

**Problem:** P95 estimate seems too low after many runs

**Cause:** Recent runs used less memory (e.g., smaller tasks)

**Solution:** P95 adapts to recent 20 runs. Consider manual override if needed.

---

**Problem:** `usage_stats.toml` corrupt or missing

**Cause:** Concurrent writes or filesystem issue

**Solution:** Delete file (will regenerate). Loss of historical data, but no functional impact.

---

**Problem:** Memory monitoring shows 0 MB peak usage

**Cause:** Process exited before first sample (500ms)

**Solution:** Increase sampling frequency in code, or ignore for very short tasks

## Performance Characteristics

**Pre-flight Check:**
- Time: < 50ms (sysinfo query)
- Blocking: Yes (prevents tool launch if insufficient)

**Memory Monitoring:**
- Overhead: < 0.1% CPU
- Sampling: Every 500ms
- Accuracy: ±500ms for peak detection

**Stats Recording:**
- Time: < 10ms (atomic file write)
- Blocking: No (async task)
- Storage: ~10 KB per project

## Future Enhancements

1. **Configurable Sampling Interval:** Allow tuning via config
2. **Memory Pressure Warnings:** Warn when approaching system limits
3. **Multi-Tool Coordination:** Account for concurrent CSA processes
4. **Per-Session Limits:** Enforce memory quotas per session tree
5. **OOM Recovery:** Automatic retry with compression after OOM kill
6. **Predictive Scheduling:** Estimate task memory needs from prompt analysis
