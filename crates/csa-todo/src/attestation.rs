//! TODO attestation write operations.
//!
//! Records the content hash that pins a plan's `TODO.md` to an audited
//! snapshot. Lives in its own module, parallel to [`crate::generated_plan`], so
//! the lock-held publish seam ([`TodoManager::ensure_attestation_with`]) stays
//! beside its companion documentation without pushing `lib.rs` over the
//! per-file token budget.

use anyhow::Result;

use crate::{TodoAttestation, TodoManager, hash_todo_content, read_todo_content};

impl TodoManager {
    /// Store an attestation when one is missing or no longer matches TODO.md.
    ///
    /// Convenience wrapper over
    /// [`ensure_attestation_with`](Self::ensure_attestation_with) for callers
    /// that only need the attestation and have no publish step to run under the
    /// same lock.
    pub fn ensure_attestation(&self, timestamp: &str) -> Result<TodoAttestation> {
        let (attestation, ()) = self.ensure_attestation_with(timestamp, |_| Ok(()))?;
        Ok(attestation)
    }

    /// Store an attestation when one is missing or no longer matches TODO.md,
    /// then run a caller `publish` step under the SAME held write lock.
    ///
    /// The attestation phase is identical to
    /// [`ensure_attestation`](Self::ensure_attestation), but `publish` is invoked
    /// *before the write lock is released*, right after the attestation is written
    /// (or confirmed already current). This makes the attestation write and the
    /// caller's publish step (e.g. the `csa todo save` git commit + the
    /// hook-trigger decision) one critical section: a concurrent TODO writer
    /// cannot interleave between the attestation and the commit and cause this
    /// call to publish the wrong snapshot (TOCTOU lost-update / corrupted audit
    /// history; rust rule 017). The write lock is released only after `publish`
    /// returns, on both the success and error paths (the lock guard in the
    /// write-lock helper drops on scope exit regardless of outcome).
    ///
    /// `publish` receives the stored attestation and returns any caller-defined
    /// value (e.g. the commit hash). It MUST NOT itself attempt to acquire the
    /// TODO write lock (e.g. by spawning `csa todo save`/`persist`), or it will
    /// deadlock against the held lock. Run any such side effect (e.g. firing the
    /// `TodoSave` hook) from the returned value, after this method returns.
    pub fn ensure_attestation_with<T>(
        &self,
        timestamp: &str,
        publish: impl FnOnce(&TodoAttestation) -> Result<T>,
    ) -> Result<(TodoAttestation, T)> {
        self.with_write_lock(|| {
            let plan = self.load_inner(timestamp)?;
            let content = read_todo_content(&plan)?;
            let actual_hash = hash_todo_content(&content);

            let attestation = if let Some(attestation) = self.read_attestation(&plan)?
                && attestation.hash == actual_hash
            {
                attestation
            } else {
                self.write_attestation(&plan, actual_hash)?
            };

            // Run the caller's publish step (commit + hook-trigger decision)
            // BEFORE releasing the write lock, so the commit captures exactly the
            // attested content and no concurrent writer can interleave.
            let published = publish(&attestation)?;
            Ok((attestation, published))
        })
    }
}
