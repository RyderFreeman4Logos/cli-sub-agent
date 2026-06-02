use std::io::Write;

use anyhow::Result;

pub(crate) fn report_daemon_error_or_exit_code(
    result: Result<i32>,
    daemon_guard: &mut crate::run_cmd_daemon::DaemonChildGuard,
) -> i32 {
    match result {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{}", crate::error_report::render_user_facing_error(&err));
            if let Some(hint) = crate::error_hints::suggest_fix(&err) {
                eprintln!();
                eprintln!("{hint}");
            }
            daemon_guard.finalize();
            exit_current_process(1);
        }
    }
}

pub(crate) fn exit_current_process(exit_code: i32) -> ! {
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    crate::session_cmds_daemon::persist_daemon_completion_from_env(exit_code);
    std::process::exit(exit_code);
}
