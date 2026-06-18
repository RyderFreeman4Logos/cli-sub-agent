use super::*;

#[test]
fn reviewer_unavailable_error_reason_maps_memory_soft_limit_admission() {
    let err = anyhow::anyhow!(
        "CSA: memory_soft_limit_admission denied -- codex reviewer soft memory threshold is 5734MB"
    );

    let reason = reviewer_unavailable_error_reason(&err, ToolName::Codex)
        .expect("memory soft-limit admission reason");

    assert!(reason.contains("codex tool failure"));
    assert!(reason.contains("memory_soft_limit_admission"));
}
