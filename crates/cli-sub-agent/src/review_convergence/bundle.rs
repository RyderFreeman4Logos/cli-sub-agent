use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{Context, Result, bail};
use csa_session::convergence::{GitObjectId, ProviderEvidenceBundle, Sha256Digest};
use serde::{Deserialize, Serialize};

const BUNDLE_SCHEMA_VERSION: u32 = 1;
const BUNDLE_KIND: &str = "convergence_exact_oid_provider_evidence";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProviderEvidenceIdentity {
    pub(crate) base_oid: String,
    pub(crate) head_oid: String,
    pub(crate) diff_digest: Sha256Digest,
    pub(crate) bundle_digest: Sha256Digest,
    pub(crate) bundle_file: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderEvidenceRef {
    pub(crate) identity: ProviderEvidenceIdentity,
    pub(crate) root: PathBuf,
    pub(crate) path: PathBuf,
}

impl ProviderEvidenceRef {
    fn from_published(
        evidence: &ExactOidEvidence,
        published: &ProviderEvidenceBundle,
    ) -> Result<Self> {
        let root = published.root();
        let path = published.path();
        let bundle_file = path
            .file_name()
            .and_then(|name| name.to_str())
            .context("provider evidence bundle file name is not UTF-8")?
            .to_string();
        Ok(Self {
            identity: ProviderEvidenceIdentity {
                base_oid: evidence.base_oid().to_string(),
                head_oid: evidence.head_oid().to_string(),
                diff_digest: evidence.diff_digest().clone(),
                bundle_digest: published.digest().clone(),
                bundle_file,
            },
            root,
            path,
        })
    }

    #[cfg(test)]
    pub(crate) fn synthetic(base_oid: &str, head_oid: &str, diff_digest: &Sha256Digest) -> Self {
        let bundle_digest = Sha256Digest::compute(b"synthetic provider evidence");
        let root = PathBuf::from("/immutable-provider-evidence").join(bundle_digest.as_str());
        let path = root.join("provider-evidence.tar");
        Self {
            identity: ProviderEvidenceIdentity {
                base_oid: base_oid.to_string(),
                head_oid: head_oid.to_string(),
                diff_digest: diff_digest.clone(),
                bundle_digest,
                bundle_file: "provider-evidence.tar".to_string(),
            },
            root,
            path,
        }
    }

    pub(crate) fn matches_tuple(
        &self,
        base_oid: &str,
        head_oid: &str,
        diff_digest: &Sha256Digest,
    ) -> bool {
        self.identity.base_oid == base_oid
            && self.identity.head_oid == head_oid
            && &self.identity.diff_digest == diff_digest
    }
}

#[derive(Debug)]
pub(super) struct ExactOidEvidence {
    base_oid: GitObjectId,
    head_oid: GitObjectId,
    diff_digest: Sha256Digest,
    bundle_bytes: Vec<u8>,
}

impl ExactOidEvidence {
    pub(super) fn base_oid(&self) -> &str {
        self.base_oid.as_str()
    }

    pub(super) fn head_oid(&self) -> &str {
        self.head_oid.as_str()
    }

    pub(super) fn diff_digest(&self) -> &Sha256Digest {
        &self.diff_digest
    }

    #[cfg(test)]
    pub(super) fn bundle_bytes(&self) -> &[u8] {
        &self.bundle_bytes
    }

    pub(super) fn publish(
        &self,
        store: &csa_session::convergence::ConvergenceLedgerStore,
    ) -> Result<(ProviderEvidenceRef, ProviderEvidenceBundle)> {
        let published = store.publish_provider_evidence_bundle(&self.bundle_bytes)?;
        let evidence_ref = ProviderEvidenceRef::from_published(self, &published)?;
        Ok((evidence_ref, published))
    }
}

#[derive(Serialize)]
struct EvidenceManifest<'a> {
    schema_version: u32,
    kind: &'static str,
    base_oid: &'a str,
    head_oid: &'a str,
    diff_sha256: String,
    diff_size_bytes: usize,
    source_archive_sha256: String,
    source_archive_size_bytes: usize,
}

