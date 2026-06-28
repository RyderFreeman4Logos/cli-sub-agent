use std::{fs, path::Path};

use anyhow::{Context, Result};
use csa_session::FindingsFile;

pub(super) fn review_findings_toml_has_findings(session_dir: &Path) -> Result<bool> {
    let findings_path = session_dir.join("output").join("findings.toml");
    if !findings_path.is_file() {
        return Ok(false);
    }
    let raw = fs::read_to_string(&findings_path)
        .with_context(|| format!("failed to read {}", findings_path.display()))?;
    let findings: FindingsFile = toml::from_str(&raw)
        .with_context(|| format!("failed to parse {}", findings_path.display()))?;
    Ok(!findings.findings.is_empty())
}
