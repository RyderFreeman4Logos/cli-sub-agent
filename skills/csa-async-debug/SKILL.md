---
name: csa-async-debug
description: Expert diagnosis for Tokio/async Rust issues including deadlocks, task leaks, performance bottlenecks, and cancellation safety
allowed-tools: Bash, Read, Grep, Glob
---

# Async/Tokio Debugging Guide

Expert diagnosis for Tokio/async Rust issues: deadlocks, task leaks, performance bottlenecks, and cancellation safety.

## Common Async Anti-Patterns

### 1. Blocking the Runtime
```rust
// WRONG: Blocking in async context
async fn bad() {
    std::thread::sleep(Duration::from_secs(1)); // Blocks entire thread
    std::fs::read_to_string("file");            // Sync I/O blocks
}

// CORRECT
async fn good() {
    tokio::time::sleep(Duration::from_secs(1)).await;
    tokio::fs::read_to_string("file").await;
    tokio::task::spawn_blocking(|| expensive_cpu_work()).await;
}
```

### 2. Deadlocks (Lock Held Across Await)
```rust
// WRONG: Holding lock across await
async fn deadlock() {
    let lock = mutex.lock().await;
    some_async_fn().await; // Holding lock!
    drop(lock);
}

// CORRECT: Release lock before await
async fn safe() {
    let data = {
        let lock = mutex.lock().await;
        lock.clone()
    }; // Lock released here
    some_async_fn().await;
}
```

### 3. Task Leaks
```rust
// WRONG: Spawned task not tracked
fn leak() {
    tokio::spawn(async { ... }); // Lost handle = leak
}

// CORRECT: Use JoinSet for multiple tasks
let mut set = JoinSet::new();
set.spawn(task1());
set.spawn(task2());
while let Some(result) = set.join_next().await { ... }
```

### 4. Cancellation Unsafety
```rust
// WRONG: Cancellation corrupts state
async fn unsafe_cancel() {
    file.write_all(header).await?;
    // If cancelled here, file is incomplete!
    file.write_all(body).await?;
}

// CORRECT: Use atomic operations
async fn safe_cancel() {
    let temp = create_temp_file().await?;
    temp.write_all(data).await?;
    temp.rename(target).await?; // Atomic rename
}
```

## Diagnosis Tools

### Tokio-Console
```bash
# Cargo.toml: console-subscriber = "0.2"
# main.rs: console_subscriber::init();
# Run: RUSTFLAGS="--cfg tokio_unstable" cargo run
# Monitor: tokio-console
```

### Runtime Metrics
```rust
let metrics = tokio::runtime::Handle::current().metrics();
println!("Active tasks: {}", metrics.active_tasks_count());
println!("Blocking threads: {}", metrics.num_blocking_threads());
```

## Debug Process

1. **Collect Evidence**: Error logs, tokio-console output, timing info
2. **Locate Problem**: Which await point hangs? Which blocking call?
3. **Analyze Root Cause**: Deadlock graph, cancellation paths, resource contention
4. **Verify Fix**: Stress test with high concurrency, long-running tests

## Checklist for Async Code

- [ ] No sync I/O in async functions
- [ ] No `std::thread::sleep()` (use `tokio::time::sleep().await`)
- [ ] Locks released before every `.await`
- [ ] All `JoinHandle`s tracked and awaited
- [ ] `select!` branches are cancellation-safe
- [ ] CPU-intensive work in `spawn_blocking()`
- [ ] No unbounded task spawning
- [ ] Resource limits on concurrent operations
