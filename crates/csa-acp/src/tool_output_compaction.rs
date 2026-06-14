//! Sidecar large ACP tool outputs before they are retained as session events.
//!
//! The manifest intentionally extends the existing `tool_outputs/manifest.toml`
//! `[[entries]]` shape with optional ACP-specific fields. Older readers that
//! only know `index`, `original_bytes`, and `path` can still parse the file.

use std::fs;
use std::path::{Path, PathBuf};

use csa_core::redact::redact_text_content;
use serde::{Deserialize, Serialize};
use tracing::warn;

const ACP_TOOL_OUTPUT_INDEX_BASE: u32 = 1_000_000;
const DEFAULT_HEAD_BYTES: usize = 4 * 1024;
const DEFAULT_TAIL_BYTES: usize = 4 * 1024;
const COMPACTED_MARKER: &str = "[tool:output:compacted]";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutputCompactionConfig {
    sidecar_dir: PathBuf,
    threshold_bytes: u64,
    head_bytes: usize,
    tail_bytes: usize,
}

impl ToolOutputCompactionConfig {
    #[must_use]
    pub fn new(sidecar_dir: PathBuf, threshold_bytes: u64) -> Self {
        Self {
            sidecar_dir,
            threshold_bytes,
            head_bytes: DEFAULT_HEAD_BYTES,
            tail_bytes: DEFAULT_TAIL_BYTES,
        }
    }

    #[must_use]
    pub fn into_state(self) -> ToolOutputCompactionState {
        ToolOutputCompactionState {
            config: self,
            next_index: ACP_TOOL_OUTPUT_INDEX_BASE,
        }
    }
}

#[derive(Debug)]
pub struct ToolOutputCompactionState {
    config: ToolOutputCompactionConfig,
    next_index: u32,
}

impl ToolOutputCompactionState {
    #[must_use]
    pub fn render_tool_output(
        &mut self,
        tool_call_id: &str,
        title: Option<&str>,
        status: &str,
        output: &str,
    ) -> String {
        let redacted_output = redact_text_content(output);
        let redacted_title = title.map(redact_text_content);
        let redacted_status = redact_text_content(status);
        if output.len() as u64 <= self.config.threshold_bytes
            || redacted_output.starts_with(COMPACTED_MARKER)
        {
            return render_full_tool_output(tool_call_id, &redacted_status, &redacted_output);
        }

        match self.write_sidecar(
            tool_call_id,
            redacted_title.as_deref(),
            &redacted_status,
            &redacted_output,
        ) {
            Ok(record) => render_compacted_summary(CompactedSummary {
                tool_call_id,
                title: redacted_title.as_deref(),
                status: &redacted_status,
                output: &redacted_output,
                record: &record,
                head_bytes: self.config.head_bytes,
                tail_bytes: self.config.tail_bytes,
                threshold_bytes: self.config.threshold_bytes,
            }),
            Err(error) => {
                warn!(
                    tool_call_id,
                    error = %error,
                    "failed to sidecar large ACP tool output; preserving raw output inline"
                );
                render_full_tool_output(tool_call_id, &redacted_status, &redacted_output)
            }
        }
    }

    fn write_sidecar(
        &mut self,
        tool_call_id: &str,
        title: Option<&str>,
        status: &str,
        output: &str,
    ) -> std::io::Result<SidecarRecord> {
        fs::create_dir_all(&self.config.sidecar_dir)?;
        let index = self.next_available_index();
        let filename = format!("{index}.raw");
        let path = self.config.sidecar_dir.join(&filename);
        fs::write(&path, output.as_bytes())?;
        let relative_path = relative_sidecar_path(&self.config.sidecar_dir, &filename);
        let record = SidecarRecord {
            index,
            original_bytes: output.len() as u64,
            original_lines: line_count(output) as u64,
            path: relative_path,
            compacted: true,
            tool_call_id: tool_call_id.to_string(),
            tool_title: title.map(str::to_string),
            status: status.to_string(),
            threshold_bytes: self.config.threshold_bytes,
        };
        append_manifest(&self.config.sidecar_dir, &record)?;
        self.next_index = index.saturating_add(1);
        Ok(record)
    }

    fn next_available_index(&mut self) -> u32 {
        loop {
            let index = self.next_index;
            let path = self.config.sidecar_dir.join(format!("{index}.raw"));
            if !path.exists() {
                return index;
            }
            self.next_index = self.next_index.saturating_add(1);
        }
    }
}

