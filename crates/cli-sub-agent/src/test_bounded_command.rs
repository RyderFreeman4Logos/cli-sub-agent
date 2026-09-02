//! Compatibility wrappers for CLI tests; implementation lives in csa-resource.

use std::process::{Command, ExitStatus, Output};
use std::time::Duration;

/// CLI tests capture archives larger than the production `bwrap --help` cap.
const CLI_TEST_MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;

pub(crate) fn output_with_timeout(command: Command, timeout: Duration) -> Output {
    csa_resource::bounded_command::output_with_timeout(command, timeout, CLI_TEST_MAX_OUTPUT_BYTES)
        .unwrap_or_else(|error| panic!("bounded test command failed: {error}"))
}

pub(crate) fn status_with_timeout(command: Command, timeout: Duration) -> ExitStatus {
    output_with_timeout(command, timeout).status
}
