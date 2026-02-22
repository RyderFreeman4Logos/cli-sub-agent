use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};
use rmcp::model::Tool;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::{RwLock, mpsc};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

use crate::config::HubConfig;
use crate::registry::McpRegistry;

const ROUTING_SKILL_NAME: &str = "mcp-hub-routing-guide";
const STARTUP_LIST_RETRIES: u32 = 20;
const STARTUP_LIST_RETRY_DELAY_MS: u64 = 300;
const PERIODIC_REFRESH_SECS: u64 = 20;
const SKILL_REFRESH_CHANNEL_CAPACITY: usize = 16;

#[derive(Debug, Clone)]
pub(crate) struct McpServerSnapshot {
    pub(crate) name: String,
    pub(crate) status: String,
    pub(crate) transport_type: String,
    pub(crate) tools: Vec<Tool>,
}

#[derive(Debug, Clone)]
pub(crate) enum SkillRefreshSignal {
    RegenerateAll,
    ToolsListChanged { server: Option<String> },
}

#[derive(Debug)]
pub(crate) struct SkillSyncHandle {
    notifier: SkillRefreshNotifier,
    join_handle: JoinHandle<()>,
}

impl SkillSyncHandle {
    pub(crate) fn notifier(&self) -> SkillRefreshNotifier {
        self.notifier.clone()
    }

    pub(crate) async fn shutdown(self) {
        self.join_handle.abort();
        let _ = self.join_handle.await;
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SkillRefreshNotifier {
    signal_tx: mpsc::Sender<SkillRefreshSignal>,
    refresh_pending: Arc<AtomicBool>,
}

impl SkillRefreshNotifier {
    pub(crate) fn new(signal_tx: mpsc::Sender<SkillRefreshSignal>) -> Self {
        Self {
            signal_tx,
            refresh_pending: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn notify(&self, signal: SkillRefreshSignal) {
        if self.refresh_pending.swap(true, Ordering::AcqRel) {
            tracing::debug!("skipping duplicate skill refresh signal while one is pending");
            return;
        }

        match self.signal_tx.try_send(signal) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.refresh_pending.store(false, Ordering::Release);
                tracing::warn!("dropping skill refresh signal because queue is full");
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.refresh_pending.store(false, Ordering::Release);
                tracing::debug!("dropping skill refresh signal because queue is closed");
            }
        }
    }
}

pub(crate) fn spawn_skill_sync_task(cfg: HubConfig, registry: Arc<McpRegistry>) -> SkillSyncHandle {
    let (signal_tx, signal_rx) = mpsc::channel(SKILL_REFRESH_CHANNEL_CAPACITY);
    let notifier = SkillRefreshNotifier::new(signal_tx);
    let refresh_pending_for_loop = Arc::clone(&notifier.refresh_pending);
    let join_handle = tokio::spawn(async move {
        if let Err(error) =
            run_skill_sync_loop(cfg, registry, signal_rx, refresh_pending_for_loop).await
        {
            tracing::warn!(error = %error, "mcp-hub routing-guide sync loop stopped");
        }
    });

    SkillSyncHandle {
        notifier,
        join_handle,
    }
}

pub(crate) async fn regenerate_routing_skill_once(cfg: HubConfig) -> Result<()> {
    let registry = Arc::new(McpRegistry::new(cfg.mcp_servers.clone()));
    let writer = SkillWriter::new(
        cfg.project_root.clone(),
        cfg.mcp_whitelist.clone(),
        cfg.mcp_blacklist.clone(),
    );
    let snapshots = collect_snapshots(
        registry.as_ref(),
        STARTUP_LIST_RETRIES,
        Duration::from_millis(STARTUP_LIST_RETRY_DELAY_MS),
    )
    .await;
    writer.regenerate(snapshots, true).await?;
    registry.shutdown_all().await?;
    Ok(())
}

pub(crate) fn parse_tools_list_changed_signal(payload: &str) -> Option<SkillRefreshSignal> {
    let message: Value = serde_json::from_str(payload.trim()).ok()?;
    let method = message.get("method")?.as_str()?;
    if method != "notifications/tools/list_changed" {
        return None;
    }

    let server = message
        .get("params")
        .and_then(|params| {
            params
                .get("server")
                .or_else(|| params.get("server_name"))
                .or_else(|| params.get("mcp"))
                .or_else(|| params.get("name"))
        })
        .and_then(Value::as_str)
        .map(str::to_string);

    Some(SkillRefreshSignal::ToolsListChanged { server })
}

async fn run_skill_sync_loop(
    cfg: HubConfig,
    registry: Arc<McpRegistry>,
    mut signal_rx: mpsc::Receiver<SkillRefreshSignal>,
    refresh_pending: Arc<AtomicBool>,
) -> Result<()> {
    let writer = SkillWriter::new(cfg.project_root, cfg.mcp_whitelist, cfg.mcp_blacklist);

    let startup_snapshots = collect_snapshots(
        registry.as_ref(),
        STARTUP_LIST_RETRIES,
        Duration::from_millis(STARTUP_LIST_RETRY_DELAY_MS),
    )
    .await;
    writer.regenerate(startup_snapshots, true).await?;

    let mut refresh = tokio::time::interval(Duration::from_secs(PERIODIC_REFRESH_SECS));
    refresh.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = refresh.tick() => {
                let snapshots = collect_snapshots(registry.as_ref(), 1, Duration::from_millis(0)).await;
                if let Err(error) = writer.regenerate(snapshots, false).await {
                    tracing::warn!(error = %error, "periodic routing-guide refresh failed");
                }
            }
            maybe_signal = signal_rx.recv() => {
                let Some(signal) = maybe_signal else {
                    break;
                };

                let force_full = matches!(signal, SkillRefreshSignal::RegenerateAll);
                if let SkillRefreshSignal::ToolsListChanged { server } = &signal {
                    tracing::debug!(server = ?server, "received tools/list_changed signal");
                }

                let snapshots = collect_snapshots(
                    registry.as_ref(),
                    if force_full { STARTUP_LIST_RETRIES } else { 1 },
                    Duration::from_millis(STARTUP_LIST_RETRY_DELAY_MS),
                )
                .await;
                if let Err(error) = writer.regenerate(snapshots, force_full).await {
                    tracing::warn!(error = %error, "signal-triggered routing-guide refresh failed");
                }
                refresh_pending.store(false, Ordering::Release);
            }
        }
    }

