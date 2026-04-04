use super::*;

#[test]
fn context_load_options_with_skips_empty_returns_none() {
    let skip_files: Vec<String> = Vec::new();
    let options = context_load_options_with_skips(&skip_files);
    assert!(options.is_none());
}

#[test]
fn context_load_options_with_skips_propagates_files() {
    let skip_files = vec!["AGENTS.md".to_string(), "rules/private.md".to_string()];
    let options = context_load_options_with_skips(&skip_files).expect("must return options");
    assert_eq!(options.skip_files, skip_files);
    assert_eq!(options.max_bytes, None);
}

#[test]
fn result_toml_path_contract_extracts_embedded_path() {
    let temp = tempfile::tempdir().unwrap();
    let result_path = temp.path().join("result.toml");
    fs::write(&result_path, "status = \"success\"\nsummary = \"done\"\n").unwrap();

    let path_str = result_path.display().to_string();
    let mut result = ExecutionResult {
        output: format!("The result is at {path_str} and work is done.\n"),
        stderr_output: String::new(),
        summary: "completed all tasks successfully".to_string(),
        exit_code: 0,
    };

    enforce_result_toml_contract_now(
        "CSA_RESULT_TOML_PATH_CONTRACT=1",
        "",
        temp.path(),
        &mut result,
    );

    assert_eq!(
        result.exit_code, 0,
        "embedded path extraction must accept valid result.toml path within longer line"
    );
    assert!(result.stderr_output.is_empty());
}

#[test]
fn result_toml_path_contract_accepts_verified_session_result_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let result_path = temp.path().join("result.toml");
    fs::write(
        &result_path,
        "status = \"success\"\nsummary = \"task complete\"\n",
    )
    .unwrap();

    // Output and summary contain no path at all — simulates the scenario
    // where verbose output completely obscures the path.
    let mut result = ExecutionResult {
        output: "Step 1: analyzed code\nStep 2: wrote fixes\nStep 3: verified changes\n"
            .to_string(),
        stderr_output: String::new(),
        summary: "All tasks completed successfully, see session directory for details".to_string(),
        exit_code: 0,
    };

    enforce_result_toml_contract_now(
        "CSA_RESULT_TOML_PATH_CONTRACT=1",
        "",
        temp.path(),
        &mut result,
    );

    assert_eq!(
        result.exit_code, 0,
        "disk-based session result.toml fallback must accept verified file"
    );
    assert!(
        result
            .stderr_output
            .contains("contract warning: output/summary path not found")
    );
}

#[test]
fn result_toml_path_contract_handles_verbose_multiline_output() {
    let temp = tempfile::tempdir().unwrap();
    let result_path = temp.path().join("result.toml");
    fs::write(
        &result_path,
        "status = \"success\"\nsummary = \"implemented feature\"\n",
    )
    .unwrap();

    let path_str = result_path.display().to_string();

    // Simulate exact failure: many lines of verbose output, path appears
    // embedded in one line among many, and summary is long and truncated
    // past the path.
    let verbose_prefix = "Analyzing codebase structure and identifying patterns for refactoring. \
        Found 47 files matching the search criteria across 12 modules. \
        Applying changes to all affected files and running verification checks.";
    let verbose_lines = format!(
        "{verbose_prefix}\n\
         Processing module auth...\n\
         Processing module config...\n\
         Processing module session...\n\
         Writing result to {path_str} for contract verification.\n\
         Cleaning up temporary artifacts...\n\
         All verification checks passed.\n"
    );
    // Summary > 200 chars that does NOT contain the path.
    let long_summary = format!(
        "Completed comprehensive refactoring of authentication module including \
         session handling, token validation, error propagation, and test coverage \
         improvements across {0} files in {0} modules with full backward compatibility",
        "multiple"
    );
    assert!(
        long_summary.len() > 200,
        "test setup: summary must exceed 200 chars"
    );

    let mut result = ExecutionResult {
        output: verbose_lines,
        stderr_output: String::new(),
        summary: long_summary,
        exit_code: 0,
    };

    enforce_result_toml_contract_now(
        "CSA_RESULT_TOML_PATH_CONTRACT=1",
        "",
        temp.path(),
        &mut result,
    );

    assert_eq!(
        result.exit_code, 0,
        "verbose multiline output with embedded path must not trigger contract violation"
    );
    assert!(result.stderr_output.is_empty());
}

#[test]
fn result_toml_path_contract_accepts_disk_fallback_when_output_and_summary_are_empty() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("result.toml"), "status = \"success\"\n").unwrap();
    let mut result = ExecutionResult {
        output: " \n\t\n".to_string(),
        stderr_output: String::new(),
        summary: String::new(),
        exit_code: 0,
    };

    enforce_result_toml_contract_now(
        "CSA_RESULT_TOML_PATH_CONTRACT=1",
        "",
        temp.path(),
        &mut result,
    );

    // With disk-based fallback, a valid result.toml on disk is accepted even
    // when output/summary are empty. This fixes the contract violation bug
    // when verbose output truncates or omits the path.
    assert_eq!(result.exit_code, 0);
    assert!(
        result
            .stderr_output
            .contains("contract warning: output/summary path not found")
    );
}

#[test]
fn result_toml_path_contract_fails_when_output_summary_empty_and_no_disk_file() {
    let temp = tempfile::tempdir().unwrap();
    // No result.toml on disk at all.
    let mut result = ExecutionResult {
        output: " \n\t\n".to_string(),
        stderr_output: String::new(),
        summary: String::new(),
        exit_code: 0,
    };

    enforce_result_toml_contract_now(
        "CSA_RESULT_TOML_PATH_CONTRACT=1",
        "",
        temp.path(),
        &mut result,
    );

    assert_eq!(result.exit_code, 1);
    assert!(result.summary.contains("output and summary were empty"));
    assert!(result.stderr_output.contains("contract violation"));
}
