#[path = "lib_wait_after_capture_loop.rs"]
mod wait_after_capture_loop;

pub(crate) use wait_after_capture_loop::{OutputEofWait, wait_after_output_eof};
