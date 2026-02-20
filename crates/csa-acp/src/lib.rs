pub mod client;
pub mod connection;
pub mod error;
pub mod mcp_proxy_client;
pub mod session_config;
pub mod transport;

pub use client::SessionEvent;
pub use connection::{AcpConnection, AcpConnectionOptions, AcpSandboxHandle, PromptIoOptions};
pub use error::{AcpError, AcpResult};
pub use session_config::{McpServerConfig, SessionConfig};
pub use transport::{AcpOutput, AcpOutputIoOptions, AcpRunOptions, AcpSession};
