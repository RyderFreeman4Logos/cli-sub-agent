use csa_core::audit::AuditManifest;
use std::collections::BTreeMap;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ManifestDiff {
    pub new: Vec<String>,
    pub modified: Vec<String>,
    pub deleted: Vec<String>,
    pub unchanged: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DiffSummary {
    pub new: usize,
    pub modified: usize,
    pub deleted: usize,
    pub unchanged: usize,
}

impl ManifestDiff {
    pub(crate) fn summary(&self) -> DiffSummary {
        DiffSummary {
            new: self.new.len(),
            modified: self.modified.len(),
            deleted: self.deleted.len(),
            unchanged: self.unchanged.len(),
        }
    }
}

pub(crate) fn diff_manifest(
    manifest: &AuditManifest,
    current: &BTreeMap<String, String>,
) -> ManifestDiff {
    let mut diff = ManifestDiff::default();

    for (path, hash) in current {
        match manifest.files.get(path) {
            None => diff.new.push(path.clone()),
            Some(entry) if entry.hash != *hash => {
                // Modified files imply the caller should downgrade audit_status to Pending.
                diff.modified.push(path.clone());
            }
            Some(_) => diff.unchanged.push(path.clone()),
        }
    }

    for path in manifest.files.keys() {
        if !current.contains_key(path) {
            diff.deleted.push(path.clone());
        }
    }

    diff
}