    Ok(())
}

async fn collect_snapshots(
    registry: &McpRegistry,
    attempts: u32,
    retry_delay: Duration,
) -> Vec<McpServerSnapshot> {
    let mut names = registry.server_names();
    names.sort();

    let mut snapshots = Vec::with_capacity(names.len());
    for name in names {
        snapshots
            .push(collect_single_snapshot(registry, &name, attempts.max(1), retry_delay).await);
    }
    snapshots
}

async fn collect_single_snapshot(
    registry: &McpRegistry,
    server_name: &str,
    attempts: u32,
    retry_delay: Duration,
) -> McpServerSnapshot {
    let transport_type = registry.transport_label(server_name).to_string();
    let mut last_error = String::new();

    for attempt in 0..attempts {
        let cancellation = CancellationToken::new();
        match registry.list_tools(server_name, cancellation).await {
            Ok(tools) => {
                return McpServerSnapshot {
                    name: server_name.to_string(),
                    status: "ready".to_string(),
                    transport_type,
                    tools,
                };
            }
            Err(error) => {
                last_error = error.to_string();
                if attempt + 1 < attempts && !retry_delay.is_zero() {
                    tokio::time::sleep(retry_delay).await;
                }
            }
        }
    }

    tracing::warn!(
        server = %server_name,
        error = %last_error,
        "failed to collect tools/list for MCP server"
    );
    McpServerSnapshot {
        name: server_name.to_string(),
        status: "error".to_string(),
        transport_type,
        tools: Vec::new(),
    }
}

pub(crate) struct SkillWriter {
    skill_root: PathBuf,
    visibility: VisibilityFilter,
    write_guard: Arc<RwLock<()>>,
}

impl SkillWriter {
    pub(crate) fn new(
        project_root: PathBuf,
        mcp_whitelist: Vec<String>,
        mcp_blacklist: Vec<String>,
    ) -> Self {
        Self {
            skill_root: project_root
                .join(".claude")
                .join("skills")
                .join(ROUTING_SKILL_NAME),
            visibility: VisibilityFilter::new(mcp_whitelist, mcp_blacklist),
            write_guard: Arc::new(RwLock::new(())),
        }
    }

