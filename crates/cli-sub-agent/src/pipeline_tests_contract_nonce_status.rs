//! #2806 R6-001 path-contract regressions for non-success nonce-bound receipts.

use super::*;

#[test]
fn result_toml_path_contract_rejects_nonce_matching_non_success_receipt() {
    // R6-001: a current-nonce receipt with status=error/partial/etc. must not
    // satisfy the path contract (fail-closed; success-only positive completion).
    let temp = tempfile::tempdir().unwrap();
    let nonce_marker =
        crate::pipeline::result_contract::current_result_artifact_marker_path(temp.path());
    fs::write(&nonce_marker, "attempt_nonce = \"current-attempt\"\n").unwrap();
    let documented_paths = [
        csa_session::next_turn_contract_result_path(temp.path(), 0),
        temp.path().join("result.toml"),
        csa_session::contract_result_path(temp.path()),
        csa_session::legacy_user_result_path(temp.path()),
    ];

    for status in ["error", "partial", "needs_clarification"] {
        for path in &documented_paths {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(
                path,
                format!("[result]\nstatus = \"{status}\"\nattempt_nonce = \"current-attempt\"\n"),
            )
            .unwrap();
            let mut result = ExecutionResult {
                output: format!("{}\n", path.display()),
                exit_code: 0,
                peak_memory_mb: None,
                ..Default::default()
            };

            enforce_result_toml_path_contract(
                "CSA_RESULT_TOML_PATH_CONTRACT=1",
                "",
                temp.path(),
                0,
                true,
                &mut result,
            );
            assert_eq!(
                result.exit_code,
                1,
                "nonce-matching status={status} receipt must not satisfy contract: {}",
                path.display()
            );
            fs::remove_file(path).unwrap();
        }
    }
}
