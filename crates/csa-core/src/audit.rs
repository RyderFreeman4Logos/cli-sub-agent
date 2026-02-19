use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Top-level audit manifest stored at .csa/audit/manifest.toml
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuditManifest {
    pub meta: ManifestMeta,
    #[serde(default)]
    pub files: BTreeMap<String, FileEntry>,
}

/// Manifest metadata
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ManifestMeta {
    pub version: u32,
    #[serde(default = "default_project_root")]
    pub project_root: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub last_scanned_at: Option<String>,
}

fn default_project_root() -> String {
    ".".to_string()
}

/// Per-file audit tracking entry
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileEntry {
    pub hash: String,
    #[serde(default)]
    pub audit_status: AuditStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blog_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auditor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<String>,
}

/// Audit status for a tracked file
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuditStatus {
    #[default]
    Pending,
    Generated,
    Approved,
}

impl fmt::Display for AuditStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Generated => write!(f, "generated"),
            Self::Approved => write!(f, "approved"),
        }
    }
}

impl AuditManifest {
    /// Create a new empty manifest with default metadata
    pub fn new(project_root: impl Into<String>) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            meta: ManifestMeta {
                version: 1,
                project_root: project_root.into(),
                created_at: now.clone(),
                updated_at: now,
                last_scanned_at: None,
            },
            files: BTreeMap::new(),
        }
    }
}
