//! Stateful MCP server pooling with lease-based lifecycle management.
//!
//! Pools are keyed by `(project_root, toolchain_hash)` and maintain warm
//! instances with TTL-based expiry and pressure-driven reclamation.

use anyhow::{Result, anyhow};
use csa_config::McpServerConfig;
use rmcp::model::{CallToolRequestParam, CallToolResult, Tool};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use super::{
    DEFAULT_MAX_ACTIVE_POOLS, DEFAULT_MAX_WARM_POOLS, DEFAULT_WARM_TTL_SECS, PoolKey,
    ServerQueueHandle, ToolCallRoute,
};

pub(super) struct StatefulServerPool {
    pub(super) server_name: String,
    pub(super) config: McpServerConfig,
    pub(super) max_warm_pools: usize,
    pub(super) max_active_pools: usize,
    pub(super) inner: Mutex<StatefulPoolInner>,
}

pub(super) struct StatefulPoolInner {
    queues: HashMap<PoolKey, Arc<ServerQueueHandle>>,
    pub(super) leases: LeaseTracker,
}

impl StatefulServerPool {
    pub(super) fn new(config: McpServerConfig) -> Self {
        let warm_ttl = Duration::from_secs(DEFAULT_WARM_TTL_SECS);
        Self {
            server_name: config.name.clone(),
            config,
            max_warm_pools: DEFAULT_MAX_WARM_POOLS,
            max_active_pools: DEFAULT_MAX_ACTIVE_POOLS,
            inner: Mutex::new(StatefulPoolInner {
                queues: HashMap::new(),
                leases: LeaseTracker::new(warm_ttl),
            }),
        }
    }

    pub(super) async fn list_tools(&self, cancellation: CancellationToken) -> Result<Vec<Tool>> {
        let queue = self.default_queue().await;
        queue.list_tools(cancellation).await
    }

    async fn default_queue(&self) -> Arc<ServerQueueHandle> {
        let default_key = PoolKey {
            project_root: PathBuf::from("/"),
            toolchain_hash: 0,
        };

        let mut inner = self.inner.lock().await;
        if let Some(existing) = inner.queues.get(&default_key) {
            return existing.clone();
        }

        let queue = Arc::new(ServerQueueHandle::spawn(
            self.config.clone(),
            Some(default_key.clone()),
        ));
        inner.leases.acquire(&default_key, Instant::now());
        inner.leases.release(&default_key, Instant::now());
        inner.queues.insert(default_key, queue.clone());
        queue
    }

    pub(super) async fn call_tool(
        &self,
        request: CallToolRequestParam,
        route: ToolCallRoute,
        cancellation: CancellationToken,
    ) -> Result<CallToolResult> {
        let key = PoolKey::from_route(route)?;

        let (queue, reclaim_handles) = {
            let mut inner = self.inner.lock().await;
            let now = Instant::now();
            let mut reclaim_keys = inner.leases.expire(now);
            let mut reclaim_handles = Vec::new();

            if reclaim_keys.iter().any(|expired_key| expired_key == &key) {
                reclaim_keys.retain(|expired_key| expired_key != &key);
                if let Some(stale_queue) = inner.queues.remove(&key) {
                    reclaim_handles.push(stale_queue);
                }
            }

            let queue = if let Some(existing) = inner.queues.get(&key).cloned() {
                inner.leases.acquire(&key, now);
                existing
            } else {
                if inner.leases.active_pool_count() >= self.max_active_pools {
                    return Err(anyhow!(
                        "stateful MCP pool limit reached: max_active_pools={} server={}",
                        self.max_active_pools,
                        self.server_name
                    ));
                }

                let queue = Arc::new(ServerQueueHandle::spawn(
                    self.config.clone(),
                    Some(key.clone()),
                ));
                inner.queues.insert(key.clone(), queue.clone());
                inner.leases.acquire(&key, now);
                queue
            };

            let pool_count = inner.queues.len();
            reclaim_keys.extend(inner.leases.reclaim_for_pressure(
                pool_count,
                self.max_warm_pools,
                &key,
            ));

            reclaim_handles.extend(inner.take_handles(&reclaim_keys));
            Ok::<_, anyhow::Error>((queue, reclaim_handles))
        }?;

        for handle in reclaim_handles {
            let _ = handle.shutdown().await;
        }

        let call_result = queue.call_tool(request, cancellation).await;

        let expire_handles = {
            let mut inner = self.inner.lock().await;
            inner.leases.release(&key, Instant::now());
            let expire_keys = inner.leases.expire(Instant::now());
            inner.take_handles(&expire_keys)
        };

        for handle in expire_handles {
            let _ = handle.shutdown().await;
        }

        call_result
    }

