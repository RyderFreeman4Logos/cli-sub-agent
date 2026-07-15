use crate::convergence::{
    AdmittedModelIdentity, CampaignId, CampaignRecord, CommandAuthorityCatalogIdentity,
    CommandAuthorityPolicy, CommandAuthoritySnapshot, CommandAuthoritySource, Sha256Digest,
};
use chrono::{TimeZone, Utc};

fn admitted(tool: &str, provider: &str, model: &str, reasoning: &str) -> AdmittedModelIdentity {
    AdmittedModelIdentity::new(tool, provider, model, reasoning).unwrap()
}

fn snapshot(
    admitted_models: Vec<AdmittedModelIdentity>,
    preference_order: &[&str],
    fallback_enabled: bool,
    force_ignore: bool,
    no_failover: bool,
) -> CommandAuthoritySnapshot {
    CommandAuthoritySnapshot::new(
        CommandAuthoritySource::tier("tier-4-critical", "review.tier").unwrap(),
        CommandAuthorityPolicy::new(
            fallback_enabled,
            preference_order.iter().map(ToString::to_string).collect(),
            force_ignore,
            no_failover,
        )
        .unwrap(),
        CommandAuthorityCatalogIdentity::new("merged:model-catalog.toml", "catalog-v7").unwrap(),
        admitted_models,
    )
    .unwrap()
}

#[test]
fn command_authority_digest_is_stable_order_sensitive_and_admission_complete() {
    let first = admitted("codex", "openai", "gpt-5.6", "xhigh");
    let second = admitted("claude-code", "anthropic", "claude-opus-4-6", "high");
    let authority = snapshot(
        vec![first.clone(), second.clone()],
        &["codex", "claude-code"],
        true,
        false,
        false,
    );

    let round_trip: CommandAuthoritySnapshot =
        serde_json::from_str(&serde_json::to_string(&authority).unwrap()).unwrap();
    assert_eq!(authority.digest(), round_trip.digest());
    assert_eq!(authority, round_trip);
    assert!(authority.contains(&first));
    assert!(authority.contains(&second));

    let reordered = snapshot(
        vec![second, first],
        &["codex", "claude-code"],
        true,
        false,
        false,
    );
    assert_ne!(authority.digest(), reordered.digest());
}

#[test]
fn command_authority_digest_changes_for_every_execution_authority_dimension() {
    let base_model = admitted("codex", "openai", "gpt-5.6", "xhigh");
    let base = snapshot(
        vec![base_model.clone()],
        &["codex", "claude-code"],
        true,
        false,
        false,
    );

    for changed in [
        snapshot(
            vec![admitted("claude-code", "openai", "gpt-5.6", "xhigh")],
            &["codex", "claude-code"],
            true,
            false,
            false,
        ),
        snapshot(
            vec![admitted("codex", "azure", "gpt-5.6", "xhigh")],
            &["codex", "claude-code"],
            true,
            false,
            false,
        ),
        snapshot(
            vec![admitted("codex", "openai", "gpt-5.5", "xhigh")],
            &["codex", "claude-code"],
            true,
            false,
            false,
        ),
        snapshot(
            vec![admitted("codex", "openai", "gpt-5.6", "high")],
            &["codex", "claude-code"],
            true,
            false,
            false,
        ),
        snapshot(
            vec![base_model.clone()],
            &["claude-code", "codex"],
            true,
            false,
            false,
        ),
        snapshot(
            vec![base_model.clone()],
            &["codex", "claude-code"],
            false,
            false,
            false,
        ),
        snapshot(
            vec![base_model.clone()],
            &["codex", "claude-code"],
            true,
            true,
            false,
        ),
        snapshot(
            vec![base_model],
            &["codex", "claude-code"],
            true,
            false,
            true,
        ),
    ] {
        assert_ne!(base.digest(), changed.digest());
    }
}

