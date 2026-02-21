//! Shared MCP hub library crate.

pub fn version_banner() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
