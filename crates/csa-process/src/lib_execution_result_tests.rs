use crate::{ExecutionResult, ProviderTurnCompletion, model_completed_from_terminal_reason};

fn execution_result(
    terminal_reason: Option<&str>,
    model_completed: Option<bool>,
    exit_signal: Option<i32>,
) -> ExecutionResult {
    ExecutionResult {
        terminal_reason: terminal_reason.map(str::to_owned),
        model_completed,
        exit_signal,
        ..Default::default()
    }
}

#[test]
fn provider_completion_contract_accepts_only_explicit_natural_reasons() {
    for reason in ["end_turn", "turn.completed", "success"] {
        assert_eq!(
            execution_result(Some(reason), None, None).provider_turn_completion(),
            ProviderTurnCompletion::Natural,
            "reason: {reason}"
        );
    }
}

#[test]
fn provider_completion_contract_separates_terminal_non_natural() {
    for reason in ["max_tokens", "max_turn_requests", "refusal"] {
        assert_eq!(
            execution_result(Some(reason), None, None).provider_turn_completion(),
            ProviderTurnCompletion::TerminalNonNatural,
            "reason: {reason}"
        );
    }
}

#[test]
fn provider_completion_contract_marks_interruption_and_signal_incomplete() {
    for reason in [
        "cancelled",
        "idle_timeout",
        "initial_response_timeout",
        "error",
        "failed",
    ] {
        assert_eq!(
            execution_result(Some(reason), None, None).provider_turn_completion(),
            ProviderTurnCompletion::Incomplete,
            "reason: {reason}"
        );
    }

    assert_eq!(
        execution_result(Some("end_turn"), Some(false), None).provider_turn_completion(),
        ProviderTurnCompletion::Incomplete
    );
    assert_eq!(
        execution_result(Some("success"), Some(true), Some(9)).provider_turn_completion(),
        ProviderTurnCompletion::Incomplete
    );
}

#[test]
fn provider_completion_contract_fails_closed_on_exit_zero_boolean_or_ambiguous_reason() {
    let mut exit_zero_with_output = execution_result(None, None, None);
    exit_zero_with_output.exit_code = 0;
    exit_zero_with_output.output = "finished".to_owned();
    assert_eq!(
        exit_zero_with_output.provider_turn_completion(),
        ProviderTurnCompletion::Unknown
    );

    for result in [
        execution_result(None, Some(true), None),
        execution_result(Some("provider_specific_done"), Some(true), None),
        execution_result(Some("completed"), Some(true), None),
    ] {
        assert_eq!(
            result.provider_turn_completion(),
            ProviderTurnCompletion::Unknown
        );
    }
}

#[test]
fn provider_completion_contract_preserves_legacy_boolean_mapping() {
    let cases = [
        (Some("end_turn"), Some(true)),
        (Some("max_tokens"), Some(true)),
        (Some("max_turn_requests"), Some(true)),
        (Some("refusal"), Some(true)),
        (Some("turn.completed"), Some(true)),
        (Some("completed"), Some(true)),
        (Some("success"), Some(true)),
        (Some("cancelled"), Some(false)),
        (Some("idle_timeout"), Some(false)),
        (Some("initial_response_timeout"), Some(false)),
        (Some("error"), Some(false)),
        (Some("failed"), Some(false)),
        (Some("provider_specific_done"), None),
        (None, None),
    ];

    for (reason, expected) in cases {
        assert_eq!(
            model_completed_from_terminal_reason(reason),
            expected,
            "reason: {reason:?}"
        );
    }
}

#[test]
fn provider_turn_completion_enum_serde_snake_case_roundtrip() {
    let cases = [
        (ProviderTurnCompletion::Natural, "natural"),
        (
            ProviderTurnCompletion::TerminalNonNatural,
            "terminal_non_natural",
        ),
        (ProviderTurnCompletion::Incomplete, "incomplete"),
        (ProviderTurnCompletion::Unknown, "unknown"),
    ];

    assert_eq!(
        ProviderTurnCompletion::default(),
        ProviderTurnCompletion::Unknown
    );

    for (value, name) in cases {
        let serialized = serde_json::to_string(&value).expect("enum should serialize");
        assert_eq!(serialized, format!("\"{name}\""));
        assert_eq!(
            serde_json::from_str::<ProviderTurnCompletion>(&serialized)
                .expect("enum should deserialize"),
            value
        );
    }
}