    pub(crate) async fn regenerate(
        &self,
        snapshots: Vec<McpServerSnapshot>,
        force_full: bool,
    ) -> Result<()> {
        let _guard = self.write_guard.write().await;
        self.regenerate_locked(snapshots, force_full).await
    }

    async fn regenerate_locked(
        &self,
        snapshots: Vec<McpServerSnapshot>,
        force_full: bool,
    ) -> Result<()> {
        let references_dir = self.skill_root.join("references");
        let mcps_dir = self.skill_root.join("mcps");
        tokio::fs::create_dir_all(&references_dir)
            .await
            .with_context(|| format!("failed to create {}", references_dir.display()))?;
        tokio::fs::create_dir_all(&mcps_dir)
            .await
            .with_context(|| format!("failed to create {}", mcps_dir.display()))?;

        let registry_path = references_dir.join("mcp-registry.toml");
        let existing = load_registry(&registry_path).await;
        let existing_map = existing
            .mcps
            .iter()
            .map(|entry| (entry.name.clone(), entry.clone()))
            .collect::<HashMap<_, _>>();

        let now = now_timestamp();
        let mut entries = Vec::new();
        let mut keep_names = HashSet::new();

        for snapshot in snapshots {
            if !self.visibility.allows(&snapshot.name) {
                continue;
            }
            keep_names.insert(snapshot.name.clone());
            let doc = ServerDoc::from_snapshot(snapshot);
            let previous = existing_map.get(&doc.name);
            let doc_path = mcps_dir.join(&doc.doc_file);
            let unchanged = previous
                .map(|entry| {
                    entry.tool_digest == doc.tool_digest
                        && entry.status == doc.status
                        && entry.tool_count == doc.tools.len()
                        && entry.purpose == doc.purpose
                })
                .unwrap_or(false)
                && doc_path.exists();

            let updated_at = if !force_full && unchanged {
                previous
                    .map(|entry| entry.updated_at.clone())
                    .unwrap_or_else(|| now.clone())
            } else {
                let content = render_mcp_doc(&doc, &now);
                write_atomic_if_changed(&doc_path, &content).await?;
                now.clone()
            };

            entries.push(RegistryMcpEntry {
                name: doc.name,
                status: doc.status,
                transport: doc.transport_type,
                tool_count: doc.tools.len(),
                updated_at,
                purpose: doc.purpose,
                tool_digest: doc.tool_digest,
                doc_file: doc.doc_file,
            });
        }

        for stale in existing
            .mcps
            .iter()
            .filter(|entry| !keep_names.contains(&entry.name))
        {
            let stale_doc = if stale.doc_file.is_empty() {
                format!("{}.md", sanitize_name(&stale.name))
            } else {
                stale.doc_file.clone()
            };
            let stale_path = mcps_dir.join(stale_doc);
            remove_if_exists(&stale_path).await?;
        }

        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let next_registry = RegistryFile {
            generated_at: now.clone(),
            mcps: entries.clone(),
        };

        let skill_path = self.skill_root.join("SKILL.md");
        let hub_connection_path = references_dir.join("hub-connection.md");
        write_atomic_if_changed(&skill_path, &render_skill_markdown(&entries)).await?;
        write_atomic_if_changed(&hub_connection_path, HUB_CONNECTION_REFERENCE).await?;
        write_atomic_if_changed(
            &registry_path,
            &toml::to_string_pretty(&next_registry).context("failed to encode mcp-registry")?,
        )
        .await?;

        Ok(())
    }
}

#[derive(Debug, Clone)]
struct VisibilityFilter {
    include: Option<HashSet<String>>,
    exclude: HashSet<String>,
}

impl VisibilityFilter {
    fn new(whitelist: Vec<String>, blacklist: Vec<String>) -> Self {
        let include = if whitelist.is_empty() {
            None
        } else {
            Some(whitelist.into_iter().collect())
        };

        Self {
            include,
            exclude: blacklist.into_iter().collect(),
        }
    }

