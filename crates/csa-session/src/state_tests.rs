use super::*;

fn sample_state_with_phase(phase: SessionPhase) -> MetaSessionState {
    let now = chrono::Utc::now();
    MetaSessionState {
        meta_session_id: ulid::Ulid::new().to_string(),
        description: Some("phase-test".to_string()),
        project_path: "/tmp/test".to_string(),
        branch: None,
        created_at: now,
        last_accessed: now,
        genealogy: Genealogy::default(),
        tools: HashMap::new(),
        context_status: ContextStatus::default(),
        total_token_usage: None,
        phase,
        task_context: TaskContext::default(),
        turn_count: 0,
        token_budget: None,
        sandbox_info: None,
        termination_reason: None,
        is_seed_candidate: false,
        git_head_at_creation: None,
        last_return_packet: None,
        fork_call_timestamps: Vec::new(),
    }
}

// ── Valid transitions ────────────────────────────────────────────

#[test]
fn test_active_compressed_becomes_available() {
    let phase = SessionPhase::Active;
    assert_eq!(
        phase.transition(&PhaseEvent::Compressed),
        Ok(SessionPhase::Available)
    );
}

#[test]
fn test_active_retired_becomes_retired() {
    let phase = SessionPhase::Active;
    assert_eq!(
        phase.transition(&PhaseEvent::Retired),
        Ok(SessionPhase::Retired)
    );
}

#[test]
fn test_available_resumed_becomes_active() {
    let phase = SessionPhase::Available;
    assert_eq!(
        phase.transition(&PhaseEvent::Resumed),
        Ok(SessionPhase::Active)
    );
}

#[test]
fn test_available_retired_becomes_retired() {
    let phase = SessionPhase::Available;
    assert_eq!(
        phase.transition(&PhaseEvent::Retired),
        Ok(SessionPhase::Retired)
    );
}

// ── Invalid transitions ─────────────────────────────────────────

#[test]
fn test_active_resumed_is_invalid() {
    let phase = SessionPhase::Active;
    assert!(phase.transition(&PhaseEvent::Resumed).is_err());
}

#[test]
fn test_available_compressed_is_invalid() {
    let phase = SessionPhase::Available;
    assert!(phase.transition(&PhaseEvent::Compressed).is_err());
}

#[test]
fn test_retired_compressed_is_invalid() {
    let phase = SessionPhase::Retired;
    assert!(phase.transition(&PhaseEvent::Compressed).is_err());
}

#[test]
fn test_retired_resumed_is_invalid() {
    let phase = SessionPhase::Retired;
    assert!(phase.transition(&PhaseEvent::Resumed).is_err());
}

#[test]
fn test_retired_retired_is_invalid() {
    let phase = SessionPhase::Retired;
    assert!(phase.transition(&PhaseEvent::Retired).is_err());
}

// ── Display ─────────────────────────────────────────────────────

#[test]
fn test_display() {
    assert_eq!(SessionPhase::Active.to_string(), "active");
    assert_eq!(SessionPhase::Available.to_string(), "available");
    assert_eq!(SessionPhase::Retired.to_string(), "retired");
}

// ── Round-trip: Active → Available → Active ─────────────────────

#[test]
fn test_round_trip_active_available_active() {
    let phase = SessionPhase::Active;
    let available = phase.transition(&PhaseEvent::Compressed).unwrap();
    assert_eq!(available, SessionPhase::Available);
    let active_again = available.transition(&PhaseEvent::Resumed).unwrap();
    assert_eq!(active_again, SessionPhase::Active);
}

// ── MetaSessionState phase application ──────────────────────────

#[test]
fn test_apply_phase_event_resumed_available_to_active() {
    let mut state = sample_state_with_phase(SessionPhase::Available);
    state
        .apply_phase_event(PhaseEvent::Resumed)
        .expect("Available -> Active should be valid");
    assert_eq!(state.phase, SessionPhase::Active);
}

#[test]
fn test_apply_phase_event_records_phase_change_in_state() {
    let mut state = sample_state_with_phase(SessionPhase::Active);
    state
        .apply_phase_event(PhaseEvent::Compressed)
        .expect("Active -> Available should be valid");
    assert_eq!(state.phase, SessionPhase::Available);
}

