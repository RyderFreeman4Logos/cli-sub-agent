use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use super::secure_fs::{self, SecureDirectory};
use super::{ConvergenceLedgerStore, Sha256Digest};
use crate::atomic_state_write;

const BUNDLES_DIRECTORY: &str = "provider-bundles";
const BUNDLE_FILE_NAME: &str = "provider-evidence.tar";
const MAX_PROVIDER_BUNDLE_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ProviderEvidenceBundle {
    secure_boundary: PathBuf,
    project_state_root: PathBuf,
    digest: Sha256Digest,
}

impl ProviderEvidenceBundle {
    #[must_use]
    pub fn digest(&self) -> &Sha256Digest {
        &self.digest
    }

    #[must_use]
    pub fn root(&self) -> PathBuf {
        self.project_state_root
            .join("convergence")
            .join(BUNDLES_DIRECTORY)
            .join(self.digest.as_str())
    }

    #[must_use]
    pub fn path(&self) -> PathBuf {
        self.root().join(BUNDLE_FILE_NAME)
    }

    pub fn verify(&self) -> Result<Vec<u8>> {
        let bundle_directory = open_bundle_directory(
            &self.secure_boundary,
            &self.project_state_root,
            self.digest.as_str(),
            false,
        )?
        .context("provider evidence bundle directory is missing")?;
        bundle_directory.verify_only_entry(bundle_file_name())?;
        let file = bundle_directory
            .open_private_file(bundle_file_name())?
            .context("provider evidence bundle file is missing")?;
        let bytes = read_bounded(file, &self.path())?;
        let actual = Sha256Digest::compute(&bytes);
        if actual != self.digest {
            bail!(
                "provider evidence bundle digest mismatch: expected {}, got {actual}",
                self.digest
            );
        }
        bundle_directory.verify_link()?;
        Ok(bytes)
    }
}

impl ConvergenceLedgerStore {
    pub fn publish_provider_evidence_bundle(
        &self,
        contents: &[u8],
    ) -> Result<ProviderEvidenceBundle> {
        let digest = Sha256Digest::compute(contents);
        let bundle_directory = open_bundle_directory(
            self.secure_boundary(),
            self.project_state_root(),
            digest.as_str(),
            true,
        )?
        .context("secure provider evidence directory was not created")?;
        let target_path = bundle_directory.path().join(BUNDLE_FILE_NAME);
        match bundle_directory.open_private_file(bundle_file_name())? {
            Some(existing) => {
                bundle_directory.verify_only_entry(bundle_file_name())?;
                let existing_bytes = read_bounded(existing, &target_path)?;
                if existing_bytes != contents {
                    bail!(
                        "content-addressed provider evidence bundle does not match existing bytes: {}",
                        target_path.display()
                    );
                }
            }
            None => {
                bundle_directory.verify_empty()?;
                atomic_state_write::publish_bytes_in(
                    bundle_directory.file(),
                    Some(bundle_directory.parent()),
                    bundle_file_name(),
                    &target_path,
                    contents,
                )
                .map_err(|error| anyhow!(error))?;
            }
        }
        bundle_directory.verify_only_entry(bundle_file_name())?;
        let published = ProviderEvidenceBundle {
            secure_boundary: self.secure_boundary().to_path_buf(),
            project_state_root: self.project_state_root().to_path_buf(),
            digest,
        };
        published.verify()?;
        Ok(published)
    }
}

fn open_bundle_directory(
    secure_boundary: &Path,
    project_state_root: &Path,
    digest: &str,
    create: bool,
) -> Result<Option<SecureDirectory>> {
    let Some(convergence) =
        secure_fs::open_convergence_directory(secure_boundary, project_state_root, create)?
    else {
        return Ok(None);
    };
    let Some(bundles) =
        convergence.open_private_subdirectory(OsStr::new(BUNDLES_DIRECTORY), create)?
    else {
        return Ok(None);
    };
    bundles.open_private_subdirectory(OsStr::new(digest), create)
}

fn bundle_file_name() -> &'static OsStr {
    OsStr::new(BUNDLE_FILE_NAME)
}

fn read_bounded(file: std::fs::File, path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.take(MAX_PROVIDER_BUNDLE_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read provider evidence bundle {}", path.display()))?;
    let size = u64::try_from(bytes.len()).context("provider evidence bundle size overflow")?;
    if size > MAX_PROVIDER_BUNDLE_BYTES {
        bail!(
            "provider evidence bundle exceeds {} bytes: {}",
            MAX_PROVIDER_BUNDLE_BYTES,
            path.display()
        );
    }
    Ok(bytes)
}
