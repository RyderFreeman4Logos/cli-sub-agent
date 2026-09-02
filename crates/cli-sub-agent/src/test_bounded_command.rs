//! Compatibility wrappers for CLI tests; implementation lives in csa-resource.

use std::process::{Command, ExitStatus, Output};
use std::time::Duration;

pub(crate) fn output_with_timeout(command: Command, timeout: Duration) -> Output {
    csa_resource::bounded_command::output_with_timeout(command, timeout)
        .unwrap_or_else(|error| panic!("bounded test command failed: {error}"))
}

pub(crate) fn status_with_timeout(command: Command, timeout: Duration) -> ExitStatus {
    output_with_timeout(command, timeout).status
}
