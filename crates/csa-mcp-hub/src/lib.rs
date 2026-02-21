//! Shared MCP hub implementation used by the csa CLI wrapper.

mod config;
mod proxy;
mod registry;
mod serve;
mod skill_writer;
mod socket;

pub use serve::{
    handle_gen_skill_command, handle_serve_command, handle_status_command, handle_stop_command,
};
