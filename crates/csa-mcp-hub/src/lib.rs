//! Shared MCP hub implementation used by the csa CLI wrapper.

mod config;
mod proxy;
mod registry;
mod serve;
mod socket;

pub use serve::{handle_serve_command, handle_status_command, handle_stop_command};
