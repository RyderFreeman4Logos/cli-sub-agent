use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use csa_config::GcConfig;
use tracing::{info, warn};

const TRANSCRIPT_REL_PATH: &str = "output/acp-events.jsonl";
const BYTES_PER_MEGABYTE: u64 = 1024 * 1024;

#[derive(Debug, Clone)]
struct TranscriptFile {
    path: PathBuf,
    size_bytes: u64,
    modified: SystemTime,
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct TranscriptCleanupStats {
    pub(crate) files_removed: u64,
    pub(crate) bytes_reclaimed: u64,
}

pub(crate) fn load_gc_config_for_sessions(
    session_root: &Path,
    sessions: &[csa_session::MetaSessionState],
) -> GcConfig {
    let Some(project_root) = sessions.first().map(|s| PathBuf::from(&s.project_path)) else {
        return GcConfig::default();
    };

    match GcConfig::load_for_project(&project_root) {
        Ok(cfg) => cfg,
        Err(error) => {
            warn!(
                root = %session_root.display(),
                project_root = %project_root.display(),
                error = %error,
                "Failed to load [gc] config for project root; falling back to defaults"
            );
            GcConfig::default()
        }
    }
}

pub(crate) fn cleanup_project_transcripts(
    session_root: &Path,
    gc_config: GcConfig,
    dry_run: bool,
) -> TranscriptCleanupStats {
    let canonical_session_root = match session_root.canonicalize() {
        Ok(path) => path,
        Err(error) => {
            warn!(
                root = %session_root.display(),
                error = %error,
                "Skipping transcript GC because session root cannot be canonicalized"
            );
            return TranscriptCleanupStats::default();
        }
    };

    let sessions_dir = session_root.join("sessions");
    let files = collect_transcript_files(&sessions_dir);
    let max_size_bytes = gc_config
        .transcript_max_size_mb
        .saturating_mul(BYTES_PER_MEGABYTE);
    let candidates = plan_transcript_cleanup(
        files,
        SystemTime::now(),
        gc_config.transcript_max_age_days,
        max_size_bytes,
    );

    let mut stats = TranscriptCleanupStats::default();
    let mut cumulative = 0u64;
    for file in candidates {
        let canonical_path = match canonical_path_within_root(&file.path, &canonical_session_root) {
            Some(path) => path,
            None => {
                warn!(
                    path = %file.path.display(),
                    root = %canonical_session_root.display(),
                    "Skipping transcript cleanup outside session root boundary"
                );
                continue;
            }
        };

        cumulative = cumulative.saturating_add(file.size_bytes);
        if dry_run {
            eprintln!(
                "[dry-run] Would remove transcript: {} ({} bytes, cumulative {} bytes)",
                canonical_path.display(),
                file.size_bytes,
                cumulative
            );
            stats.files_removed = stats.files_removed.saturating_add(1);
            stats.bytes_reclaimed = stats.bytes_reclaimed.saturating_add(file.size_bytes);
            continue;
        }

        match fs::remove_file(&canonical_path) {
            Ok(()) => {
                info!(
                    path = %canonical_path.display(),
                    size_bytes = file.size_bytes,
                    "Removed transcript file during GC"
                );
                stats.files_removed = stats.files_removed.saturating_add(1);
                stats.bytes_reclaimed = stats.bytes_reclaimed.saturating_add(file.size_bytes);
            }
            Err(error) => {
                warn!(
                    path = %canonical_path.display(),
                    error = %error,
                    "Failed to remove transcript file during GC"
                );
            }
        }
    }
    stats
}

fn collect_transcript_files(sessions_dir: &Path) -> Vec<TranscriptFile> {
    let mut files = Vec::new();
    let entries = match fs::read_dir(sessions_dir) {
        Ok(entries) => entries,
        Err(_) => return files,
    };

    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|ft| ft.is_dir()) {
            continue;
        }
        let transcript_path = entry.path().join(TRANSCRIPT_REL_PATH);
        if !transcript_path.is_file() {
            continue;
        }
        let Ok(metadata) = fs::metadata(&transcript_path) else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        files.push(TranscriptFile {
            path: transcript_path,
            size_bytes: metadata.len(),
            modified,
        });
    }
    files
}