#[derive(Debug, Clone)]
struct SidecarRecord {
    index: u32,
    original_bytes: u64,
    original_lines: u64,
    path: String,
    compacted: bool,
    tool_call_id: String,
    tool_title: Option<String>,
    status: String,
    threshold_bytes: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Manifest {
    #[serde(default)]
    entries: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ManifestEntry {
    index: u32,
    original_bytes: u64,
    path: String,
    #[serde(default, skip_serializing_if = "is_false")]
    compacted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    original_lines: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    threshold_bytes: Option<u64>,
}

impl From<&SidecarRecord> for ManifestEntry {
    fn from(record: &SidecarRecord) -> Self {
        Self {
            index: record.index,
            original_bytes: record.original_bytes,
            path: record.path.clone(),
            compacted: record.compacted,
            original_lines: Some(record.original_lines),
            tool_call_id: Some(record.tool_call_id.clone()),
            tool_title: record.tool_title.clone(),
            status: Some(record.status.clone()),
            threshold_bytes: Some(record.threshold_bytes),
        }
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn append_manifest(sidecar_dir: &Path, record: &SidecarRecord) -> std::io::Result<()> {
    let manifest_path = sidecar_dir.join("manifest.toml");
    let mut manifest = if manifest_path.exists() {
        let content = fs::read_to_string(&manifest_path)?;
        toml::from_str::<Manifest>(&content).unwrap_or_default()
    } else {
        Manifest::default()
    };
    manifest.entries.push(ManifestEntry::from(record));
    let serialized =
        toml::to_string_pretty(&manifest).map_err(|err| std::io::Error::other(err.to_string()))?;
    fs::write(manifest_path, serialized)
}

fn render_full_tool_output(tool_call_id: &str, status: &str, output: &str) -> String {
    format!("[tool:output] {tool_call_id} {status}\n{output}")
}

struct CompactedSummary<'summary> {
    tool_call_id: &'summary str,
    title: Option<&'summary str>,
    status: &'summary str,
    output: &'summary str,
    record: &'summary SidecarRecord,
    head_bytes: usize,
    tail_bytes: usize,
    threshold_bytes: u64,
}

fn render_compacted_summary(summary_input: CompactedSummary<'_>) -> String {
    let CompactedSummary {
        tool_call_id,
        title,
        status,
        output,
        record,
        head_bytes,
        tail_bytes,
        threshold_bytes,
    } = summary_input;

    let mut summary = String::new();
    summary.push_str(COMPACTED_MARKER);
    summary.push('\n');
    summary.push_str(&format!("tool_call_id: {tool_call_id}\n"));
    if let Some(title) = title.filter(|value| !value.trim().is_empty()) {
        summary.push_str(&format!("tool: {title}\n"));
    }
    summary.push_str(&format!("status: {status}\n"));
    summary.push_str(&format!("original_bytes: {}\n", record.original_bytes));
    summary.push_str(&format!("original_lines: {}\n", record.original_lines));
    summary.push_str(&format!("threshold_bytes: {threshold_bytes}\n"));
    summary.push_str(&format!("sidecar_path: {}\n", record.path));
    summary.push_str("\n--- head excerpt ---\n");
    summary.push_str(&head_excerpt(output, head_bytes));
    summary.push_str("\n--- tail excerpt ---\n");
    summary.push_str(&tail_excerpt(output, tail_bytes));
    if !summary.ends_with('\n') {
        summary.push('\n');
    }
    summary
}

fn relative_sidecar_path(sidecar_dir: &Path, filename: &str) -> String {
    let dirname = sidecar_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("tool_outputs");
    format!("{dirname}/{filename}")
}

fn line_count(output: &str) -> usize {
    if output.is_empty() {
        0
    } else {
        output.lines().count()
    }
}

fn head_excerpt(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    value[..floor_char_boundary(value, max_bytes)].to_string()
}

fn tail_excerpt(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let start = ceil_char_boundary(value, value.len().saturating_sub(max_bytes));
    value[start..].to_string()
}

fn floor_char_boundary(value: &str, mut index: usize) -> usize {
    index = index.min(value.len());
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(value: &str, mut index: usize) -> usize {
    index = index.min(value.len());
    while index < value.len() && !value.is_char_boundary(index) {
        index += 1;
    }
    index
}
