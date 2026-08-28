use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::{ResolvedReviewContext, ResolvedReviewContextKind, digest_review_context};

const DAEMON_REVIEW_CONTEXT_SNAPSHOT_MAX_BYTES: usize = super::REVIEW_CONTEXT_MAX_BYTES * 6
    + r#"{"digest":""#.len()
    + "sha256:".len()
    + 64
    + r#"","kind":"TodoMarkdown","snapshot":""#.len()
    + r#""}"#.len();
const DAEMON_REVIEW_CONTEXT_SNAPSHOT_FILE: &str = "review-context.json";

#[derive(Serialize, Deserialize)]
struct DaemonReviewContextSnapshot {
    digest: String,
    kind: DaemonReviewContextKind,
    snapshot: String,
}

#[derive(Serialize, Deserialize)]
enum DaemonReviewContextKind {
    TodoMarkdown,
    Passthrough,
    SpecToml,
}

impl ResolvedReviewContext {
    pub(crate) fn persist_daemon_snapshot(&self, session_dir: &Path) -> Result<()> {
        let kind = match &self.kind {
            ResolvedReviewContextKind::TodoMarkdown => DaemonReviewContextKind::TodoMarkdown,
            ResolvedReviewContextKind::Passthrough => DaemonReviewContextKind::Passthrough,
            ResolvedReviewContextKind::SpecToml { .. } => DaemonReviewContextKind::SpecToml,
        };
        let encoded = serde_json::to_vec(&DaemonReviewContextSnapshot {
            digest: self.digest.clone(),
            kind,
            snapshot: self.snapshot.clone(),
        })
        .context("failed to encode admitted daemon review context")?;
        let input_dir = session_dir.join("input");
        std::fs::create_dir_all(&input_dir)
            .context("failed to create admitted daemon review context input directory")?;
        std::fs::write(input_dir.join(DAEMON_REVIEW_CONTEXT_SNAPSHOT_FILE), encoded)
            .context("failed to persist admitted daemon review context")
    }

    pub(crate) fn load_daemon_snapshot(session_dir: &Path) -> Result<Option<Self>> {
        let path = session_dir
            .join("input")
            .join(DAEMON_REVIEW_CONTEXT_SNAPSHOT_FILE);
        let file = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).context("failed to read admitted daemon review context");
            }
        };
        let mut encoded = Vec::new();
        file.take(DAEMON_REVIEW_CONTEXT_SNAPSHOT_MAX_BYTES as u64 + 1)
            .read_to_end(&mut encoded)
            .context("failed to read admitted daemon review context")?;
        anyhow::ensure!(
            encoded.len() <= DAEMON_REVIEW_CONTEXT_SNAPSHOT_MAX_BYTES,
            "admitted daemon review context exceeds the allowed size"
        );
        let snapshot: DaemonReviewContextSnapshot = serde_json::from_slice(&encoded)
            .context("failed to decode admitted daemon review context")?;
        let expected_digest = std::env::var(super::DAEMON_REVIEW_CONTEXT_DIGEST_ENV_KEY)
            .context("missing admitted daemon review context launch digest")?;
        anyhow::ensure!(
            expected_digest == snapshot.digest,
            "admitted daemon review context digest mismatch"
        );
        anyhow::ensure!(
            digest_review_context(&snapshot.snapshot) == snapshot.digest,
            "admitted daemon review context digest mismatch"
        );
        let kind = match snapshot.kind {
            DaemonReviewContextKind::TodoMarkdown => ResolvedReviewContextKind::TodoMarkdown,
            DaemonReviewContextKind::Passthrough => ResolvedReviewContextKind::Passthrough,
            DaemonReviewContextKind::SpecToml => ResolvedReviewContextKind::SpecToml {
                spec: toml::from_str(&snapshot.snapshot)
                    .context("failed to parse admitted daemon review spec context")?,
            },
        };
        Ok(Some(Self {
            path: String::new(),
            digest: snapshot.digest,
            kind,
            snapshot: snapshot.snapshot,
        }))
    }
}
