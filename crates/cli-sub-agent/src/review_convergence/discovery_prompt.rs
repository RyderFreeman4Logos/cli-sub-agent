use csa_session::convergence::{ArtifactEvidenceRef, EpochRecord};

use super::discovery_contract::{DiscoveryFocus, DiscoveryRequest, TargetedDiscoveryFocus};

pub(crate) fn build_discovery_prompt(request: &DiscoveryRequest) -> String {
    match &request.focus {
        DiscoveryFocus::Broad => build_broad_discovery_prompt(request),
        DiscoveryFocus::Targeted(focus) => build_targeted_discovery_prompt(request, focus),
    }
}

fn build_targeted_discovery_prompt(
    request: &DiscoveryRequest,
    focus: &TargetedDiscoveryFocus,
) -> String {
    let artifact = focus.artifact();
    let identities = focus
        .semantic_finding_ids()
        .iter()
        .map(|identity| format!("stable_id={identity}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Use the csa-review skill. Observe only; do not modify files. This is a clean-room targeted rediscovery, not a continuation of any transcript. Read only immutable artifact csa-session://{}/{} with required SHA-256 {} and the exact frozen range tuple below. Do not use parent context, hidden memory, prior transcripts, raw history, or reviewer prose. Re-discover only the listed compact semantic identities.\nRange label: {}\nExact merge-base OID: {}\nExact HEAD OID: {}\nExact diff SHA-256: {}\nSemantic finding identities:\n{}\nReturn exact JSON and no prose. Use exactly this schema: {{\"schema_version\":1,\"kind\":\"convergence_discovery_page\",\"response_status\":\"complete|partial\",\"candidate_limit\":{},\"more_candidates_possible\":false,\"unscanned_items\":[],\"candidates\":[{{\"violated_invariant\":\"...\",\"trigger_failure_mode\":\"...\",\"primary_component\":\"...\",\"bug_class\":\"...\"}}]}}. The candidates array must not exceed candidate_limit. A complete page must have no continuation signals. A partial page must set more_candidates_possible or list at least one unscanned item.",
        artifact.csa_session_id(),
        artifact.path(),
        artifact.digest(),
        request.range,
        request.frozen.base_oid,
        request.frozen.head_oid,
        request.frozen.diff_digest,
        identities,
        request.candidate_limit,
    )
}

pub(crate) fn build_clean_room_prompt(
    epoch: &EpochRecord,
    artifact: &ArtifactEvidenceRef,
) -> String {
    format!(
        "Perform a fresh clean-room review of only immutable artifact csa-session://{}/{} with required SHA-256 {} for exact epoch {} (base {}, HEAD {}, diff {}). Do not use prior transcripts, parent context, hidden memory, or prior reviewer prose. Return exact JSON and no prose. Use exactly this schema: {{\"schema_version\":1,\"kind\":\"convergence_clean_room_review\",\"artifact\":{{\"csa_session_id\":\"...\",\"path\":\"...\",\"digest\":\"sha256:...\"}},\"model_identity\":{{\"tool\":\"...\",\"provider\":\"...\",\"model\":\"...\",\"reasoning\":\"...\"}},\"findings\":[{{\"semantic_identity\":{{\"violated_invariant\":\"...\",\"trigger_failure_mode\":\"...\",\"primary_component\":\"...\",\"bug_class\":\"...\"}},\"path\":\"normalized/relative/path\",\"span\":{{\"start_line\":1,\"end_line\":1}},\"category\":\"lowercase-token\",\"severity\":\"blocker|critical|high|medium|low\",\"summary\":\"...\",\"evidence\":\"...\"}}],\"questions\":[],\"unchecked_items\":[]}}. Repeat the exact artifact reference and admitted model identity used for this review.",
        artifact.csa_session_id(),
        artifact.path(),
        artifact.digest(),
        epoch.id(),
        epoch.base_oid(),
        epoch.head_oid(),
        epoch.diff_digest(),
    )
}

fn build_broad_discovery_prompt(request: &DiscoveryRequest) -> String {
    let intent = match request.intent {
        csa_session::convergence::DiscoveryRunIntent::Initial => "initial broad discovery",
        csa_session::convergence::DiscoveryRunIntent::Continuation => {
            "continuation for hidden or unscanned candidates"
        }
        csa_session::convergence::DiscoveryRunIntent::SaturationChallenge => {
            "zero-new saturation challenge"
        }
    };
    let known_findings = if request.continuation.findings.is_empty() {
        "none".to_string()
    } else {
        request
            .continuation
            .findings
            .iter()
            .map(|finding| {
                format!(
                    "stable_id={}; violated_invariant={}; trigger_failure_mode={}; primary_component={}; bug_class={}",
                    finding.stable_finding_id.as_str(),
                    finding.semantic_identity.violated_invariant(),
                    finding.semantic_identity.trigger_failure_mode(),
                    finding.semantic_identity.primary_component(),
                    finding.semantic_identity.bug_class(),
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let latest_unscanned = if request
        .continuation
        .latest_finalized_unscanned_items
        .is_empty()
    {
        "none".to_string()
    } else {
        request
            .continuation
            .latest_finalized_unscanned_items
            .join(", ")
    };
    let uncovered_cells = if request.continuation.uncovered_cells.is_empty() {
        "none".to_string()
    } else {
        request
            .continuation
            .uncovered_cells
            .iter()
            .map(|cell| {
                format!(
                    "cell_id={}; scope={}={}; lens={}",
                    cell.id(),
                    cell.scope().kind(),
                    cell.scope().key(),
                    cell.lens().as_str(),
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let uncovered_items = if request.continuation.uncovered_items.is_empty() {
        "none".to_string()
    } else {
        request.continuation.uncovered_items.join(", ")
    };
    format!(
        "Use the csa-review skill. Observe only; do not modify files. The only review evidence is the immutable bundle file ./{}, and the checkout that created it is not available to you. Before reading evidence and again after finishing, run `sha256sum {}` and require SHA-256 {}. Use read-only commands such as `tar -tf {}`, `tar -xOf {} manifest.json`, `tar -xOf {} diff.patch`, and `tar -xOf {} source.tar | tar -tf -`; do not extract files to disk. This request covers one Required cell in a deterministic scope-by-semantic-lens manifest. Discovery evidence completes only after every required manifest cell has a fresh zero-new saturation page.\nRange label: {}\nExact merge-base OID: {}\nExact HEAD OID: {}\nExact diff SHA-256: {}\nCurrent manifest cell: id={}; scope={}={}; lens={}.\nRun intent: {intent}; finalized attempts: {}.\nPreviously reported semantic findings (return only genuinely new candidates):\n{}\nLatest finalized attempt unscanned items:\n{}\nUncovered manifest cells:\n{}\nUncovered manifest items:\n{}\nReturn exact JSON or one complete json fence and no prose. Use exactly this schema: {{\"schema_version\":1,\"kind\":\"convergence_discovery_page\",\"response_status\":\"complete|partial\",\"candidate_limit\":{},\"more_candidates_possible\":false,\"unscanned_items\":[],\"candidates\":[{{\"violated_invariant\":\"...\",\"trigger_failure_mode\":\"...\",\"primary_component\":\"...\",\"bug_class\":\"...\"}}]}}. The candidates array must not exceed candidate_limit. A complete page must have no continuation signals. A partial page must set more_candidates_possible or list at least one unscanned item.",
        request.frozen.provider_evidence.identity.bundle_file,
        request.frozen.provider_evidence.identity.bundle_file,
        request.frozen.provider_evidence.identity.bundle_digest,
        request.frozen.provider_evidence.identity.bundle_file,
        request.frozen.provider_evidence.identity.bundle_file,
        request.frozen.provider_evidence.identity.bundle_file,
        request.frozen.provider_evidence.identity.bundle_file,
        request.range,
        request.frozen.base_oid,
        request.frozen.head_oid,
        request.frozen.diff_digest,
        request.cell.id(),
        request.cell.scope().kind(),
        request.cell.scope().key(),
        request.cell.lens().as_str(),
        request.prior_finalized_attempt_count,
        known_findings,
        latest_unscanned,
        uncovered_cells,
        uncovered_items,
        request.candidate_limit,
    )
}
