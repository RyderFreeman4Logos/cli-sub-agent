use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;

use anyhow::{Result, anyhow};
use clap::{CommandFactory, Parser};
use csa_process::ProviderTurnCompletion;
use csa_session::convergence::{
    AdmittedModelIdentity, CampaignId, ConvergenceEvent, ConvergenceLedger, DiscoveryRunIntent,
    Sha256Digest,
};
use serde_json::{Value, json};

use super::engine::{
    DiscoveryRequest, DiscoveryRunOutput, DiscoveryRunner, FrozenWorkspace, LedgerPort,
    ObservationInput, WorkspaceProbe, run_discovery_observation,
};
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
}

impl LedgerPort for MemoryStore {
    fn load(&self) -> Result<ConvergenceLedger> {
        Ok(self.ledger.borrow().clone())
    }

    fn append(&self, campaign_id: CampaignId, event: ConvergenceEvent) -> Result<()> {
        let count = self.append_count.get();
        if self.fail_at.get() == Some(count) {
            return Err(anyhow!("scripted store failure"));
        }
        self.ledger.borrow_mut().append(campaign_id, event)?;
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
}

impl ScriptedRunner {
    fn pages(pages: impl IntoIterator<Item = String>) -> Self {
        Self {
            steps: pages.into_iter().map(RunnerStep::Page).collect(),
            requests: Vec::new(),
        }
    }
}

impl DiscoveryRunner for ScriptedRunner {
    fn run<'a>(
        &'a mut self,
        request: DiscoveryRequest,
    ) -> Pin<Box<dyn Future<Output = Result<DiscoveryRunOutput>> + 'a>> {
        self.requests.push(request);
        let step = self.steps.pop_front();
        Box::pin(async move {
            match step {
                Some(RunnerStep::Page(raw)) => output(ProviderTurnCompletion::Natural, raw),
                Some(RunnerStep::Completion(completion, raw)) => output(completion, raw),
                Some(RunnerStep::Failure(message)) => Err(anyhow!(message)),
                None => Err(anyhow!("scripted runner exhausted")),
            }
        })
    }
}

fn frozen() -> FrozenWorkspace {
    FrozenWorkspace::new(BASE, HEAD, Sha256Digest::compute(b"diff"), true, true).unwrap()
}

fn output(completion: ProviderTurnCompletion, raw: String) -> Result<DiscoveryRunOutput> {
    DiscoveryRunOutput::new(
        raw,
        SESSION,
        completion,
        AdmittedModelIdentity::new("codex", "openai", "gpt-5.6", "high")?,
        "output/convergence-discovery-page.json",
    )
}

fn input() -> ObservationInput {
    ObservationInput::new("main...HEAD", Sha256Digest::compute(b"catalog"))
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
        "response_status": response_status,
        "completion": "natural",
        "candidate_limit": limit,
        "candidate_count": candidates.len(),
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
            "complete",
            2,
            false,
            &[],
            vec![candidate("top-a"), candidate("top-b")],
        ),
        page("complete", 2, false, &[], vec![candidate("hidden-c")]),
        page("complete", 2, false, &[], Vec::new()),
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
    let signal_pages = [
        page("incomplete", 2, false, &[], Vec::new()),
        page("complete", 2, true, &[], Vec::new()),
        page("complete", 2, false, &["src/unscanned.rs"], Vec::new()),
        page("complete", 1, false, &[], vec![candidate("limit-full")]),
    ];

    for signal in signal_pages {
        let mut probe = ScriptedProbe::stable(5);
        let mut runner =
            ScriptedRunner::pages([signal, page("complete", 2, false, &[], Vec::new())]);
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
                2,
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
            page("complete", 2, false, &[], Vec::new()),
        ),
    ];

    for step in cases {
        let mut probe = ScriptedProbe::stable(3);
        let mut runner = ScriptedRunner {
            steps: [step].into_iter().collect(),
            requests: Vec::new(),
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
                2,
                false,
                &[],
                vec![candidate("persisted")],
            )),
            RunnerStep::Failure("stop after first finalized page"),
        ]
        .into_iter()
        .collect(),
        requests: Vec::new(),
    };
    assert!(
        run_discovery_observation(&input(), &mut first_probe, &mut first_runner, &store)
            .await
            .is_err()
    );

    let mut resumed_probe = ScriptedProbe::stable(3);
    let mut resumed_runner = ScriptedRunner::pages([page("complete", 2, false, &[], Vec::new())]);
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
            "response_status": "complete", "completion": "natural",
            "candidate_limit": 1, "candidate_count": 0,
            "more_candidates_possible": false, "unscanned_items": [],
            "candidates": [], "unknown": true
        })
        .to_string(),
        json!({
            "response_status": "complete", "completion": "natural",
            "candidate_limit": 1, "candidate_count": 1,
            "more_candidates_possible": false, "unscanned_items": [],
            "candidates": []
        })
        .to_string(),
        json!({
            "response_status": "complete", "completion": "natural",
            "candidate_limit": 1, "candidate_count": 2,
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

    let prompt = super::runner::build_discovery_prompt(&DiscoveryRequest::for_test(frozen()));
    assert!(prompt.contains("walking-skeleton observation cell"));
    assert!(prompt.contains("not exhaustive semantic coverage"));
    assert!(!prompt.contains("prior round"));
    assert!(!prompt.contains("patch"));
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
    let cases = [
        vec!["--converge", "--range", "main...HEAD"],
        vec!["--discovery-only", "--range", "main...HEAD"],
        vec!["--converge", "--discovery-only"],
        vec!["--converge", "--discovery-only", "--range", "main..HEAD"],
        vec![
            "--converge",
            "--discovery-only",
            "--range",
            "main...HEAD",
            "--check-verdict",
        ],
        vec![
            "--converge",
            "--discovery-only",
            "--range",
            "main...HEAD",
            "--fix",
        ],
        vec![
            "--converge",
            "--discovery-only",
            "--range",
            "main...HEAD",
            "--fix-finding",
            "--session",
            SESSION,
        ],
        vec![
            "--converge",
            "--discovery-only",
            "--range",
            "main...HEAD",
            "--session",
            SESSION,
        ],
        vec![
            "--converge",
            "--discovery-only",
            "--range",
            "main...HEAD",
            "--reviewers",
            "2",
        ],
        vec![
            "--converge",
            "--discovery-only",
            "--range",
            "main...HEAD",
            "--no-fs-sandbox",
        ],
        vec![
            "--converge",
            "--discovery-only",
            "--range",
            "main...HEAD",
            "--extra-writable",
            "/tmp",
        ],
        vec![
            "--converge",
            "--discovery-only",
            "--range",
            "main...HEAD",
            "--prior-rounds-summary",
            "old.toml",
        ],
    ];

    for tail in cases {
        let mut argv = vec!["csa", "review"];
        argv.extend(tail);
        let args = parsed_review(&argv);
        let error = crate::cli::validate_review_args(&args)
            .expect_err("unsafe convergence combination must fail");
        assert!(
            error.to_string().contains("experimental observe-only"),
            "unexpected validation error: {error}"
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
