//! Bounded subprocess helpers for unit/integration tests (Rust 015).

#[cfg(test)]
use std::process::{Command, Output, Stdio};
#[cfg(test)]
use std::time::Duration;

#[cfg(test)]
pub(crate) fn output_with_timeout(mut command: Command, timeout: Duration) -> Output {
    use std::sync::mpsc;
    use std::thread;

    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = command.spawn().expect("spawn bounded test command");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(timeout) {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => panic!("bounded test command failed to wait: {error}"),
        Err(_) => panic!("bounded test command exceeded {timeout:?}"),
    }
}

#[cfg(test)]
pub(crate) fn status_with_timeout(command: Command, timeout: Duration) -> std::process::ExitStatus {
    output_with_timeout(command, timeout).status
}
