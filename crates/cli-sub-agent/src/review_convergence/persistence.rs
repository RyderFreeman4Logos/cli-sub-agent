use std::time::{Duration, Instant};

use csa_session::convergence::{CampaignRecord, ConvergenceEvent, ConvergenceLedger};

use super::engine::{EngineError, LedgerPort, blocked};

pub(super) fn persist<S: LedgerPort>(
    store: &S,
    campaign: &CampaignRecord,
    event: ConvergenceEvent,
    ledger: &mut ConvergenceLedger,
    elapsed: &mut Duration,
    calls: usize,
) -> std::result::Result<(), EngineError> {
    persist_batch(store, campaign, vec![event], ledger, elapsed, calls)
}

pub(super) fn persist_batch<S: LedgerPort>(
    store: &S,
    campaign: &CampaignRecord,
    events: Vec<ConvergenceEvent>,
    ledger: &mut ConvergenceLedger,
    elapsed: &mut Duration,
    calls: usize,
) -> std::result::Result<(), EngineError> {
    let started = Instant::now();
    store
        .append_batch(campaign.id().clone(), events)
        .map_err(|error| blocked("store_failure", format!("{error:#}"), calls))?;
    *ledger = store
        .load()
        .map_err(|error| blocked("store_failure", format!("{error:#}"), calls))?;
    *elapsed += started.elapsed();
    Ok(())
}