#[test]
fn command_authority_same_display_tier_with_different_admission_never_collides() {
    let openai = snapshot(
        vec![admitted("codex", "openai", "gpt-5.6", "xhigh")],
        &["codex"],
        false,
        false,
        true,
    );
    let anthropic = snapshot(
        vec![admitted(
            "claude-code",
            "anthropic",
            "claude-opus-4-6",
            "xhigh",
        )],
        &["codex"],
        false,
        false,
        true,
    );

    assert_eq!(openai.source(), anthropic.source());
    assert_ne!(openai.digest(), anthropic.digest());
}

#[test]
fn command_authority_catalog_and_source_identity_are_digest_authority() {
    let admitted_models = vec![admitted("codex", "openai", "gpt-5.6", "xhigh")];
    let base = snapshot(admitted_models.clone(), &["codex"], false, false, true);
    let changed_tier = CommandAuthoritySnapshot::new(
        CommandAuthoritySource::tier("quality", "review.tier").unwrap(),
        base.policy().clone(),
        base.catalog().clone(),
        admitted_models.clone(),
    )
    .unwrap();
    let changed_source = CommandAuthoritySnapshot::new(
        CommandAuthoritySource::tier("tier-4-critical", "project.review.tier").unwrap(),
        base.policy().clone(),
        base.catalog().clone(),
        admitted_models.clone(),
    )
    .unwrap();
    let changed_catalog_source = CommandAuthoritySnapshot::new(
        base.source().clone(),
        base.policy().clone(),
        CommandAuthorityCatalogIdentity::new("shipped:model-catalog.toml", "catalog-v7").unwrap(),
        admitted_models.clone(),
    )
    .unwrap();
    let changed_catalog_version = CommandAuthoritySnapshot::new(
        base.source().clone(),
        base.policy().clone(),
        CommandAuthorityCatalogIdentity::new("merged:model-catalog.toml", "catalog-v8").unwrap(),
        admitted_models,
    )
    .unwrap();

    for changed in [
        changed_tier,
        changed_source,
        changed_catalog_source,
        changed_catalog_version,
    ] {
        assert_ne!(base.digest(), changed.digest());
    }
}

#[test]
fn command_authority_rejects_ambiguous_or_empty_authority() {
    let model = admitted("codex", "openai", "gpt-5.6", "xhigh");
    let source = CommandAuthoritySource::direct("review.tool").unwrap();
    let catalog = CommandAuthorityCatalogIdentity::new("shipped", "catalog-v7").unwrap();
    let policy =
        CommandAuthorityPolicy::new(false, vec!["codex".to_string()], false, true).unwrap();

    assert!(
        CommandAuthoritySnapshot::new(source.clone(), policy.clone(), catalog.clone(), vec![])
            .is_err()
    );
    assert!(
        CommandAuthoritySnapshot::new(source, policy, catalog, vec![model.clone(), model],)
            .is_err()
    );
    assert!(CommandAuthoritySource::default_model(" ").is_err());
    assert!(CommandAuthorityCatalogIdentity::new("shipped", "").is_err());
    assert!(
        CommandAuthorityPolicy::new(
            false,
            vec!["codex".to_string(), "codex".to_string()],
            false,
            true
        )
        .is_err()
    );
}

#[test]
fn campaign_persists_authority_and_rejects_a_mismatched_digest() {
    let authority = snapshot(
        vec![admitted("codex", "openai", "gpt-5.6", "xhigh")],
        &["codex"],
        false,
        false,
        true,
    );
    let campaign = CampaignRecord::new(
        CampaignId::parse("01J00000000000000000000000").unwrap(),
        Utc.with_ymd_and_hms(2026, 7, 14, 12, 0, 0).unwrap(),
        Some(Sha256Digest::compute(b"policy")),
        authority.clone(),
    );

    assert_eq!(campaign.command_authority(), &authority);
    assert_eq!(campaign.command_authority_digest(), &authority.digest());
    let round_trip: CampaignRecord =
        serde_json::from_value(serde_json::to_value(&campaign).unwrap()).unwrap();
    assert_eq!(round_trip, campaign);

    let mut corrupted = serde_json::to_value(&campaign).unwrap();
    corrupted["command_authority_digest"] = serde_json::json!("f".repeat(64));
    assert!(serde_json::from_value::<CampaignRecord>(corrupted).is_err());
}
