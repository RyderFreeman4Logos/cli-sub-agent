use super::*;
use crate::transport_gemini_retry::*;
use csa_acp::SessionConfig;
use csa_resource::isolation_plan::IsolationPlan;

include!("transport_tests_tail.rs");
include!("transport_tests_ephemeral.rs");
include!("transport_tests_gemini_fallback.rs");
include!("transport_tests_gemini_init_classification.rs");
include!("transport_tests_gemini_acp_mcp_retry.rs");
include!("transport_tests_gemini_oauth_prompt.rs");
include!("transport_tests_extra.rs");
include!("transport_tests_codex_acp_stall.rs");
include!("transport_tests_capabilities.rs");