#[test]
fn test_apply_phase_event_rejects_retired_to_active() {
    let mut state = sample_state_with_phase(SessionPhase::Retired);
    let err = state
        .apply_phase_event(PhaseEvent::Resumed)
        .expect_err("Retired -> Active should fail");
    assert!(
        err.contains("invalid phase transition"),
        "error should describe invalid transition"
    );
    assert_eq!(state.phase, SessionPhase::Retired);
}

// ── Serde round-trip ───────────────────────────────────────────

/// Wrapper struct to test enum serialization (TOML can't serialize bare enums).
#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct PhaseWrapper {
    phase: SessionPhase,
}

#[test]
fn test_session_phase_serde_roundtrip() {
    for phase in [
        SessionPhase::Active,
        SessionPhase::Available,
        SessionPhase::Retired,
    ] {
        let wrapper = PhaseWrapper {
            phase: phase.clone(),
        };
        let serialized = toml::to_string(&wrapper).expect("Serialize should succeed");
        let deserialized: PhaseWrapper =
            toml::from_str(&serialized).expect("Deserialize should succeed");
        assert_eq!(deserialized.phase, phase);
    }
}

#[test]
fn test_session_phase_serde_snake_case() {
    // Verify rename_all = "snake_case" produces expected strings
    let active_toml = toml::to_string(&PhaseWrapper {
        phase: SessionPhase::Active,
    })
    .unwrap();
    assert!(active_toml.contains("active"));

    let available_toml = toml::to_string(&PhaseWrapper {
        phase: SessionPhase::Available,
    })
    .unwrap();
    assert!(available_toml.contains("available"));

    let retired_toml = toml::to_string(&PhaseWrapper {
        phase: SessionPhase::Retired,
    })
    .unwrap();
    assert!(retired_toml.contains("retired"));
}

// ── Error message content ──────────────────────────────────────

#[test]
fn test_invalid_transition_error_contains_states() {
    let err = SessionPhase::Retired
        .transition(&PhaseEvent::Compressed)
        .unwrap_err();
    assert!(
        err.contains("Retired"),
        "Error should mention the current phase"
    );
    assert!(err.contains("Compressed"), "Error should mention the event");
}

// ── Default phase ──────────────────────────────────────────────

#[test]
fn test_default_phase_is_active() {
    let phase: SessionPhase = Default::default();
    assert_eq!(phase, SessionPhase::Active);
}

// ── MetaSessionState TOML round-trip ───────────────────────────

#[test]
fn test_meta_session_state_toml_roundtrip() {
    let now = chrono::Utc::now();
    let state = MetaSessionState {
        meta_session_id: ulid::Ulid::new().to_string(),
        description: Some("Round-trip test".to_string()),
        project_path: "/tmp/test".to_string(),
        branch: Some("feat/session-branch".to_string()),
        created_at: now,
        last_accessed: now,
        genealogy: Genealogy::default(),
        tools: HashMap::new(),
        context_status: ContextStatus::default(),
        total_token_usage: None,
        phase: SessionPhase::Available,
        task_context: TaskContext {
            task_type: Some("review".to_string()),
            tier_name: Some("quick".to_string()),
        },
        turn_count: 0,
        token_budget: None,
        sandbox_info: None,

        termination_reason: None,
        is_seed_candidate: false,
        git_head_at_creation: None,
        last_return_packet: None,
        fork_call_timestamps: Vec::new(),
    };

    let toml_str = toml::to_string_pretty(&state).expect("Serialize should succeed");
    let loaded: MetaSessionState =
        toml::from_str(&toml_str).expect("Deserialize should succeed");

    assert_eq!(loaded.meta_session_id, state.meta_session_id);
    assert_eq!(loaded.description, state.description);
    assert_eq!(loaded.branch, state.branch);
    assert_eq!(loaded.phase, SessionPhase::Available);
    assert_eq!(loaded.task_context.task_type, Some("review".to_string()));
    assert_eq!(loaded.task_context.tier_name, Some("quick".to_string()));
}

