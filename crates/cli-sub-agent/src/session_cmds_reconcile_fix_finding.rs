use std::path::Path;

use csa_session::MetaSessionState;

use super::reconcile_diagnostics::synthetic_failure_diagnostics;

pub(super) fn missing_result_summary_prefix(
    project_root: &Path,
    session: &MetaSessionState,
    session_dir: &Path,
    trigger: &str,
    output_log_mtime: &str,
    liveness_reason: &str,
) -> String {
    let diagnostics = synthetic_failure_diagnostics(session_dir, session, liveness_reason);
    let default = || {
        format!(
            "synthetic failure by {trigger}: process dead, result.toml missing \
             (reconciliation_reason=true_missing_result, output_log_mtime={output_log_mtime})\
             {diagnostics}"
        )
    };
    if !crate::session_fix_finding_recovery::is_fix_finding_session(session) {
        return default();
    }

    let side_effects =
        crate::session_fix_finding_recovery::side_effect_diagnostic(project_root, session);
    format!(
        "`csa review --fix-finding` session failed closed: process dead before writing \
         result.toml for fix session {} (output_log_mtime={output_log_mtime}). \
         The original failed review verdict is not a fix-session result. {side_effects}. \
         Recovery: inspect `git status --short`, `git diff`, and `git diff --staged`; \
         preserve/finish or discard dirty side effects; create a hook-enabled commit if \
         appropriate; then run a fresh exact-head `csa review` before push/PR. \
         Diagnostics are from {}.\
         {diagnostics}",
        session.meta_session_id,
        session_dir.display()
    )
}
