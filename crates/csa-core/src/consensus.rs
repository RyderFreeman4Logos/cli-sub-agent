//! Thin consensus re-exports from agent-teams.

pub use agent_teams::consensus::{
    AgentResponse, ConsensusRequest, ConsensusResult, ConsensusStrategy, resolve,
    resolve_human_in_the_loop, resolve_majority, resolve_unanimous, resolve_weighted,
};