    fn allows(&self, name: &str) -> bool {
        if let Some(include) = &self.include
            && !include.contains(name)
        {
            return false;
        }
        !self.exclude.contains(name)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct RegistryFile {
    #[serde(default)]
    generated_at: String,
    #[serde(default)]
    mcps: Vec<RegistryMcpEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RegistryMcpEntry {
    name: String,
    status: String,
    #[serde(default)]
    transport: String,
    tool_count: usize,
    updated_at: String,
    purpose: String,
    tool_digest: String,
    #[serde(default)]
    doc_file: String,
}

#[derive(Debug, Clone)]
struct ServerDoc {
    name: String,
    status: String,
    transport_type: String,
    purpose: String,
    doc_file: String,
    tool_digest: String,
    tools: Vec<ToolDoc>,
}

impl ServerDoc {
    fn from_snapshot(snapshot: McpServerSnapshot) -> Self {
        let mut tools = snapshot
            .tools
            .iter()
            .map(ToolDoc::from_tool)
            .collect::<Vec<_>>();
        tools.sort_by(|a, b| a.name.cmp(&b.name));

        let purpose = match tools.first() {
            Some(first) if !first.description.is_empty() => format!(
                "{} tools available. Example: {} - {}",
                tools.len(),
                first.name,
                first.description
            ),
            Some(first) => format!("{} tools available. Example: {}", tools.len(), first.name),
            None => "No tools currently available".to_string(),
        };
        let doc_file = format!("{}.md", sanitize_name(&snapshot.name));
        let tool_digest = digest_tool_docs(&snapshot.status, &tools);

        Self {
            name: snapshot.name,
            status: snapshot.status,
            transport_type: snapshot.transport_type,
            purpose,
            doc_file,
            tool_digest,
            tools,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct ToolDoc {
    name: String,
    description: String,
    input_schema: Value,
}

impl ToolDoc {
    fn from_tool(tool: &Tool) -> Self {
        let value = serde_json::to_value(tool).unwrap_or(Value::Null);
        let name = value
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| tool.name.to_string());
        let description = value
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let input_schema = value
            .get("inputSchema")
            .cloned()
            .or_else(|| value.get("input_schema").cloned())
            .unwrap_or_else(|| serde_json::json!({ "type": "object", "properties": {} }));

        Self {
            name,
            description,
            input_schema,
        }
    }
}

fn digest_tool_docs(status: &str, tools: &[ToolDoc]) -> String {
    #[derive(Serialize)]
    struct DigestPayload<'a> {
        status: &'a str,
        tools: &'a [ToolDoc],
    }

    let payload = serde_json::to_vec(&DigestPayload { status, tools }).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(payload);
    format!("{:x}", hasher.finalize())
}

fn sanitize_name(name: &str) -> String {
    let mut normalized = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            normalized.push(ch);
        } else {
            normalized.push('_');
        }
    }

    if normalized.is_empty() {
        "unknown".to_string()
    } else {
        normalized
    }
}

fn render_skill_markdown(entries: &[RegistryMcpEntry]) -> String {
    let mut lines = vec![
        "# MCP Hub Routing Guide".to_string(),
        String::new(),
        "## L0 - Hub Basics (always visible)".to_string(),
        "- csa-mcp-hub is the shared MCP router used by CSA sessions.".to_string(),
        "- Start hub: `csa mcp-hub serve --foreground`".to_string(),
        "- Check hub: `csa mcp-hub status`".to_string(),
        "- Trigger skill regeneration: `csa mcp-hub gen-skill`".to_string(),
        "- Connection and troubleshooting: `references/hub-connection.md`".to_string(),
        String::new(),
        "## L1 - Enabled MCP Overview".to_string(),
    ];

    if entries.is_empty() {
        lines.push("- No MCP servers are currently visible to this project.".to_string());
    } else {
        for entry in entries {
            let transport_tag = if entry.transport.is_empty() || entry.transport == "stdio" {
                String::new()
            } else {
                format!(" [{}]", entry.transport)
            };
            lines.push(format!(
                "- `{}`: {}{}",
                entry.name, entry.purpose, transport_tag
            ));
        }
    }

    lines.push(String::new());
    lines.push("## L2 - Per-MCP Tool Definitions (on-demand)".to_string());
    lines.push("- Tool details live in `mcps/<name>.md`; open only the MCP you need.".to_string());
    lines.push(
        "- Structured metadata for automation lives in `references/mcp-registry.toml`.".to_string(),
    );
    lines.push(String::new());

    lines.join("\n")
}

fn render_mcp_doc(doc: &ServerDoc, updated_at: &str) -> String {
    let mut lines = vec![
        format!("# MCP: {}", doc.name),
        String::new(),
        format!("Purpose: {}", doc.purpose),
        format!("Status: {}", doc.status),
        format!("Transport: {}", doc.transport_type),
        format!("Updated At: {}", updated_at),
        String::new(),
        format!("Tools ({}):", doc.tools.len()),
    ];

    if doc.tools.is_empty() {
        lines.push("- No tools currently available.".to_string());
        lines.push(String::new());
        return lines.join("\n");
    }

    for tool in &doc.tools {
        lines.push(format!("- `{}`", tool.name));
        if tool.description.is_empty() {
            lines.push("  Description: n/a".to_string());
        } else {
            lines.push(format!("  Description: {}", tool.description));
        }
        lines.push(format!(
            "  Parameters: {}",
            summarize_params(&tool.input_schema)
        ));
    }
    lines.push(String::new());
    lines.join("\n")
}

fn summarize_params(schema: &Value) -> String {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return "none".to_string();
    };
    if properties.is_empty() {
        return "none".to_string();
    }

    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();

    let mut ordered = BTreeMap::new();
    for (key, value) in properties {
        let type_text = match value.get("type") {
            Some(Value::String(single)) => single.clone(),
            Some(Value::Array(arr)) => arr
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("|"),
            _ => "any".to_string(),
        };
        let marker = if required.contains(key) {
            "required"
        } else {
            "optional"
        };
        ordered.insert(key.clone(), format!("{key}:{type_text} ({marker})"));
    }

    ordered.into_values().collect::<Vec<_>>().join(", ")
}

async fn load_registry(path: &Path) -> RegistryFile {
    let content = match tokio::fs::read_to_string(path).await {
        Ok(content) => content,
        Err(error) if error.kind() == ErrorKind::NotFound => return RegistryFile::default(),
        Err(error) => {
            tracing::warn!(path = %path.display(), error = %error, "failed to read existing mcp-registry");
            return RegistryFile::default();
        }
    };

    match toml::from_str::<RegistryFile>(&content) {
        Ok(parsed) => parsed,
        Err(error) => {
            tracing::warn!(path = %path.display(), error = %error, "failed to parse existing mcp-registry");
            RegistryFile::default()
        }
    }
}

async fn write_atomic_if_changed(path: &Path, content: &str) -> Result<bool> {
    match tokio::fs::read_to_string(path).await {
        Ok(existing) if existing == content => return Ok(false),
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to read existing file: {}", path.display()));
        }
    }