#[test]
fn test_meta_session_state_backward_compat_without_branch() {
    let toml_str = r#"
meta_session_id = "01J6F5W0M6Q7BW7Q3T0J4A8V45"
description = "Legacy session"
project_path = "/tmp/test"
created_at = "2026-01-01T00:00:00Z"
last_accessed = "2026-01-01T00:00:00Z"
turn_count = 0

[genealogy]
depth = 0

[tools]

[context_status]
is_compacted = false
"#;

    let loaded: MetaSessionState =
        toml::from_str(toml_str).expect("Deserialize legacy state should succeed");
    assert_eq!(loaded.branch, None);
    assert_eq!(loaded.last_return_packet, None);
}

#[test]
fn test_meta_session_state_backward_compat_without_last_return_packet() {
    let toml_str = r#"
meta_session_id = "01J6F5W0M6Q7BW7Q3T0J4A8V45"
description = "Legacy session"
project_path = "/tmp/test"
created_at = "2026-01-01T00:00:00Z"
last_accessed = "2026-01-01T00:00:00Z"
turn_count = 0

[genealogy]
depth = 0

[tools]

[context_status]
is_compacted = false
"#;

    let loaded: MetaSessionState =
        toml::from_str(toml_str).expect("Deserialize legacy state should succeed");
    assert_eq!(loaded.last_return_packet, None);
}

#[test]
fn test_meta_session_state_last_return_packet_roundtrip() {
    let now = chrono::Utc::now();
    let state = MetaSessionState {
        meta_session_id: ulid::Ulid::new().to_string(),
        description: Some("return-packet".to_string()),
        project_path: "/tmp/test".to_string(),
        branch: None,
        created_at: now,
        last_accessed: now,
        genealogy: Genealogy::default(),
        tools: HashMap::new(),
        context_status: ContextStatus::default(),
        total_token_usage: None,
        phase: SessionPhase::Active,
        task_context: TaskContext::default(),
        turn_count: 0,
        token_budget: None,
        sandbox_info: None,
        termination_reason: None,
        is_seed_candidate: false,
        git_head_at_creation: None,
        last_return_packet: Some(ReturnPacketRef {
            child_session_id: "01CHILDSESSIONID000000000000".to_string(),
            section_path: "/tmp/test-session/output/return-packet.md".to_string(),
        }),
        fork_call_timestamps: Vec::new(),
    };

    let toml_str = toml::to_string_pretty(&state).expect("serialize");
    let loaded: MetaSessionState = toml::from_str(&toml_str).expect("deserialize");
    assert_eq!(loaded.last_return_packet, state.last_return_packet);
}

#[test]
fn test_record_fork_call_attempt_rejects_eleventh_within_window() {
    let mut state = sample_state_with_phase(SessionPhase::Active);
    let base = Instant::now();

    for i in 0..FORK_CALL_RATE_LIMIT_MAX {
        state
            .record_fork_call_attempt(base + Duration::from_secs(i as u64))
            .expect("first ten attempts should pass");
    }

    let err = state
        .record_fork_call_attempt(base + Duration::from_secs(FORK_CALL_RATE_LIMIT_MAX as u64))
        .expect_err("11th attempt inside window should fail");
    assert!(
        err.contains("rate limit exceeded"),
        "error should indicate rate limiting"
    );
}

// ── Retired is terminal ────────────────────────────────────────

#[test]
fn test_retired_is_terminal_for_all_events() {
    let retired = SessionPhase::Retired;
    assert!(retired.transition(&PhaseEvent::Compressed).is_err());
    assert!(retired.transition(&PhaseEvent::Resumed).is_err());
    assert!(retired.transition(&PhaseEvent::Retired).is_err());
}

// ── TokenBudget ──────────────────────────────────────────────────

#[test]
fn test_token_budget_new_defaults() {
    let budget = TokenBudget::new(100_000);
    assert_eq!(budget.allocated, 100_000);
    assert_eq!(budget.used, 0);
    assert_eq!(budget.soft_threshold_pct, 75);
    assert_eq!(budget.hard_threshold_pct, 100);
    assert_eq!(budget.max_turns, None);
}

