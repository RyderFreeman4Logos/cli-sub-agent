use crate::transport_gemini_oauth::GEMINI_OAUTH_PROMPT_SUMMARY;

#[test]
fn test_detect_gemini_oauth_prompt_result_handles_guarded_browser_prompt_variant() {
    let execution = ExecutionResult {
        summary: "Opening authentication page in your browser. Do you want to continue? [Y/n]:"
            .to_string(),
        output: concat!(
            "<csa-caller-sa-guard>\n",
            "SA MODE ACTIVE — You are Layer 0 Manager (pure orchestrator).\n",
            "</csa-caller-sa-guard>\n",
            "\n",
            "Opening authentication page in your browser. Do you want to continue? [Y/n]: ",
            "<csa-caller-sa-guard>\n",
            "SA MODE ACTIVE — You are Layer 0 Manager (pure orchestrator).\n",
            "</csa-caller-sa-guard>\n",
        )
        .to_string(),
        stderr_output: concat!(
            "WARNING: weave.lock records stale version stamp(s)\n",
            "[stdout] Opening authentication page in your browser. Do you want to continue? [Y/n]: \n",
        )
        .to_string(),
        exit_code: 0,
        peak_memory_mb: None,
        ..Default::default()
    };

    assert!(is_gemini_oauth_prompt_result(&execution));
}

#[test]
fn test_classify_gemini_oauth_prompt_result_marks_auth_failure_summary() {
    let mut execution = ExecutionResult {
        output: "Opening authentication page in your browser. Do you want to continue? [Y/n]:"
            .to_string(),
        exit_code: 0,
        peak_memory_mb: None,
        ..Default::default()
    };

    classify_gemini_oauth_prompt_result(&mut execution);

    assert_eq!(execution.exit_code, 1);
    assert_eq!(execution.summary, GEMINI_OAUTH_PROMPT_SUMMARY);
    assert!(execution.stderr_output.contains(GEMINI_OAUTH_PROMPT_SUMMARY));
}
