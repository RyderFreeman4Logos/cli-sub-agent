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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_toml_round_trip() {
        let mut files = BTreeMap::new();
        files.insert(
            "src/lib.rs".to_string(),
            FileEntry {
                hash: "sha256:111".to_string(),
                audit_status: AuditStatus::Pending,
                blog_path: None,
                auditor: Some("auditor-a".to_string()),
                approved_by: None,
                approved_at: None,
            },
        );
        files.insert(
            "src/main.rs".to_string(),
            FileEntry {
                hash: "sha256:222".to_string(),
                audit_status: AuditStatus::Approved,
                blog_path: Some("blog/post.md".to_string()),
                auditor: Some("auditor-b".to_string()),
                approved_by: Some("human".to_string()),
                approved_at: Some("2026-02-19T00:00:00Z".to_string()),
            },
        );

        let manifest = AuditManifest {
            meta: ManifestMeta {
                version: 1,
                project_root: ".".to_string(),
                created_at: "2026-02-19T00:00:00Z".to_string(),
                updated_at: "2026-02-19T00:01:00Z".to_string(),
                last_scanned_at: Some("2026-02-19T00:02:00Z".to_string()),
            },
            files,
        };

        let toml = toml::to_string_pretty(&manifest).expect("manifest should serialize");
        let parsed: AuditManifest = toml::from_str(&toml).expect("manifest should deserialize");
        assert_eq!(parsed, manifest);
    }

    #[test]
    fn test_audit_status_serde() {
        for (raw, expected) in [
            ("pending", AuditStatus::Pending),
            ("generated", AuditStatus::Generated),
            ("approved", AuditStatus::Approved),
        ] {
            #[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
            struct Wrapper {
                status: AuditStatus,
            }

            let parsed: Wrapper =
                toml::from_str(&format!("status = \"{raw}\"")).expect("status should deserialize");
            assert_eq!(parsed.status, expected);

            let serialized =
                toml::to_string(&Wrapper { status: expected }).expect("status should serialize");
            assert!(
                serialized.contains(&format!("status = \"{raw}\"")),
                "serialized status should be lowercase"
            );
        }
    }

    #[test]
    fn test_default_values() {
        let empty_manifest_toml = r#"
[meta]
version = 1
project_root = "."
created_at = "2026-02-19T00:00:00Z"
updated_at = "2026-02-19T00:01:00Z"
"#;
        let empty_manifest: AuditManifest =
            toml::from_str(empty_manifest_toml).expect("empty manifest should deserialize");
        assert!(empty_manifest.files.is_empty());

        let default_status_toml = r#"
[meta]
version = 1
project_root = "."
created_at = "2026-02-19T00:00:00Z"
updated_at = "2026-02-19T00:01:00Z"

[files."src/lib.rs"]
hash = "sha256:abc"
"#;
        let manifest: AuditManifest =
            toml::from_str(default_status_toml).expect("manifest should deserialize");
        let entry = manifest
            .files
            .get("src/lib.rs")
            .expect("expected src/lib.rs entry");
        assert_eq!(entry.audit_status, AuditStatus::Pending);
        assert_eq!(entry.blog_path, None);
        assert_eq!(entry.approved_by, None);
    }

    #[test]
    fn test_btreemap_ordering() {
        let mut manifest = AuditManifest::new(".");
        manifest.meta.created_at = "2026-02-19T00:00:00Z".to_string();
        manifest.meta.updated_at = "2026-02-19T00:01:00Z".to_string();
        manifest.files.insert(
            "z.txt".to_string(),
            FileEntry {
                hash: "sha256:z".to_string(),
                audit_status: AuditStatus::Pending,
                blog_path: None,
                auditor: None,
                approved_by: None,
                approved_at: None,
            },
        );
        manifest.files.insert(
            "a.txt".to_string(),
            FileEntry {
                hash: "sha256:a".to_string(),
                audit_status: AuditStatus::Pending,
                blog_path: None,
                auditor: None,
                approved_by: None,
                approved_at: None,
            },
        );
        manifest.files.insert(
            "m.txt".to_string(),
            FileEntry {
                hash: "sha256:m".to_string(),
                audit_status: AuditStatus::Pending,
                blog_path: None,
                auditor: None,
                approved_by: None,
                approved_at: None,
            },
        );

        let toml = toml::to_string_pretty(&manifest).expect("manifest should serialize");
        let a_pos = toml
            .find("[files.\"a.txt\"]")
            .expect("a.txt table should exist");
        let m_pos = toml
            .find("[files.\"m.txt\"]")
            .expect("m.txt table should exist");
        let z_pos = toml
            .find("[files.\"z.txt\"]")
            .expect("z.txt table should exist");

        assert!(a_pos < m_pos);
        assert!(m_pos < z_pos);
    }
}
