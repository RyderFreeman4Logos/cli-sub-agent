//! Typed command-isolation policy for native clean-room execution.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use csa_process::{CleanEnvironmentError, ClearedCommandEnvironment};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbsoluteProgram(PathBuf);

impl AbsoluteProgram {
    pub fn try_new(program: impl Into<PathBuf>) -> Result<Self, CommandIsolationError> {
        let program = program.into();
        if program.as_os_str().is_empty()
            || !program.is_absolute()
            || program.as_os_str().as_encoded_bytes().contains(&0)
        {
            return Err(CommandIsolationError::InvalidProgram(
                program.display().to_string(),
            ));
        }
        Ok(Self(program))
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExactPromptDelivery(());

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanCommandContract {
    program: AbsoluteProgram,
    environment: ClearedCommandEnvironment,
    prompt_delivery: ExactPromptDelivery,
}

impl CleanCommandContract {
    pub fn try_new(
        program: impl Into<PathBuf>,
        explicit_environment: BTreeMap<String, String>,
    ) -> Result<Self, CommandIsolationError> {
        Ok(Self {
            program: AbsoluteProgram::try_new(program)?,
            environment: ClearedCommandEnvironment::try_new(explicit_environment)?,
            prompt_delivery: ExactPromptDelivery::default(),
        })
    }

    pub fn program(&self) -> &AbsoluteProgram {
        &self.program
    }

    pub fn environment(&self) -> &ClearedCommandEnvironment {
        &self.environment
    }

    pub fn prompt_delivery(&self) -> ExactPromptDelivery {
        self.prompt_delivery
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandIsolationPolicy {
    Legacy,
    CleanRoom(CleanCommandContract),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanRoomCapability {
    Unsupported { reason: &'static str },
    ExactPromptAndClearedEnvironment,
}

#[derive(Debug, thiserror::Error)]
pub enum CommandIsolationError {
    #[error("clean-room program must be a non-empty absolute path without NUL: {0:?}")]
    InvalidProgram(String),
    #[error("clean-room transport is unsupported: {reason}")]
    Unsupported { reason: &'static str },
    #[error("invalid clean-room request: {reason}")]
    InvalidRequest { reason: &'static str },
    #[error(transparent)]
    Environment(#[from] CleanEnvironmentError),
}