    write_atomic(path, content).await?;
    Ok(true)
}

async fn write_atomic(path: &Path, content: &str) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("missing parent directory for {}", path.display()))?;
    tokio::fs::create_dir_all(parent)
        .await
        .with_context(|| format!("failed to create parent directory {}", parent.display()))?;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("tmp");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp_path = parent.join(format!(".{file_name}.tmp-{}-{nonce}", std::process::id()));
    tokio::fs::write(&temp_path, content)
        .await
        .with_context(|| format!("failed to write temp file {}", temp_path.display()))?;
    tokio::fs::rename(&temp_path, path)
        .await
        .with_context(|| format!("failed to atomically replace {}", path.display()))
}

async fn remove_if_exists(path: &Path) -> Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("failed to remove {}", path.display())),
    }
}

fn now_timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

const HUB_CONNECTION_REFERENCE: &str = r#"# Hub Connection

## Connection Methods
- Unix socket (default): `$XDG_RUNTIME_DIR/cli-sub-agent/mcp-hub.sock` or `/tmp/cli-sub-agent-$UID/mcp-hub.sock`
- HTTP/SSE endpoint: start hub with `--http-bind` and `--http-port`
- Systemd socket activation: `systemctl --user enable --now mcp-hub.socket`

## Common Commands
- Start in foreground: `csa mcp-hub serve --foreground`
- Start in background: `csa mcp-hub serve --background`
- Status query: `csa mcp-hub status`
- Stop hub: `csa mcp-hub stop`
- Regenerate routing guide: `csa mcp-hub gen-skill`

## Troubleshooting
- Socket missing: ensure hub is running and check `mcp-hub status`
- Permission denied: verify socket owner and mode (`0600`)
- Empty tools list: validate MCP server command/env in global config
- Refresh routing guide after MCP changes: run `csa mcp-hub gen-skill`
"#;

#[cfg(test)]
#[path = "skill_writer_tests.rs"]
mod tests;
