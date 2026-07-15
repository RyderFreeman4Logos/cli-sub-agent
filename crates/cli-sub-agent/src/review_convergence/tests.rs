use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use anyhow::{Result, anyhow};
use clap::{CommandFactory, Parser};
use csa_process::ProviderTurnCompletion;
use csa_session::convergence::{
    AdmittedModelIdentity, ArtifactEvidenceRef, CampaignId, CommandAuthorityCatalogIdentity,
    CommandAuthorityPolicy, CommandAuthoritySnapshot, CommandAuthoritySource, ConvergenceEvent,
    ConvergenceLedger, DiscoveryRunIntent, Sha256Digest,
};
use serde_json::{Value, json};

use super::engine::{
    DiscoveryRequest, DiscoveryRunOutput, DiscoveryRunner, FrozenWorkspace, LedgerPort,
    ObservationInput, WorkspaceProbe, run_discovery_observation,
};
use super::output::encode_discovery_page_artifact;
use super::schema::parse_discovery_page;

const BASE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HEAD: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const SESSION: &str = "01J00000000000000000000000";

#[derive(Default)]
struct MemoryStore {
    ledger: RefCell<ConvergenceLedger>,
    append_count: Cell<usize>,
    fail_at: Cell<Option<usize>>,
}

impl MemoryStore {
    fn fail_next(&self) {
        self.fail_at.set(Some(self.append_count.get()));
    }

    fn fail_at_append(&self, append_index: usize) {
        self.fail_at.set(Some(append_index));
    }

    fn clear_failure(&self) {
        self.fail_at.set(None);
    }
}

impl LedgerPort for MemoryStore {
    fn load(&self) -> Result<ConvergenceLedger> {
        Ok(self.ledger.borrow().clone())
    }

    fn append_batch(&self, campaign_id: CampaignId, events: Vec<ConvergenceEvent>) -> Result<()> {
        let count = self.append_count.get();
        if self.fail_at.get() == Some(count) {
            return Err(anyhow!("scripted store failure"));
        }
        let mut next = self.ledger.borrow().clone();
        next.append_batch(campaign_id, events)?;
        *self.ledger.borrow_mut() = next;
        self.append_count.set(count + 1);
        Ok(())
    }
}

struct ScriptedProbe {
    captures: VecDeque<Result<FrozenWorkspace>>,
}

impl ScriptedProbe {
    fn stable(call_count: usize) -> Self {
        let frozen = frozen();
        Self {
            captures: (0..call_count).map(|_| Ok(frozen.clone())).collect(),
        }
    }
}

impl WorkspaceProbe for ScriptedProbe {
    fn capture(&mut self, _range: &str) -> Result<FrozenWorkspace> {
        self.captures
            .pop_front()
            .unwrap_or_else(|| Err(anyhow!("unexpected workspace capture")))
    }
}

enum RunnerStep {
    Page(String),
    Failure(&'static str),
    Completion(ProviderTurnCompletion, String),
}

#[derive(Default)]
struct ScriptedRunner {
    steps: VecDeque<RunnerStep>,
    requests: Vec<DiscoveryRequest>,
    artifacts: HashMap<String, Vec<u8>>,
}

impl ScriptedRunner {
    fn pages(pages: impl IntoIterator<Item = String>) -> Self {
        Self {
            steps: pages.into_iter().map(RunnerStep::Page).collect(),
            requests: Vec::new(),
            artifacts: HashMap::new(),
        }
    }
}

impl DiscoveryRunner for ScriptedRunner {
    fn run<'a>(
        &'a mut self,
        request: DiscoveryRequest,
    ) -> Pin<Box<dyn Future<Output = Result<DiscoveryRunOutput>> + 'a>> {
        let provider_evidence = request.frozen.provider_evidence.identity.clone();
        self.requests.push(request);
        let step = self.steps.pop_front();
        let result = match step {
            Some(RunnerStep::Page(raw)) => output(ProviderTurnCompletion::Natural, raw),
            Some(RunnerStep::Completion(completion, raw)) => output(completion, raw),
            Some(RunnerStep::Failure(message)) => Err(anyhow!(message)),
            None => Err(anyhow!("scripted runner exhausted")),
        };
        if let Ok(output) = &result {
            self.artifacts.insert(
                output.artifact.digest().to_string(),
                encode_discovery_page_artifact(&output.raw_response, &provider_evidence)
                    .expect("encode scripted discovery artifact"),
            );
        }
        Box::pin(async move { result })
    }

    fn read_artifact<'a>(
        &'a mut self,
        artifact: &'a ArtifactEvidenceRef,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>>> + 'a>> {
        let result = self
            .artifacts
            .get(&artifact.digest().to_string())
            .cloned()
            .ok_or_else(|| anyhow!("scripted artifact not found"));
        Box::pin(async move { result })
    }
}