#[test]
fn test_token_budget_remaining() {
    let mut budget = TokenBudget::new(100_000);
    assert_eq!(budget.remaining(), 100_000);
    budget.record_usage(30_000);
    assert_eq!(budget.remaining(), 70_000);
    budget.record_usage(70_000);
    assert_eq!(budget.remaining(), 0);
}

#[test]
fn test_token_budget_remaining_saturates() {
    let mut budget = TokenBudget::new(100_000);
    budget.record_usage(200_000);
    assert_eq!(budget.remaining(), 0);
}

#[test]
fn test_token_budget_usage_pct() {
    let mut budget = TokenBudget::new(100_000);
    assert_eq!(budget.usage_pct(), 0);
    budget.record_usage(50_000);
    assert_eq!(budget.usage_pct(), 50);
    budget.record_usage(25_000);
    assert_eq!(budget.usage_pct(), 75);
    budget.record_usage(25_000);
    assert_eq!(budget.usage_pct(), 100);
}

#[test]
fn test_token_budget_usage_pct_zero_allocated() {
    let budget = TokenBudget::new(0);
    assert_eq!(budget.usage_pct(), 0);
}

#[test]
fn test_token_budget_soft_threshold() {
    let mut budget = TokenBudget::new(100_000);
    budget.record_usage(74_999);
    assert!(!budget.is_soft_exceeded());
    budget.record_usage(1);
    assert!(budget.is_soft_exceeded());
}

#[test]
fn test_token_budget_hard_threshold() {
    let mut budget = TokenBudget::new(100_000);
    budget.record_usage(99_999);
    assert!(!budget.is_hard_exceeded());
    budget.record_usage(1);
    assert!(budget.is_hard_exceeded());
}

#[test]
fn test_token_budget_custom_thresholds() {
    let mut budget = TokenBudget::new(100_000);
    budget.soft_threshold_pct = 50;
    budget.hard_threshold_pct = 80;

    budget.record_usage(49_999);
    assert!(!budget.is_soft_exceeded());
    budget.record_usage(1);
    assert!(budget.is_soft_exceeded());
    assert!(!budget.is_hard_exceeded());

    budget.record_usage(29_999);
    assert!(!budget.is_hard_exceeded());
    budget.record_usage(1);
    assert!(budget.is_hard_exceeded());
}

#[test]
fn test_token_budget_turns_exceeded() {
    let mut budget = TokenBudget::new(100_000);
    assert!(!budget.is_turns_exceeded(10));

    budget.max_turns = Some(5);
    assert!(!budget.is_turns_exceeded(4));
    assert!(budget.is_turns_exceeded(5));
    assert!(budget.is_turns_exceeded(10));
}

#[test]
fn test_token_budget_record_usage_saturates() {
    let mut budget = TokenBudget::new(100_000);
    budget.record_usage(u64::MAX);
    assert_eq!(budget.used, u64::MAX);
    budget.record_usage(1);
    assert_eq!(budget.used, u64::MAX); // saturating add
}

#[test]
fn test_token_budget_serde_roundtrip() {
    let mut budget = TokenBudget::new(200_000);
    budget.used = 50_000;
    budget.max_turns = Some(10);

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct BudgetWrapper {
        budget: TokenBudget,
    }

    let wrapper = BudgetWrapper {
        budget: budget.clone(),
    };
    let serialized = toml::to_string(&wrapper).expect("Serialize should succeed");
    let deserialized: BudgetWrapper =
        toml::from_str(&serialized).expect("Deserialize should succeed");
    assert_eq!(deserialized.budget, budget);
}

#[test]
fn test_token_budget_serde_defaults() {
    // Deserialize with missing optional fields — serde defaults should fill them
    let toml_str = r#"
        [budget]
        allocated = 100000
    "#;

    #[derive(Debug, Deserialize)]
    struct BudgetWrapper {
        budget: TokenBudget,
    }

    let wrapper: BudgetWrapper = toml::from_str(toml_str).expect("Deserialize should succeed");
    assert_eq!(wrapper.budget.allocated, 100_000);
    assert_eq!(wrapper.budget.used, 0);
    assert_eq!(wrapper.budget.soft_threshold_pct, 75);
    assert_eq!(wrapper.budget.hard_threshold_pct, 100);
    assert_eq!(wrapper.budget.max_turns, None);
}

