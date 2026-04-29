use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use csa_memory::{MemoryEntry, MemoryStore};
use serde_json::json;

const WING: &str = "cli-sub-agent";
const ROOM: &str = "csa-migration";
const SOURCE_PREFIX: &str = "csa-memory-";
const MEMORY_FILE_NAME: &str = "memories.jsonl";

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MigrationStats {
    pub migrated: usize,
    pub skipped: usize,
    pub failed: usize,
}

pub fn migrate_to_mempal(store: MemoryStore, dry_run: bool, cd: Option<String>) -> Result<()> {
    let total_legacy_lines = count_legacy_memory_lines(&store)?;
    let entries = store.load_all()?;
    if entries.is_empty() {
        println!(
            "Memory migration complete: migrated=0 skipped={} failed=0",
            total_legacy_lines
        );
        return Ok(());
    }

    let working_dir = resolve_working_dir(cd)?;
    let mempal_binary = if dry_run {
        None
    } else {
        Some(resolve_mempal_binary()?)
    };

    let stats = migrate_entries(
        entries,
        mempal_binary.as_deref(),
        dry_run,
        working_dir.as_deref(),
        total_legacy_lines,
    )?;
    println!(
        "Memory migration complete: migrated={} skipped={} failed={}",
        stats.migrated, stats.skipped, stats.failed
    );
    Ok(())
}

fn migrate_entries(
    entries: Vec<MemoryEntry>,
    mempal_binary: Option<&Path>,
    dry_run: bool,
    working_dir: Option<&Path>,
    total_legacy_lines: usize,
) -> Result<MigrationStats> {
    let mut stats = MigrationStats {
        skipped: total_legacy_lines.saturating_sub(entries.len()),
        ..MigrationStats::default()
    };

    for entry in entries {
        if entry.content.trim().is_empty() {
            stats.skipped += 1;
            eprintln!("Warning: skipped empty memory entry {}", entry.id);
            continue;
        }

        let payload = build_mempal_payload(&entry);
        if dry_run {
            stats.migrated += 1;
            println!("{}", serde_json::to_string(&payload)?);
            continue;
        }

        let binary_path = mempal_binary.context("mempal binary path missing")?;
        match run_mempal_ingest(binary_path, &payload, working_dir) {
            Ok(()) => stats.migrated += 1,
            Err(error) => {
                stats.failed += 1;
                eprintln!(
                    "Warning: failed to migrate memory entry {}: {error}",
                    entry.id
                );
            }
        }
    }

    Ok(stats)
}

fn count_legacy_memory_lines(store: &MemoryStore) -> Result<usize> {
    let file_path = store.base_dir().join(MEMORY_FILE_NAME);
    if !file_path.exists() {
        return Ok(0);
    }

    let file = OpenOptions::new()
        .read(true)
        .open(&file_path)
        .with_context(|| format!("failed to read memory file: {}", file_path.display()))?;
    let reader = BufReader::new(file);
    let mut count = 0usize;
    for line in reader.lines() {
        if !line
            .with_context(|| format!("failed to read memory file: {}", file_path.display()))?
            .trim()
            .is_empty()
        {
            count += 1;
        }
    }
    Ok(count)
}

fn build_mempal_payload(entry: &MemoryEntry) -> serde_json::Value {
    json!({
        "content": entry.content,
        "wing": WING,
        "room": ROOM,
        "project": entry.project,
        "source": format!("{SOURCE_PREFIX}{}", entry.id),
        "metadata": {
            "tool": entry.tool,
            "session_id": entry.session_id,
        },
    })
}

fn resolve_mempal_binary() -> Result<PathBuf> {
    let info = csa_memory::detect_mempal().ok_or_else(|| {
        anyhow::anyhow!(
            "mempal is not installed or not on PATH. Install mempal, then rerun `csa memory migrate --to mempal`."
        )
    })?;
    Ok(PathBuf::from(&info.binary_path))
}

fn resolve_working_dir(cd: Option<String>) -> Result<Option<PathBuf>> {
    let Some(raw) = cd else {
        return Ok(None);
    };
    let path = PathBuf::from(raw);
    if !path.is_dir() {
        bail!(
            "--cd must point to an existing directory: {}",
            path.display()
        );
    }
    Ok(Some(path))
}

fn run_mempal_ingest(
    binary_path: &Path,
    payload: &serde_json::Value,
    working_dir: Option<&Path>,
) -> Result<()> {
    let mut command = Command::new(binary_path);
    command
        .arg("ingest")
        .arg("--stdin")
        .arg("--json")
        .arg("--no-gate")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    if let Some(dir) = working_dir {
        command.current_dir(dir);
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    let mut child = command.spawn().with_context(|| {
        format!(
            "failed to spawn mempal ingest using {}",
            binary_path.display()
        )
    })?;
    if let Some(mut stdin) = child.stdin.take() {
        serde_json::to_writer(&mut stdin, payload).context("failed to write mempal payload")?;
        stdin
            .write_all(b"\n")
            .context("failed to terminate mempal payload")?;
    }

    let output = child
        .wait_with_output()
        .context("failed to wait for mempal ingest")?;
    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!(
        "mempal ingest exited with code {}: {}",
        output.status.code().unwrap_or(-1),
        stderr.trim()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use csa_memory::MemorySource;
    use ulid::Ulid;

    #[test]
    fn build_mempal_payload_preserves_idempotency_source_and_metadata() {
        let id = Ulid::new();
        let entry = MemoryEntry {
            id,
            timestamp: Utc::now(),
            project: Some("project-key".to_string()),
            tool: Some("codex".to_string()),
            session_id: Some("01SESSION".to_string()),
            tags: vec!["one".to_string()],
            content: "memory content".to_string(),
            facts: Vec::new(),
            source: MemorySource::PostRun,
            valid_from: None,
            valid_until: None,
        };

        let payload = build_mempal_payload(&entry);

        assert_eq!(payload["content"], "memory content");
        assert_eq!(payload["wing"], WING);
        assert_eq!(payload["room"], ROOM);
        assert_eq!(payload["project"], "project-key");
        assert_eq!(payload["source"], format!("{SOURCE_PREFIX}{id}"));
        assert_eq!(payload["metadata"]["tool"], "codex");
        assert_eq!(payload["metadata"]["session_id"], "01SESSION");
    }
}
