//! Read-only campaign and environment inputs for production completion.

use std::collections::BTreeMap;

use anyhow::{Context, Result, anyhow, bail};
use csa_process::ProviderTurnCompletion;
use csa_session::convergence::{
    CampaignId, CommandAuthoritySnapshot, ConvergenceEvent, ConvergenceLedger,
    ProviderTurnReservation,
};

use super::completion_types::{ProviderTurnEvidence, ProviderTurnReconciliation};

pub(super) fn campaign_record<'a>(
    ledger: &'a ConvergenceLedger,
    campaign_id: &CampaignId,
) -> Result<&'a csa_session::convergence::CampaignRecord> {
    ledger
        .entries()
        .iter()
        .filter(|entry| entry.campaign_id() == campaign_id)
        .find_map(|entry| match entry.event() {
            ConvergenceEvent::CampaignStarted(record) => Some(record),
            _ => None,
        })
        .context("clustered campaign start record is missing")
}

pub(super) fn allowed_provider_environment(
    authority: &CommandAuthoritySnapshot,
) -> Result<BTreeMap<String, String>> {
    let selected = authority
        .ordered_admitted()
        .first()
        .context("completion authority has no admitted executor")?;
    let credential = match (selected.tool(), selected.provider()) {
        ("codex" | "opencode", "openai") => "OPENAI_API_KEY",
        ("opencode", "anthropic") => "ANTHROPIC_API_KEY",
        _ => bail!("unsupported clean-room provider identity"),
    };
    let keys = [
        "PATH",
        credential,
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "LANG",
        "LC_ALL",
        "NO_PROXY",
        "OPENSSL_CERT_DIR",
        "OPENSSL_CERT_FILE",
        "SSL_CERT_DIR",
        "SSL_CERT_FILE",
        "http_proxy",
        "https_proxy",
        "no_proxy",
    ];
    let mut captured = BTreeMap::new();
    for key in keys {
        if let Some(value) = std::env::var_os(key) {
            captured.insert(
                key.to_string(),
                value
                    .into_string()
                    .map_err(|_| anyhow!("provider environment value is not UTF-8"))?,
            );
        }
    }
    Ok(captured)
}

pub(super) fn reconciliation_from_completion(
    reservation: ProviderTurnReservation,
    completion: ProviderTurnCompletion,
) -> ProviderTurnReconciliation {
    let evidence = match completion {
        ProviderTurnCompletion::Unknown => ProviderTurnEvidence::ConfirmedExecutionFallback,
        observed => ProviderTurnEvidence::Transport(observed),
    };
    ProviderTurnReconciliation::Reconciled {
        reservation,
        host_observed_turn_delta: 1,
        evidence,
    }
}
