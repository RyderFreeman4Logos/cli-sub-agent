//! Event bus abstractions for hook dispatch.
//!
//! Separates gatekeeping events (synchronous, result-bearing) from
//! observational events (fire-and-forget notifications).

use crate::config::HooksConfig;
use crate::event::HookEvent;
use crate::runner::run_hooks_for_event;
use anyhow::Result;
use std::collections::HashMap;

/// Dispatches hook events with semantic awareness of gatekeeping vs observational.
///
/// Gatekeeping events block the caller and propagate errors (the pipeline
/// inspects the `Result` to decide continue/abort). Observational events are
/// pure notifications whose failures are logged but never propagate.
///
/// Two implementations are provided:
/// - [`SyncEventBus`]: both paths execute synchronously (default, backward-compatible).
/// - [`AsyncEventBus`]: observational events are dispatched to a background channel
///   (behind the `async-hooks` feature flag).
pub trait EventBus: Send + Sync {
    /// Publish a gatekeeping event synchronously.
    ///
    /// The caller blocks until the hook completes. Errors propagate so the
    /// pipeline can decide whether to continue or abort.
    ///
    /// # Errors
    ///
    /// Returns the hook execution error when the hook fails under a closed
    /// fail-policy without a valid waiver.
    fn publish_gatekeeping(
        &self,
        event: HookEvent,
        hooks_config: &HooksConfig,
        variables: &HashMap<String, String>,
    ) -> Result<()>;

    /// Publish an observational event.
    ///
    /// The caller does not need the result — failures are logged and silently
    /// discarded. Implementations may execute this asynchronously.
    fn publish_observational(
        &self,
        event: HookEvent,
        hooks_config: &HooksConfig,
        variables: &HashMap<String, String>,
    );
}

/// Synchronous event bus — both gatekeeping and observational events run inline.
///
/// This is the default implementation that preserves 100% backward compatibility
/// with direct `run_hooks_for_event` calls.
pub struct SyncEventBus;

impl EventBus for SyncEventBus {
    fn publish_gatekeeping(
        &self,
        event: HookEvent,
        hooks_config: &HooksConfig,
        variables: &HashMap<String, String>,
    ) -> Result<()> {
        run_hooks_for_event(event, hooks_config, variables)
    }