fn frozen() -> FrozenWorkspace {
    FrozenWorkspace::new(BASE, HEAD, Sha256Digest::compute(b"diff"), true, true).unwrap()
}

fn output(completion: ProviderTurnCompletion, raw: String) -> Result<DiscoveryRunOutput> {
    let frozen = frozen();
    DiscoveryRunOutput::new(
        raw,
        SESSION,
        completion,
        AdmittedModelIdentity::new("codex", "openai", "gpt-5.6", "high")?,
        "output/convergence-discovery-page.json",
        &frozen.provider_evidence.identity,
    )
}

fn input() -> ObservationInput {
    ObservationInput::new("main...HEAD", authority("gpt-5.6"))
}

fn authority(model: &str) -> CommandAuthoritySnapshot {
    CommandAuthoritySnapshot::new(
        CommandAuthoritySource::tier("quality", "review.tier").unwrap(),
        CommandAuthorityPolicy::new(false, vec!["codex".to_string()], false, true).unwrap(),
        CommandAuthorityCatalogIdentity::new("effective command catalog", "test-v1").unwrap(),
        vec![AdmittedModelIdentity::new("codex", "openai", model, "high").unwrap()],
    )
    .unwrap()
}

fn candidate(mechanism: &str) -> Value {
    json!({
        "mechanism": mechanism,
        "affected_component": "review convergence",
        "bug_class": "evidence gap"
    })
}

fn page(
    response_status: &str,
    limit: u32,
    more: bool,
    unscanned: &[&str],
    candidates: Vec<Value>,
) -> String {
    json!({
        "schema_version": 1,
        "kind": "convergence_discovery_page",
        "response_status": response_status,
        "candidate_limit": limit,
        "more_candidates_possible": more,
        "unscanned_items": unscanned,
        "candidates": candidates,
    })
    .to_string()
}

#[tokio::test]
async fn hidden_top_k_collects_union_on_one_frozen_tuple_before_zero_new_challenge() {
    let pages = [
        page(
            "partial",
            8,
            true,
            &[],
            vec![candidate("top-a"), candidate("top-b")],
        ),
        page("complete", 8, false, &[], vec![candidate("hidden-c")]),
        page("complete", 8, false, &[], Vec::new()),
    ];
    let mut probe = ScriptedProbe::stable(7);
    let mut runner = ScriptedRunner::pages(pages);
    let store = MemoryStore::default();

    let summary = run_discovery_observation(&input(), &mut probe, &mut runner, &store)
        .await
        .expect("discovery should reach zero-new evidence");

    assert_eq!(summary.provider_calls, 3);
    assert_eq!(summary.candidates, 3);
    assert_eq!(summary.base_oid, BASE);
    assert_eq!(summary.head_oid, HEAD);
    assert!(summary.discovery_evidence_complete);
    assert_eq!(summary.review_verdict, None);
    assert!(!summary.merge_attestation);
    assert_eq!(
        runner
            .requests
            .iter()
            .map(|request| request.intent)
            .collect::<Vec<_>>(),
        vec![
            DiscoveryRunIntent::Initial,
            DiscoveryRunIntent::Continuation,
            DiscoveryRunIntent::SaturationChallenge,
        ]
    );
    assert!(
        probe.captures.is_empty(),
        "every call must be bracketed by probes"
    );
}