    pub(super) async fn shutdown(&self) -> Result<()> {
        let handles = {
            let mut inner = self.inner.lock().await;
            inner.leases.clear();
            inner
                .queues
                .drain()
                .map(|(_, handle)| handle)
                .collect::<Vec<_>>()
        };

        for handle in handles {
            let _ = handle.shutdown().await;
        }

        Ok(())
    }
}

impl StatefulPoolInner {
    fn take_handles(&mut self, keys: &[PoolKey]) -> Vec<Arc<ServerQueueHandle>> {
        let mut handles = Vec::new();
        for key in keys {
            if let Some(handle) = self.queues.remove(key) {
                handles.push(handle);
            }
        }
        handles
    }
}

pub(super) struct LeaseTracker {
    pub(super) warm_ttl: Duration,
    leases: HashMap<PoolKey, LeaseState>,
}

#[derive(Clone, Copy)]
struct LeaseState {
    active_leases: usize,
    last_release: Instant,
}

impl LeaseTracker {
    pub(super) fn new(warm_ttl: Duration) -> Self {
        Self {
            warm_ttl,
            leases: HashMap::new(),
        }
    }

    pub(super) fn acquire(&mut self, key: &PoolKey, now: Instant) {
        let lease = self.leases.entry(key.clone()).or_insert(LeaseState {
            active_leases: 0,
            last_release: now,
        });
        lease.active_leases = lease.active_leases.saturating_add(1);
    }

    pub(super) fn release(&mut self, key: &PoolKey, now: Instant) {
        if let Some(lease) = self.leases.get_mut(key) {
            if lease.active_leases > 0 {
                lease.active_leases -= 1;
            }
            if lease.active_leases == 0 {
                lease.last_release = now;
            }
        }
    }

    pub(super) fn active_pool_count(&self) -> usize {
        self.leases
            .values()
            .filter(|lease| lease.active_leases > 0)
            .count()
    }

    #[cfg(test)]
    pub(super) fn active_leases(&self, key: &PoolKey) -> usize {
        self.leases
            .get(key)
            .map(|lease| lease.active_leases)
            .unwrap_or_default()
    }

    pub(super) fn expire(&mut self, now: Instant) -> Vec<PoolKey> {
        let expired = self
            .leases
            .iter()
            .filter_map(|(key, lease)| {
                if lease.active_leases == 0
                    && now.saturating_duration_since(lease.last_release) >= self.warm_ttl
                {
                    Some(key.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        for key in &expired {
            self.leases.remove(key);
        }

        expired
    }

    pub(super) fn reclaim_for_pressure(
        &mut self,
        pool_count: usize,
        max_warm_pools: usize,
        protected_key: &PoolKey,
    ) -> Vec<PoolKey> {
        if pool_count <= max_warm_pools {
            return Vec::new();
        }

        let mut candidates = self
            .leases
            .iter()
            .filter_map(|(key, lease)| {
                if key == protected_key || lease.active_leases > 0 {
                    return None;
                }
                Some((key.clone(), lease.last_release))
            })
            .collect::<Vec<_>>();

        candidates.sort_by_key(|(_, last_release)| *last_release);

        let reclaim_count = pool_count.saturating_sub(max_warm_pools);
        let reclaimed = candidates
            .into_iter()
            .take(reclaim_count)
            .map(|(key, _)| key)
            .collect::<Vec<_>>();

        for key in &reclaimed {
            self.leases.remove(key);
        }

        reclaimed
    }

    pub(super) fn clear(&mut self) {
        self.leases.clear();
    }
}
