use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::process::Command;

use crate::model_spec::ThinkingBudget;

const HERMES_FINGERPRINT_FILE: &str = "hermes-run-config.sha256";

/// Per-run Hermes selection passed to the ACP adapter.
///
/// Hermes reads provider/model/thinking through its ACP session metadata, not
/// by mutating the user's global `~/.hermes/config.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HermesRunConfig {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub thinking: Option<ThinkingBudget>,
}

impl HermesRunConfig {
    pub fn new(
        provider: Option<String>,
        model: Option<String>,
        thinking: Option<ThinkingBudget>,
    ) -> Self {
        Self {
            provider,
            model,
            thinking,
        }
    }

    pub fn from_model_spec(
        provider: String,
        model: String,
        thinking: ThinkingBudget,
    ) -> Self {
        Self::new(Some(provider), Some(model), Some(thinking))
    }

    pub fn meta_options(&self) -> Option<serde_json::Value> {
        let mut options = serde_json::Map::new();
        if let Some(provider) = self.provider.as_deref().filter(|value| !value.is_empty()) {
            options.insert("provider".to_string(), provider.into());
        }
        if let Some(model) = self.model.as_deref().filter(|value| !value.is_empty()) {
            options.insert("model".to_string(), model.into());
        }
        if let Some(thinking) = &self.thinking {
            options.insert("thinking".to_string(), thinking_label(thinking).into());
        }

        (!options.is_empty()).then_some(serde_json::Value::Object(options))
    }

    pub fn fingerprint(&self) -> String {
        let payload =
            serde_json::to_vec(self).expect("HermesRunConfig serialization should not fail");
        format!("{:x}", Sha256::digest(payload))
    }
}

fn thinking_label(thinking: &ThinkingBudget) -> String {
    match thinking {
        ThinkingBudget::DefaultBudget => "default".to_string(),
        ThinkingBudget::Low => "low".to_string(),
        ThinkingBudget::Medium => "medium".to_string(),
        ThinkingBudget::High => "high".to_string(),
        ThinkingBudget::Xhigh => "xhigh".to_string(),
        ThinkingBudget::Max => "max".to_string(),
        ThinkingBudget::Custom(value) => value.to_string(),
    }
}

pub(crate) fn filter_resume_session_id_for_hermes(
    session_dir: Option<&Path>,
    run_config: Option<&HermesRunConfig>,
    resume_session_id: Option<String>,
) -> Option<String> {
    let resume_session_id = resume_session_id?;
    let Some(run_config) = run_config else {
        return Some(resume_session_id);
    };
    let session_dir = session_dir?;
    if stored_fingerprint_matches(session_dir, &run_config.fingerprint()) {
        Some(resume_session_id)
    } else {
        None
    }
}

pub(crate) fn persist_hermes_fingerprint(
    session_dir: Option<&Path>,
    run_config: Option<&HermesRunConfig>,
) -> Result<()> {
    let Some(session_dir) = session_dir else {
        return Ok(());
    };
    let Some(run_config) = run_config else {
        return Ok(());
    };
    std::fs::write(
        fingerprint_path(session_dir),
        format!("{}\n", run_config.fingerprint()),
    )
    .with_context(|| {
        format!(
            "failed to write Hermes run-config fingerprint under {}",
            session_dir.display()
        )
    })
}

fn stored_fingerprint_matches(session_dir: &Path, expected: &str) -> bool {
    std::fs::read_to_string(fingerprint_path(session_dir))
        .map(|value| value.trim() == expected)
        .unwrap_or(false)
}

fn fingerprint_path(session_dir: &Path) -> PathBuf {
    session_dir.join(HERMES_FINGERPRINT_FILE)
}

pub(crate) async fn run_hermes_acp_check(
    command: &str,
    args: &[String],
    working_dir: &Path,
    env: &std::collections::HashMap<String, String>,
) -> Result<()> {
    let mut cmd = Command::new(command);
    cmd.args(args)
        .arg("--check")
        .current_dir(working_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for (key, value) in env {
        cmd.env(key, value);
    }

    let output = cmd
        .output()
        .await
        .with_context(|| format!("failed to run Hermes ACP preflight `{}`", check_label(args)))?;
    if output.status.success() {
        return Ok(());
    }

    let code = output
        .status
        .code()
        .map_or_else(|| "signal".to_string(), |code| code.to_string());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    Err(anyhow!(
        "Hermes ACP preflight failed: `{}` exited with {code}\nstdout:\n{}\nstderr:\n{}",
        check_label(args),
        stdout.trim(),
        stderr.trim()
    ))
}

fn check_label(args: &[String]) -> String {
    if args.is_empty() {
        "hermes --check".to_string()
    } else {
        format!("hermes {} --check", args.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_changes_when_provider_changes() {
        let first = HermesRunConfig::from_model_spec(
            "openai".to_string(),
            "gpt-5.5".to_string(),
            ThinkingBudget::Xhigh,
        );
        let second = HermesRunConfig::from_model_spec(
            "anthropic".to_string(),
            "gpt-5.5".to_string(),
            ThinkingBudget::Xhigh,
        );

        assert_ne!(first.fingerprint(), second.fingerprint());
    }

    #[test]
    fn resume_is_filtered_when_fingerprint_mismatches() {
        let dir = tempfile::tempdir().expect("tempdir");
        let first = HermesRunConfig::from_model_spec(
            "openai".to_string(),
            "gpt-5.5".to_string(),
            ThinkingBudget::Xhigh,
        );
        let second = HermesRunConfig::from_model_spec(
            "anthropic".to_string(),
            "claude-opus".to_string(),
            ThinkingBudget::High,
        );
        persist_hermes_fingerprint(Some(dir.path()), Some(&first)).expect("write fingerprint");

        let resume = filter_resume_session_id_for_hermes(
            Some(dir.path()),
            Some(&second),
            Some("provider-session".to_string()),
        );

        assert_eq!(resume, None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn acp_check_failure_surfaces_structured_error() {
        use std::collections::HashMap;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let hermes = dir.path().join("hermes");
        std::fs::write(&hermes, "#!/bin/sh\necho check-failed >&2\nexit 42\n")
            .expect("write fake hermes");
        let mut perms = std::fs::metadata(&hermes)
            .expect("fake hermes metadata")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&hermes, perms).expect("chmod fake hermes");

        let args = vec!["acp".to_string()];
        let error = run_hermes_acp_check(
            hermes.to_str().expect("utf8 path"),
            &args,
            dir.path(),
            &HashMap::new(),
        )
        .await
        .expect_err("check failure should propagate");

        let message = format!("{error:#}");
        assert!(message.contains("Hermes ACP preflight failed"), "{message}");
        assert!(message.contains("hermes acp --check"), "{message}");
        assert!(message.contains("42"), "{message}");
        assert!(message.contains("check-failed"), "{message}");
    }
}