#[tokio::test]
async fn every_page_continuation_signal_forces_another_call() {
    let full_page = (0..8)
        .map(|index| candidate(&format!("limit-full-{index}")))
        .collect();
    let signal_pages = [
        page("partial", 8, true, &[], Vec::new()),
        page("partial", 8, true, &["src/unscanned.rs"], Vec::new()),
        page("partial", 8, false, &["src/unscanned.rs"], Vec::new()),
        page("complete", 8, false, &[], full_page),
    ];

    for signal in signal_pages {
        let mut probe = ScriptedProbe::stable(5);
        let mut runner =
            ScriptedRunner::pages([signal, page("complete", 8, false, &[], Vec::new())]);
        let store = MemoryStore::default();
        let summary = run_discovery_observation(&input(), &mut probe, &mut runner, &store)
            .await
            .expect("continuation signal should be resolved by zero-new page");
        assert_eq!(summary.provider_calls, 2);
        assert_eq!(runner.requests[1].intent, DiscoveryRunIntent::Continuation);
    }
}

#[tokio::test]
async fn workspace_mutation_blocks_without_accepting_mixed_evidence() {
    let mut changed = frozen();
    changed.head_oid = "cccccccccccccccccccccccccccccccccccccccc".to_string();
    let mut probe = ScriptedProbe {
        captures: [Ok(frozen()), Ok(frozen()), Ok(changed)]
            .into_iter()
            .collect(),
    };
    let mut runner = ScriptedRunner::pages([page(
        "complete",
        2,
        false,
        &[],
        vec![candidate("must-not-be-accepted")],
    )]);
    let store = MemoryStore::default();

    let error = run_discovery_observation(&input(), &mut probe, &mut runner, &store)
        .await
        .expect_err("mutation must fail closed");

    assert_eq!(error.diagnostic().reason_code, "workspace_mutated");
    assert!(
        !store
            .ledger
            .borrow()
            .entries()
            .iter()
            .any(|entry| matches!(entry.event(), ConvergenceEvent::DiscoveryAttemptRecorded(_)))
    );
}

#[tokio::test]
async fn budget_exhaustion_is_blocked_and_never_evidence_complete() {
    let pages = (0..super::engine::MAX_PROVIDER_CALLS_PER_CELL)
        .map(|index| {
            page(
                "complete",
                8,
                false,
                &[],
                vec![candidate(&format!("candidate-{index}"))],
            )
        })
        .collect::<Vec<_>>();
    let mut probe = ScriptedProbe::stable(1 + (pages.len() * 2));
    let mut runner = ScriptedRunner::pages(pages);
    let store = MemoryStore::default();

    let error = run_discovery_observation(&input(), &mut probe, &mut runner, &store)
        .await
        .expect_err("bounded call budget must block");
    assert_eq!(
        error.diagnostic().reason_code,
        "provider_call_budget_exhausted"
    );
    assert!(!error.diagnostic().discovery_evidence_complete);
}