fn plan_transcript_cleanup(
    mut files: Vec<TranscriptFile>,
    now: SystemTime,
    max_age_days: u64,
    max_size_bytes: u64,
) -> Vec<TranscriptFile> {
    files.sort_by_key(|f| f.modified);
    let mut removals = Vec::new();
    let mut survivors = Vec::new();

    for file in files {
        if is_transcript_expired(now, file.modified, max_age_days) {
            removals.push(file);
        } else {
            survivors.push(file);
        }
    }

    let mut survivor_total_bytes = survivors
        .iter()
        .fold(0u64, |acc, file| acc.saturating_add(file.size_bytes));
    for file in survivors {
        if survivor_total_bytes <= max_size_bytes {
            break;
        }
        survivor_total_bytes = survivor_total_bytes.saturating_sub(file.size_bytes);
        removals.push(file);
    }

    removals
}

fn is_transcript_expired(now: SystemTime, modified: SystemTime, max_age_days: u64) -> bool {
    let max_age = Duration::from_secs(max_age_days.saturating_mul(24 * 60 * 60));
    now.duration_since(modified).is_ok_and(|age| age > max_age)
}

fn canonical_path_within_root(path: &Path, root: &Path) -> Option<PathBuf> {
    let canonical = path.canonicalize().ok()?;
    canonical.starts_with(root).then_some(canonical)
}

#[cfg(test)]
mod tests {
    use super::{TranscriptFile, canonical_path_within_root, plan_transcript_cleanup};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};
    use tempfile::tempdir;

    #[test]
    fn test_plan_transcript_cleanup_removes_files_older_than_age_limit() {
        let now = SystemTime::now();
        let files = vec![
            TranscriptFile {
                path: PathBuf::from("/tmp/old/output/acp-events.jsonl"),
                size_bytes: 256,
                modified: now - Duration::from_secs(40 * 24 * 60 * 60),
            },
            TranscriptFile {
                path: PathBuf::from("/tmp/new/output/acp-events.jsonl"),
                size_bytes: 256,
                modified: now - Duration::from_secs(2 * 24 * 60 * 60),
            },
        ];

        let removals = plan_transcript_cleanup(files, now, 30, u64::MAX);
        assert_eq!(removals.len(), 1);
        assert_eq!(
            removals[0].path,
            PathBuf::from("/tmp/old/output/acp-events.jsonl")
        );
    }

    #[test]
    fn test_canonical_path_within_root_accepts_internal_path() {
        let root = tempdir().unwrap();
        let transcript = root.path().join("sessions/s1/output/acp-events.jsonl");
        fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        fs::write(&transcript, "{}\n").unwrap();

        let canonical_root = root.path().canonicalize().unwrap();
        let resolved = canonical_path_within_root(&transcript, &canonical_root);
        assert!(resolved.is_some());
    }

    #[cfg(unix)]
    #[test]
    fn test_canonical_path_within_root_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;

        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let outside_output = outside.path().join("output");
        fs::create_dir_all(&outside_output).unwrap();
        fs::write(outside_output.join("acp-events.jsonl"), "{}\n").unwrap();

        let session_dir = root.path().join("sessions/01TESTSESSION00000000000000");
        fs::create_dir_all(&session_dir).unwrap();
        symlink(&outside_output, session_dir.join("output")).unwrap();

        let escaped = session_dir.join("output/acp-events.jsonl");
        let canonical_root = root.path().canonicalize().unwrap();
        let resolved = canonical_path_within_root(&escaped, &canonical_root);
        assert!(resolved.is_none(), "symlink escape must be rejected");
    }
}
