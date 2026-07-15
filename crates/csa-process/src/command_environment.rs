//! Typed deterministic child-process environment contract.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use csa_resource::isolation_plan::IsolationPlan;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvironmentInheritance {
    InheritParent,
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClearedCommandEnvironment {
    entries: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanEnvironmentError {
    InvalidKey(String),
    InvalidValue(String),
    ReservedKey(String),
    ConflictingValue(String),
    UnresolvableProgram(String),
}

impl fmt::Display for CleanEnvironmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidKey(key) => write!(formatter, "invalid environment key: {key:?}"),
            Self::InvalidValue(key) => {
                write!(formatter, "environment value for {key:?} contains NUL")
            }
            Self::ReservedKey(key) => write!(formatter, "reserved environment key: {key}"),
            Self::ConflictingValue(key) => {
                write!(
                    formatter,
                    "conflicting clean-room environment value for {key}"
                )
            }
            Self::UnresolvableProgram(program) => write!(
                formatter,
                "relative program {program:?} is not resolvable through explicit PATH"
            ),
        }
    }
}

impl std::error::Error for CleanEnvironmentError {}

impl ClearedCommandEnvironment {
    pub fn try_new(
        entries: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, CleanEnvironmentError> {
        let mut sorted = BTreeMap::new();
        for (key, value) in entries {
            if let Some(existing) = sorted.get(&key)
                && existing != &value
            {
                return Err(CleanEnvironmentError::ConflictingValue(key));
            }
            sorted.insert(key, value);
        }
        validate_entries(&sorted)?;
        Ok(Self { entries: sorted })
    }

    pub const fn inheritance(&self) -> EnvironmentInheritance {
        EnvironmentInheritance::Clear
    }

    pub fn entries(&self) -> &BTreeMap<String, String> {
        &self.entries
    }

    pub fn effective_entries(
        &self,
        isolation_plan: Option<&IsolationPlan>,
    ) -> Result<BTreeMap<String, String>, CleanEnvironmentError> {
        let mut effective = self.entries.clone();
        let Some(plan) = isolation_plan else {
            return Ok(effective);
        };
        let plan_entries = plan
            .env_overrides
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<BTreeMap<_, _>>();
        validate_entries(&plan_entries)?;
        for (key, value) in plan_entries {
            if let Some(existing) = effective.get(&key)
                && existing != &value
            {
                return Err(CleanEnvironmentError::ConflictingValue(key));
            }
            effective.insert(key, value);
        }
        Ok(effective)
    }
}

fn validate_entries(entries: &BTreeMap<String, String>) -> Result<(), CleanEnvironmentError> {
    for (key, value) in entries {
        if key.is_empty() || key.contains(['=', '\0']) {
            return Err(CleanEnvironmentError::InvalidKey(key.clone()));
        }
        if value.contains('\0') {
            return Err(CleanEnvironmentError::InvalidValue(key.clone()));
        }
        if csa_core::env::GIT_PUSH_AUTHORIZATION_ENV_KEYS.contains(&key.as_str())
            || csa_core::env::SUBTREE_PIN_ENV_KEYS.contains(&key.as_str())
        {
            return Err(CleanEnvironmentError::ReservedKey(key.clone()));
        }
    }
    Ok(())
}

pub(crate) fn apply_cleared_environment(command: &mut Command, entries: &BTreeMap<String, String>) {
    command.env_clear();
    command.envs(entries);
}

pub(crate) fn validate_program(
    program: &OsStr,
    entries: &BTreeMap<String, String>,
) -> Result<(), CleanEnvironmentError> {
    let program_path = Path::new(program);
    if program_path.is_absolute() {
        return Ok(());
    }
    let Some(path) = entries.get("PATH") else {
        return Err(CleanEnvironmentError::UnresolvableProgram(
            program.to_string_lossy().into_owned(),
        ));
    };
    let resolved = std::env::split_paths(path)
        .any(|directory| directory.is_absolute() && is_executable(directory.join(program_path)));
    if resolved {
        Ok(())
    } else {
        Err(CleanEnvironmentError::UnresolvableProgram(
            program.to_string_lossy().into_owned(),
        ))
    }
}

#[cfg(unix)]
fn is_executable(path: PathBuf) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: PathBuf) -> bool {
    path.is_file()
}
