use anyhow::Error;
use std::{error::Error as StdError, fmt};

pub(crate) struct RoutingConflict;

impl fmt::Debug for RoutingConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RoutingConflict")
    }
}

impl fmt::Display for RoutingConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("routing conflict")
    }
}

impl StdError for RoutingConflict {}

pub(crate) fn routing_conflict_error(message: impl Into<String>) -> Error {
    Error::new(RoutingConflict).context(message.into())
}

pub(crate) fn is_routing_conflict(err: &Error) -> bool {
    err.chain()
        .any(|cause| cause.downcast_ref::<RoutingConflict>().is_some())
}
