//! Source-repository identity checks and cross-process ownership for repair execution.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{Context, Result, bail};
use csa_session::convergence::{CompletionActionClaim, EpochRecord, GitObjectId, Sha256Digest};

const SOURCE_REPAIR_OWNER_FILE: &str = "csa-convergence-repair.owner";

/// A source epoch captured from structured Git argv execution.
pub(super) struct CapturedEpoch {
    pub(super) epoch: EpochRecord,
    pub(super) clean: bool,
}

/// Cross-process source-repository owner held for the complete repair writer lifetime.
pub(super) struct SourceRepairOwner {
    file: File,
    path: PathBuf,
    released: bool,
}

impl SourceRepairOwner {
    pub(super) fn acquire(project_root: &Path, expected_epoch: &EpochRecord) -> Result<Self> {
        let git_directory = source_git_directory(project_root)?;
        let path = git_directory.join(SOURCE_REPAIR_OWNER_FILE);
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&path)
            .with_context(|| format!("open source repair owner {}", path.display()))?;
        // SAFETY: `file` remains owned by this guard until explicit release or Drop, so its valid
        // file descriptor keeps the advisory lock alive for the complete repair operation.
        let lock_result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if lock_result != 0 {
            return Err(std::io::Error::last_os_error()).with_context(|| {
                format!(
                    "source repository already has a concurrent fenced repair owner at {}",
                    path.display()
                )
            });
        }
        let record = serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "expected_epoch_id": expected_epoch.id(),
            "expected_commit": expected_epoch.head_oid(),
        }))?;
        file.set_len(0)
            .context("clear source repair owner record before fencing")?;
        file.write_all(&record)
            .context("write source repair owner record")?;
        file.sync_all().context("sync source repair owner record")?;
        Ok(Self {
            file,
            path,
            released: false,
        })
    }

    pub(super) fn bind_claim(&mut self, claim: &CompletionActionClaim) -> Result<()> {
        let record = serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "campaign_id": claim.campaign_id(),
            "epoch_id": claim.epoch_id(),
            "generation": claim.generation(),
            "action_id": claim.action_id(),
            "policy_digest": claim.policy_digest(),
        }))?;
        self.file
            .set_len(0)
            .context("clear source repair owner record before claim fencing")?;
        self.file
            .write_all(&record)
            .context("write source repair claim fence")?;
        self.file
            .sync_all()
            .context("sync source repair claim fence")
    }

    pub(super) fn release(mut self) -> Result<()> {
        self.unlock()?;
        self.released = true;
        Ok(())
    }

    fn unlock(&self) -> Result<()> {
        // SAFETY: this guard owns `file`; unlocking its still-valid descriptor cannot invalidate
        // aliases and is the paired operation for the successful `flock` acquisition above.
        if unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) } != 0 {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("release source repair owner {}", self.path.display()));
        }
        Ok(())
    }
}

impl Drop for SourceRepairOwner {
    fn drop(&mut self) {
        if !self.released {
            let _ = self.unlock();
        }
    }
}

/// Capture a clean, immutable source epoch without constructing a shell command.
pub(super) fn capture_epoch(project_root: &Path, base_oid: &GitObjectId) -> Result<CapturedEpoch> {
    let head = git(project_root, &["rev-parse", "--verify", "HEAD^{commit}"])?;
    let head_oid = String::from_utf8(head.stdout)
        .context("repair HEAD was not UTF-8")?
        .trim()
        .to_owned();
    let diff = git(
        project_root,
        &[
            "diff",
            "--binary",
            "--full-index",
            "--no-ext-diff",
            base_oid.as_str(),
            &head_oid,
            "--",
        ],
    )?;
    let status = git(
        project_root,
        &["status", "--porcelain=v1", "--untracked-files=normal"],
    )?;
    Ok(CapturedEpoch {
        epoch: EpochRecord::new(
            base_oid.clone(),
            GitObjectId::parse(&head_oid)?,
            Sha256Digest::compute(&diff.stdout),
        ),
        clean: status.stdout.is_empty(),
    })
}

fn source_git_directory(project_root: &Path) -> Result<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rev-parse", "--absolute-git-dir"])
        .output()
        .context("resolve source repository Git directory")?;
    if !output.status.success() {
        bail!(
            "resolve source repository Git directory failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let raw = String::from_utf8(output.stdout)
        .context("source repository Git directory was not UTF-8")?;
    let canonical = std::fs::canonicalize(raw.trim())
        .context("canonicalize source repository Git directory")?;
    if !canonical.is_dir() {
        bail!(
            "source repository Git directory is not a directory: {}",
            canonical.display()
        );
    }
    Ok(canonical)
}

fn git(project_root: &Path, args: &[&str]) -> Result<Output> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(args)
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