#[tokio::test]
async fn malformed_provider_failure_and_noncompletion_fail_closed() {
    let cases = [
        RunnerStep::Page("not json".to_string()),
        RunnerStep::Failure("provider unavailable"),
        RunnerStep::Completion(
            ProviderTurnCompletion::Incomplete,
            page("complete", 8, false, &[], Vec::new()),
        ),
    ];

    for step in cases {
        let mut probe = ScriptedProbe::stable(3);
        let mut runner = ScriptedRunner {
            steps: [step].into_iter().collect(),
            requests: Vec::new(),
            ..ScriptedRunner::default()
        };
        let store = MemoryStore::default();
        assert!(
            run_discovery_observation(&input(), &mut probe, &mut runner, &store)
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn persisted_history_resumes_from_the_same_reducer_directive() {
    let store = MemoryStore::default();
    let mut first_probe = ScriptedProbe::stable(5);
    let mut first_runner = ScriptedRunner {
        steps: [
            RunnerStep::Page(page(
                "complete",
                8,
                false,
                &[],
                vec![candidate("persisted")],
            )),
            RunnerStep::Failure("stop after first finalized page"),
        ]
        .into_iter()
        .collect(),
        requests: Vec::new(),
        ..ScriptedRunner::default()
    };
    assert!(
        run_discovery_observation(&input(), &mut first_probe, &mut first_runner, &store)
            .await
            .is_err()
    );

    let mut resumed_probe = ScriptedProbe::stable(3);
    let mut resumed_runner = ScriptedRunner::pages([page("complete", 8, false, &[], Vec::new())]);
    let summary =
        run_discovery_observation(&input(), &mut resumed_probe, &mut resumed_runner, &store)
            .await
            .expect("resume should replay and complete");
    assert_eq!(
        resumed_runner.requests[0].intent,
        DiscoveryRunIntent::SaturationChallenge
    );
    assert_eq!(summary.candidates, 1);
}

#[tokio::test]
async fn persisted_partial_attempt_recovers_candidates_from_its_artifact() {
    let store = MemoryStore::default();
    // Campaign, epoch, cell, disposition, plan, and attempt all publish first.
    // The next append fails before the candidate is durable.
    store.fail_at_append(6);
    let mut first_probe = ScriptedProbe::stable(3);
    let mut first_runner = ScriptedRunner::pages([page(
        "complete",
        8,
        false,
        &[],
        vec![candidate("recoverable")],
    )]);
    let first_error =
        run_discovery_observation(&input(), &mut first_probe, &mut first_runner, &store)
            .await
            .expect_err("candidate publication should hit the scripted failure");
    assert_eq!(first_error.diagnostic().reason_code, "store_failure");
    assert!(
        store
            .ledger
            .borrow()
            .entries()
            .iter()
            .any(|entry| matches!(entry.event(), ConvergenceEvent::DiscoveryAttemptRecorded(_)))
    );
    assert!(
        !store
            .ledger
            .borrow()
            .entries()
            .iter()
            .any(|entry| matches!(entry.event(), ConvergenceEvent::CandidateRecorded(_)))
    );

    let durable_artifacts = std::mem::take(&mut first_runner.artifacts);
    store.clear_failure();
    let mut resumed_probe = ScriptedProbe::stable(3);
    let mut resumed_runner = ScriptedRunner::pages([page("complete", 8, false, &[], Vec::new())]);
    resumed_runner.artifacts = durable_artifacts;
    let summary =
        run_discovery_observation(&input(), &mut resumed_probe, &mut resumed_runner, &store)
            .await
            .expect("resume should recover the missing candidate from durable page evidence");

    assert_eq!(summary.provider_calls, 2);
    assert_eq!(summary.candidates, 1);
    assert_eq!(resumed_runner.requests.len(), 1);
    assert_eq!(
        resumed_runner.requests[0].intent,
        DiscoveryRunIntent::SaturationChallenge
    );
}

#[tokio::test]
async fn store_failure_is_a_structured_block() {
    let store = MemoryStore::default();
    store.fail_next();
    let mut probe = ScriptedProbe::stable(1);
    let mut runner = ScriptedRunner::default();
    let error = run_discovery_observation(&input(), &mut probe, &mut runner, &store)
        .await
        .expect_err("store errors must block");
    assert_eq!(error.diagnostic().reason_code, "store_failure");
}

#[test]
fn parser_accepts_exact_json_or_one_complete_fence_and_rejects_ambiguous_pages() {
    let valid = page("complete", 2, false, &[], vec![candidate("one")]);
    assert!(parse_discovery_page(&valid).is_ok());
    assert!(parse_discovery_page(&format!("```json\n{valid}\n```\n")).is_ok());

    let invalid = [
        format!("prose\n{valid}"),
        format!("{valid}\nprose"),
        format!("```json\n{valid}\n```\ntrailing"),
        "{}".to_string(),
        json!({
            "schema_version": 1, "kind": "convergence_discovery_page",
            "response_status": "complete", "candidate_limit": 1,
            "more_candidates_possible": false, "unscanned_items": [],
            "candidates": [], "unknown": true
        })
        .to_string(),
        json!({
            "schema_version": 1, "kind": "convergence_discovery_page",
            "response_status": "complete", "completion": "natural", "candidate_limit": 1,
            "more_candidates_possible": false, "unscanned_items": [],
            "candidates": []
        })
        .to_string(),
        json!({
            "schema_version": 1, "kind": "convergence_discovery_page",
            "response_status": "complete", "candidate_limit": 1,
            "more_candidates_possible": false, "unscanned_items": [],
            "candidates": [candidate("a"), candidate("b")]
        })
        .to_string(),
        page(
            "complete",
            2,
            false,
            &[],
            vec![candidate("duplicate"), candidate("duplicate")],
        ),
        page("complete", 2, true, &[], Vec::new()),
        page("complete", 2, false, &["src/unscanned.rs"], Vec::new()),
        page("partial", 2, false, &[], Vec::new()),
    ];
    for raw in invalid {
        assert!(
            parse_discovery_page(&raw).is_err(),
            "accepted invalid page: {raw}"
        );
    }
}

#[test]
fn success_json_is_observation_only_and_never_claims_pass_clean_or_attestation() {
    let summary = super::engine::ObservationSummary::for_test(frozen());
    let rendered = serde_json::to_string(&summary).unwrap();
    assert!(rendered.contains("convergence_discovery_observation"));
    assert!(rendered.contains("\"review_verdict\":null"));
    assert!(rendered.contains("\"merge_attestation\":false"));
    assert!(rendered.contains("not exhaustive semantic coverage"));
    assert!(!rendered.contains("PASS"));
    assert!(!rendered.contains("CLEAN"));
}

#[test]
fn production_runner_mapping_is_fresh_readonly_and_history_free() {
    let policy = super::runner::execution_policy();
    assert!(policy.fresh_session);
    assert!(policy.readonly_project_root);
    assert!(policy.extra_writable.is_empty());
    assert!(!policy.no_fs_sandbox);

    let request = DiscoveryRequest::for_test(frozen());
    let input = super::runner::provider_input(&request);
    assert_ne!(input.project_root, Path::new("/mutable/source-checkout"));
    assert_eq!(
        input.bundle_path.parent(),
        Some(input.project_root.as_path())
    );
    assert!(input.extra_readable.is_empty());

    let prompt = super::runner::build_discovery_prompt(&request);
    assert!(prompt.contains("walking-skeleton observation cell"));
    assert!(prompt.contains("not exhaustive semantic coverage"));
    assert!(prompt.contains("provider-evidence.tar"));
    assert!(prompt.contains(input.bundle_digest.as_str()));
    assert!(prompt.contains("sha256sum"));
    assert!(prompt.contains("tar -tf"));
    assert!(!prompt.contains("/mutable/source-checkout"));
    assert!(!prompt.contains("prior round"));
}

fn parsed_review(argv: &[&str]) -> crate::cli::ReviewArgs {
    let cli = crate::cli::Cli::try_parse_from(argv).expect("review args should parse");
    let crate::cli::Commands::Review(args) = cli.command else {
        panic!("expected review command");
    };
    args
}

#[test]
fn convergence_cli_flags_parse_validate_and_appear_in_help() {
    let args = parsed_review(&[
        "csa",
        "review",
        "--converge",
        "--discovery-only",
        "--range",
        "main...HEAD",
    ]);
    assert!(args.converge);
    assert!(args.discovery_only);
    crate::cli::validate_review_args(&args).expect("experimental invocation should validate");

    let mut command = crate::cli::Cli::command();
    let help = command
        .find_subcommand_mut("review")
        .expect("review subcommand")
        .render_long_help()
        .to_string();
    assert!(help.contains("--converge"));
    assert!(help.contains("--discovery-only"));
    assert!(help.contains("observation"));
}

#[test]
fn convergence_cli_rejects_unpaired_non_range_and_unsafe_options() {
    let unsafe_case = |tail: &[&'static str]| {
        let mut args = vec!["--converge", "--discovery-only", "--range", "main...HEAD"];
        args.extend_from_slice(tail);
        args
    };
    let cases = [
        vec!["--converge", "--range", "main...HEAD"],
        vec!["--discovery-only", "--range", "main...HEAD"],
        vec!["--converge", "--discovery-only"],
        vec!["--converge", "--discovery-only", "--range", "main..HEAD"],
        unsafe_case(&["--check-verdict"]),
        unsafe_case(&["--fix"]),
        unsafe_case(&["--fix-finding", "--session", SESSION]),
        unsafe_case(&["--session", SESSION]),
        unsafe_case(&["--reviewers", "2"]),
        unsafe_case(&["--no-fs-sandbox"]),
        unsafe_case(&["--extra-readable", "/tmp/provider-input"]),
        unsafe_case(&["--context", "context.md"]),
        unsafe_case(&["--prompt-file", "prompt.md"]),
        unsafe_case(&["--spec", "contract.spec"]),
        unsafe_case(&["--extra-writable", "/tmp"]),
        unsafe_case(&["--prior-rounds-summary", "old.toml"]),
    ];

    for mut tail in cases {
        let spec = tail.iter().position(|arg| *arg == "--spec").map(|index| {
            tail.remove(index);
            tail.remove(index).to_owned()
        });
        let mut argv = vec!["csa", "review"];
        argv.extend(tail);
        let mut args = parsed_review(&argv);
        args.spec = spec;
        let error = crate::cli::validate_review_args(&args)
            .expect_err("unsafe convergence combination must fail");
        assert!(
            error.to_string().contains("experimental observe-only"),
            "unexpected validation error: {error}"
        );
    }
}

#[test]
fn legacy_review_still_accepts_supplementary_read_inputs() {
    let mut args = parsed_review(&[
        "csa",
        "review",
        "--range",
        "main...HEAD",
        "--context",
        "context.md",
        "--prompt-file",
        "prompt.md",
        "--extra-readable",
        "/tmp/provider-input",
    ]);
    args.spec = Some("contract.spec".to_owned());
    crate::cli::validate_review_args(&args)
        .expect("legacy review supplementary inputs must remain accepted");
}

#[test]
fn convergence_dispatch_precedes_ordinary_quality_gate_prompt_context_depth_and_diff() {
    let source = include_str!("../review_cmd_handle.rs");
    let Some(dispatch) = source.find("if args.converge {") else {
        panic!("convergence dispatch is missing");
    };
    for ordinary_step in [
        "verify_review_skill_available",
        "run_pre_review_quality_gate",
        "derive_scope_for_project",
        "resolve_review_depth_for_project",
        "compute_review_diff_size",
        "build_review_instruction_for_project",
    ] {
        let Some(position) = source.find(ordinary_step) else {
            panic!("ordinary review step is missing: {ordinary_step}");
        };
        assert!(
            dispatch < position,
            "convergence dispatch must precede ordinary review step {ordinary_step}"
        );
    }
}

#[test]
fn convergence_cli_rejects_non_range_scope_selectors_at_parse_time() {
    let selectors = [
        vec!["--diff"],
        vec!["--branch", "feature"],
        vec!["--commit", "HEAD"],
        vec!["--files", "src/lib.rs"],
    ];
    for selector in selectors {
        let mut argv = vec![
            "csa",
            "review",
            "--converge",
            "--discovery-only",
            "--range",
            "main...HEAD",
        ];
        argv.extend(selector);
        assert!(crate::cli::Cli::try_parse_from(argv).is_err());
    }
}

#[path = "campaign_authority_tests.rs"]
mod campaign_authority_tests;