#[test]
fn test_meta_session_state_with_budget_roundtrip() {
    let now = chrono::Utc::now();
    let mut budget = TokenBudget::new(150_000);
    budget.used = 30_000;
    budget.max_turns = Some(8);

    let state = MetaSessionState {
        meta_session_id: ulid::Ulid::new().to_string(),
        description: Some("Budget test".to_string()),
        project_path: "/tmp/test".to_string(),
        branch: None,
        created_at: now,
        last_accessed: now,
        genealogy: Genealogy::default(),
        tools: HashMap::new(),
        context_status: ContextStatus::default(),
        total_token_usage: None,
        phase: SessionPhase::Active,
        task_context: TaskContext::default(),
        turn_count: 3,
        token_budget: Some(budget.clone()),
        sandbox_info: None,

        termination_reason: None,
        is_seed_candidate: false,
        git_head_at_creation: None,
        last_return_packet: None,
        fork_call_timestamps: Vec::new(),
    };

    let toml_str = toml::to_string_pretty(&state).expect("Serialize should succeed");
    let loaded: MetaSessionState =
        toml::from_str(&toml_str).expect("Deserialize should succeed");

    assert_eq!(loaded.turn_count, 3);
    assert_eq!(loaded.token_budget, Some(budget));
}

// ── Genealogy fork fields ──────────────────────────────────────

#[test]
fn test_genealogy_backward_compat_without_fork_fields() {
    let toml_str = r#"
depth = 1
parent_session_id = "01PARENT"
"#;
    let genealogy: Genealogy =
        toml::from_str(toml_str).expect("should deserialize without fork fields");
    assert_eq!(genealogy.parent_session_id, Some("01PARENT".to_string()));
    assert_eq!(genealogy.depth, 1);
    assert_eq!(genealogy.fork_of_session_id, None);
    assert_eq!(genealogy.fork_provider_session_id, None);
    assert!(!genealogy.is_fork());
    assert_eq!(genealogy.fork_source(), None);
}

#[test]
fn test_genealogy_with_fork_fields_roundtrip() {
    let genealogy = Genealogy {
        parent_session_id: Some("01PARENT".to_string()),
        depth: 1,
        fork_of_session_id: Some("01SOURCE".to_string()),
        fork_provider_session_id: Some("provider-abc-123".to_string()),
    };

    let serialized = toml::to_string(&genealogy).expect("serialize");
    let deserialized: Genealogy = toml::from_str(&serialized).expect("deserialize");

    assert_eq!(deserialized.parent_session_id, Some("01PARENT".to_string()));
    assert_eq!(deserialized.depth, 1);
    assert_eq!(
        deserialized.fork_of_session_id,
        Some("01SOURCE".to_string())
    );
    assert_eq!(
        deserialized.fork_provider_session_id,
        Some("provider-abc-123".to_string())
    );
}

#[test]
fn test_genealogy_is_fork_true() {
    let genealogy = Genealogy {
        fork_of_session_id: Some("01SOURCE".to_string()),
        ..Default::default()
    };
    assert!(genealogy.is_fork());
    assert_eq!(genealogy.fork_source(), Some("01SOURCE"));
}

#[test]
fn test_genealogy_is_fork_false_for_spawn_child() {
    let genealogy = Genealogy {
        parent_session_id: Some("01PARENT".to_string()),
        depth: 1,
        ..Default::default()
    };
    assert!(!genealogy.is_fork());
    assert_eq!(genealogy.fork_source(), None);
}

#[test]
fn test_genealogy_skip_serializing_none_fork_fields() {
    let genealogy = Genealogy::default();
    let serialized = toml::to_string(&genealogy).expect("serialize");
    assert!(
        !serialized.contains("fork_of_session_id"),
        "None fork fields should be skipped in serialization"
    );
    assert!(!serialized.contains("fork_provider_session_id"));
}