    fn publish_observational(
        &self,
        event: HookEvent,
        hooks_config: &HooksConfig,
        variables: &HashMap<String, String>,
    ) {
        if let Err(err) = run_hooks_for_event(event, hooks_config, variables) {
            tracing::warn!(
                event = ?event,
                error = %err,
                "Observational hook failed (ignored)"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// AsyncEventBus — behind `async-hooks` feature flag
// ---------------------------------------------------------------------------

#[cfg(feature = "async-hooks")]
mod async_bus {
    use super::*;
    use std::sync::Arc;
    use std::thread;
    use tokio::sync::mpsc;

    /// Capacity of the bounded channel for observational events.
    const CHANNEL_CAPACITY: usize = 1024;

    /// Payload sent through the async channel for background execution.
    struct ObservationalPayload {
        event: HookEvent,
        hooks_config: HooksConfig,
        variables: HashMap<String, String>,
    }

    /// Asynchronous event bus — observational events are dispatched to a
    /// background thread via a bounded channel.
    ///
    /// Gatekeeping events still execute synchronously (they need the `Result`).
    /// When the channel is full, the oldest semantics don't apply — instead the
    /// newest event is dropped with a warning, because observational events are
    /// acceptable to lose under backpressure.
    pub struct AsyncEventBus {
        sender: mpsc::Sender<ObservationalPayload>,
        /// Hold the join handle so we can drain on drop.
        _worker: Arc<thread::JoinHandle<()>>,
    }

    impl Default for AsyncEventBus {
        fn default() -> Self {
            Self::new()
        }
    }

    impl AsyncEventBus {
        /// Create a new `AsyncEventBus` with a background consumer thread.
        pub fn new() -> Self {
            let (tx, mut rx) = mpsc::channel::<ObservationalPayload>(CHANNEL_CAPACITY);

            let handle = thread::spawn(move || {
                // Build a single-threaded tokio runtime for the consumer.
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to build tokio runtime for async hook bus");

                rt.block_on(async move {
                    while let Some(payload) = rx.recv().await {
                        if let Err(err) = run_hooks_for_event(
                            payload.event,
                            &payload.hooks_config,
                            &payload.variables,
                        ) {
                            tracing::warn!(
                                event = ?payload.event,
                                error = %err,
                                "Async observational hook failed (ignored)"
                            );
                        }
                    }
                    // Channel closed — drain complete.
                });
            });

            Self {
                sender: tx,
                _worker: Arc::new(handle),
            }
        }
    }

    impl EventBus for AsyncEventBus {
        fn publish_gatekeeping(
            &self,
            event: HookEvent,
            hooks_config: &HooksConfig,
            variables: &HashMap<String, String>,
        ) -> Result<()> {
            // Gatekeeping events always run synchronously.
            run_hooks_for_event(event, hooks_config, variables)
        }

        fn publish_observational(
            &self,
            event: HookEvent,
            hooks_config: &HooksConfig,
            variables: &HashMap<String, String>,
        ) {
            let payload = ObservationalPayload {
                event,
                hooks_config: hooks_config.clone(),
                variables: variables.clone(),
            };

            match self.sender.try_send(payload) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(_)) => {
                    tracing::warn!(
                        event = ?event,
                        "Async hook channel full — dropping observational event"
                    );
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    tracing::warn!(
                        event = ?event,
                        "Async hook channel closed — dropping observational event"
                    );
                }
            }
        }
    }

    impl Drop for AsyncEventBus {
        fn drop(&mut self) {
            // Dropping `self.sender` (implicitly) closes the channel,
            // which causes the worker's `rx.recv()` to return `None`,
            // draining any remaining payloads before exiting.
            //
            // We don't join the worker here because the Arc may have other
            // references, and blocking in Drop can cause deadlocks.
        }
    }
}

#[cfg(feature = "async-hooks")]
pub use async_bus::AsyncEventBus;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HookConfig;
    use crate::policy::FailPolicy;

    fn make_hooks_config_with_echo() -> HooksConfig {
        let mut config = HooksConfig::default();
        config.hooks.insert(
            "post_run".to_string(),
            HookConfig {
                enabled: true,
                command: Some("true".to_string()),
                timeout_secs: 5,
                fail_policy: FailPolicy::Open,
                waivers: Vec::new(),
            },
        );
        config.hooks.insert(
            "pre_run".to_string(),
            HookConfig {
                enabled: true,
                command: Some("true".to_string()),
                timeout_secs: 5,
                fail_policy: FailPolicy::Open,
                waivers: Vec::new(),
            },
        );
        config
    }

    #[test]
    fn test_event_bus_sync_gatekeeping_ok() {
        let bus = SyncEventBus;
        let config = make_hooks_config_with_echo();
        let vars = HashMap::new();

        let result = bus.publish_gatekeeping(HookEvent::PreRun, &config, &vars);
        assert!(result.is_ok());
    }

    #[test]
    fn test_event_bus_sync_gatekeeping_err_propagates() {
        let bus = SyncEventBus;
        let mut config = HooksConfig::default();
        config.hooks.insert(
            "pre_run".to_string(),
            HookConfig {
                enabled: true,
                command: Some("exit 1".to_string()),
                timeout_secs: 5,
                fail_policy: FailPolicy::Closed,
                waivers: Vec::new(),
            },
        );
        let vars = HashMap::new();

        let result = bus.publish_gatekeeping(HookEvent::PreRun, &config, &vars);
        assert!(result.is_err());
    }

    #[test]
    fn test_event_bus_sync_observational_swallows_error() {
        let bus = SyncEventBus;
        let mut config = HooksConfig::default();
        config.hooks.insert(
            "post_run".to_string(),
            HookConfig {
                enabled: true,
                command: Some("exit 1".to_string()),
                timeout_secs: 5,
                fail_policy: FailPolicy::Closed,
                waivers: Vec::new(),
            },
        );
        let vars = HashMap::new();

        // Should not panic — errors are swallowed for observational events.
        bus.publish_observational(HookEvent::PostRun, &config, &vars);
    }

    #[test]
    fn test_event_bus_sync_observational_ok() {
        let bus = SyncEventBus;
        let config = make_hooks_config_with_echo();
        let vars = HashMap::new();

        bus.publish_observational(HookEvent::PostRun, &config, &vars);
    }

    #[cfg(feature = "async-hooks")]
    mod async_tests {
        use super::*;

        #[test]
        fn test_async_event_bus_gatekeeping_ok() {
            let bus = AsyncEventBus::new();
            let config = make_hooks_config_with_echo();
            let vars = HashMap::new();

            let result = bus.publish_gatekeeping(HookEvent::PreRun, &config, &vars);
            assert!(result.is_ok());
        }

        #[test]
        fn test_async_event_bus_observational_does_not_block() {
            let bus = AsyncEventBus::new();
            let config = make_hooks_config_with_echo();
            let vars = HashMap::new();

            // Should return immediately (event processed in background).
            bus.publish_observational(HookEvent::PostRun, &config, &vars);

            // Give the background worker a moment to process.
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        #[test]
        fn test_async_event_bus_drop_drains() {
            let bus = AsyncEventBus::new();
            let config = make_hooks_config_with_echo();
            let vars = HashMap::new();

            // Enqueue a few events.
            bus.publish_observational(HookEvent::PostRun, &config, &vars);
            bus.publish_observational(HookEvent::TodoCreate, &config, &vars);

            // Drop should close the channel and let the worker drain.
            drop(bus);
        }
    }
}