pub(super) fn build_exact_oid_evidence(
    project_root: &Path,
    range: &str,
) -> Result<ExactOidEvidence> {
    let base = range
        .strip_suffix("...HEAD")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("convergence range must be <base>...HEAD"))?;
    let head_oid = git_text(project_root, &["rev-parse", "--verify", "HEAD^{commit}"])
        .context("resolve exact convergence HEAD OID")?;
    build_exact_oid_evidence_for_head(project_root, base, &head_oid)
}

pub(super) fn build_exact_oid_evidence_for_head(
    project_root: &Path,
    base: &str,
    head_oid: &str,
) -> Result<ExactOidEvidence> {
    let head_oid = GitObjectId::parse(head_oid).context("validate exact convergence HEAD OID")?;
    let base_oid = git_text(project_root, &["merge-base", base, head_oid.as_str()])
        .context("resolve exact convergence merge-base OID")?;
    let base_oid = GitObjectId::parse(&base_oid).context("validate convergence merge-base OID")?;
    let diff = git_bytes(
        project_root,
        &[
            "diff",
            "--binary",
            "--full-index",
            "--no-ext-diff",
            base_oid.as_str(),
            head_oid.as_str(),
            "--",
        ],
    )
    .context("capture exact-OID convergence diff")?;
    let source_archive = git_bytes(
        project_root,
        &["archive", "--format=tar", head_oid.as_str()],
    )
    .context("capture exact-OID convergence source archive")?;
    let diff_digest = Sha256Digest::compute(&diff);
    let source_archive_digest = Sha256Digest::compute(&source_archive);
    let manifest = EvidenceManifest {
        schema_version: BUNDLE_SCHEMA_VERSION,
        kind: BUNDLE_KIND,
        base_oid: base_oid.as_str(),
        head_oid: head_oid.as_str(),
        diff_sha256: diff_digest.to_string(),
        diff_size_bytes: diff.len(),
        source_archive_sha256: source_archive_digest.to_string(),
        source_archive_size_bytes: source_archive.len(),
    };
    let manifest_bytes = serde_json::to_vec(&manifest).context("serialize evidence manifest")?;
    let bundle_bytes = build_tar_bundle(&manifest_bytes, &diff, &source_archive)?;
    Ok(ExactOidEvidence {
        base_oid,
        head_oid,
        diff_digest,
        bundle_bytes,
    })
}

fn build_tar_bundle(manifest: &[u8], diff: &[u8], source_archive: &[u8]) -> Result<Vec<u8>> {
    let mut builder = tar::Builder::new(Vec::new());
    append_tar_entry(&mut builder, "manifest.json", manifest)?;
    append_tar_entry(&mut builder, "diff.patch", diff)?;
    append_tar_entry(&mut builder, "source.tar", source_archive)?;
    builder.finish().context("finish provider evidence tar")?;
    builder
        .into_inner()
        .context("finalize provider evidence tar")
}

fn append_tar_entry(
    builder: &mut tar::Builder<Vec<u8>>,
    path: &str,
    contents: &[u8],
) -> Result<()> {
    let size = u64::try_from(contents.len()).context("evidence tar entry is too large")?;
    let mut header = tar::Header::new_ustar();
    header.set_size(size);
    header.set_mode(0o400);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_cksum();
    builder
        .append_data(&mut header, path, Cursor::new(contents))
        .with_context(|| format!("append {path} to provider evidence tar"))
}

fn git_text(project_root: &Path, args: &[&str]) -> Result<String> {
    let output = git_output(project_root, args)?;
    let value = String::from_utf8(output.stdout).context("git output was not UTF-8")?;
    let value = value.trim();
    if value.is_empty() {
        bail!("git {} returned empty output", args.join(" "));
    }
    Ok(value.to_string())
}

fn git_bytes(project_root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    Ok(git_output(project_root, args)?.stdout)
}

fn git_output(project_root: &Path, args: &[&str]) -> Result<Output> {
    let output = Command::new("git")
        .args(args)
        .current_dir(project_root)
        .output()
        .with_context(|| format!("run git {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;

    fn git(repo: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .expect("run git command");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("git stdout should be utf-8")
            .trim()
            .to_string()
    }

    fn commit_file(repo: &Path, path: &str, contents: &str, message: &str) -> String {
        std::fs::write(repo.join(path), contents).expect("write fixture file");
        git(repo, &["add", path]);
        git(repo, &["commit", "-m", message]);
        git(repo, &["rev-parse", "HEAD"])
    }

    fn tar_entry(bundle: &[u8], name: &str) -> Vec<u8> {
        let mut archive = tar::Archive::new(Cursor::new(bundle));
        for entry in archive.entries().expect("read evidence bundle entries") {
            let mut entry = entry.expect("read evidence bundle entry");
            if entry.path().expect("read tar path") == Path::new(name) {
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).expect("read tar entry");
                return bytes;
            }
        }
        panic!("missing evidence bundle entry {name}");
    }

    fn initialized_repo() -> tempfile::TempDir {
        let repo = tempfile::tempdir().expect("temp git repository");
        git(repo.path(), &["init", "-b", "main"]);
        git(repo.path(), &["config", "user.name", "CSA Test"]);
        git(
            repo.path(),
            &["config", "user.email", "csa@example.invalid"],
        );
        repo
    }

    #[test]
    fn exact_oid_bundle_uses_one_fixed_head_for_merge_base_diff_and_archive() {
        let repo = initialized_repo();
        let base = commit_file(repo.path(), "tracked.txt", "base\n", "base");
        let fixed_head = commit_file(repo.path(), "tracked.txt", "fixed\n", "fixed head");
        let _new_head = commit_file(repo.path(), "tracked.txt", "new\n", "move head");

        let evidence = build_exact_oid_evidence_for_head(repo.path(), &base, &fixed_head)
            .expect("build exact-OID evidence");
        let expected_diff = Command::new("git")
            .args(["diff", "--binary", "--full-index", &base, &fixed_head, "--"])
            .current_dir(repo.path())
            .output()
            .expect("read fixed diff");

        assert_eq!(evidence.base_oid(), base);
        assert_eq!(evidence.head_oid(), fixed_head);
        assert_eq!(
            tar_entry(evidence.bundle_bytes(), "diff.patch"),
            expected_diff.stdout
        );
        let source_archive = tar_entry(evidence.bundle_bytes(), "source.tar");
        assert_eq!(tar_entry(&source_archive, "tracked.txt"), b"fixed\n");
    }

    #[test]
    fn source_modify_read_revert_cannot_change_provider_visible_bundle_bytes() {
        let repo = initialized_repo();
        let base = commit_file(repo.path(), "tracked.txt", "base\n", "base");
        let head = commit_file(repo.path(), "tracked.txt", "committed\n", "head");
        let evidence = build_exact_oid_evidence_for_head(repo.path(), &base, &head)
            .expect("build exact-OID evidence");
        let before_digest = Sha256Digest::compute(evidence.bundle_bytes());

        std::fs::write(repo.path().join("tracked.txt"), "mutable\n")
            .expect("mutate source checkout");
        let source_archive = tar_entry(evidence.bundle_bytes(), "source.tar");
        let provider_visible = tar_entry(&source_archive, "tracked.txt");
        std::fs::write(repo.path().join("tracked.txt"), "committed\n")
            .expect("revert source checkout");

        assert_eq!(provider_visible, b"committed\n");
        assert_eq!(
            Sha256Digest::compute(evidence.bundle_bytes()),
            before_digest
        );
    }
}
