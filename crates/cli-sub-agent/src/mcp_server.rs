use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, Write};
use tempfile::TempDir;
use tracing::{debug, error, info};

use csa_config::ProjectConfig;
use csa_core::types::ToolName;
use csa_session::{delete_session, list_sessions};

/// MCP server implementation
///
/// Exposes CSA session management as MCP tools over JSON-RPC 2.0 stdio protocol.
pub(crate) async fn run_mcp_server() -> Result<()> {
    info!("Starting MCP server on stdio");

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let line = line.context("Failed to read line from stdin")?;
        let trimmed = line.trim();

        // Skip empty lines
        if trimmed.is_empty() {
            continue;
        }

        debug!("Received: {}", trimmed);

        // Parse JSON-RPC request
        let request: JsonRpcRequest = match serde_json::from_str(trimmed) {
            Ok(req) => req,
            Err(e) => {
                error!("Failed to parse JSON-RPC request: {}", e);
                // Send error response
                let error_response = JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32700,
                        message: format!("Parse error: {}", e),
                    }),
                    id: None,
                };
                write_response(&stdout, &error_response)?;
                continue;
            }
        };

        // Handle request
        let response = handle_request(request).await;

        // Write response
        write_response(&stdout, &response)?;
    }

    info!("MCP server shutting down");
    Ok(())
}

/// JSON-RPC 2.0 Request
#[derive(Deserialize)]
struct JsonRpcRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    method: String,
    #[serde(default)]
    params: Option<Value>,
    id: Option<Value>,
}

/// JSON-RPC 2.0 Response
#[derive(Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
    id: Option<Value>,
}

/// JSON-RPC 2.0 Error
#[derive(Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

/// MCP Tool Definition
#[derive(Serialize)]
struct McpToolDef {
    name: String,
    description: String,
    #[serde(rename = "inputSchema")]
    input_schema: Value,
}

/// MCP tool definitions
fn get_tools() -> Vec<McpToolDef> {
    vec![
        McpToolDef {
            name: "csa_session_list".to_string(),
            description: "List all CSA sessions for the current project".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "tool_filter": {
                        "type": "string",
                        "description": "Filter by tool name (comma-separated)"
                    }
                }
            }),
        },
        McpToolDef {
            name: "csa_session_delete".to_string(),
            description: "Delete a CSA session by ID".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "session_id": {
                        "type": "string",
                        "description": "Session ULID or prefix to delete"
                    }
                },
                "required": ["session_id"]
            }),
        },
        McpToolDef {
            name: "csa_gc".to_string(),
            description: "Run garbage collection to clean stale locks and empty sessions"
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "dry_run": {
                        "type": "boolean",
                        "description": "Show what would be removed without actually removing"
                    },
                    "max_age_days": {
                        "type": "number",
                        "description": "Remove sessions not accessed within N days"
                    }
                }
            }),
        },
        McpToolDef {
            name: "csa_run".to_string(),
            description: "Execute a task using CSA".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "tool": {
                        "type": "string",
                        "description": "Tool to use (gemini-cli, opencode, codex, claude-code)"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "Task prompt"
                    },
                    "session": {
                        "type": "string",
                        "description": "Session ID to resume (optional)"
                    },
                    "model_spec": {
                        "type": "string",
                        "description": "Model specification in format tool:model (optional)"
                    },
                    "ephemeral": {
                        "type": "boolean",
                        "description": "Run without persistent session (optional)"
                    }
                },
                "required": ["prompt"]
            }),
        },
    ]
}

/// Handle JSON-RPC request
async fn handle_request(request: JsonRpcRequest) -> JsonRpcResponse {
    let id = request.id.clone();

    match request.method.as_str() {
        "initialize" => {
            debug!("Handling initialize");
            JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                result: Some(serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": "csa-mcp",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                })),
                error: None,
                id,
            }
        }
        "notifications/initialized" => {
            debug!("Handling initialized notification");
            // Notification, no response needed
            JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                result: None,
                error: None,
                id: None,
            }
        }
        "tools/list" => {
            debug!("Handling tools/list");
            let tools = get_tools();
            JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                result: Some(serde_json::json!({
                    "tools": tools
                })),
                error: None,
                id,
            }
        }
        "tools/call" => {
            debug!("Handling tools/call");
            match handle_tool_call(request.params).await {
                Ok(result) => JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    result: Some(result),
                    error: None,
                    id,
                },
                Err(e) => JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32603,
                        message: e.to_string(),
                    }),
                    id,
                },
            }
        }
        "shutdown" => {
            debug!("Handling shutdown");
            JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                result: Some(serde_json::json!({})),
                error: None,
                id,
            }
        }
        _ => JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: format!("Method not found: {}", request.method),
            }),
            id,
        },
    }
}

/// Handle tool call
async fn handle_tool_call(params: Option<Value>) -> Result<Value> {
    let params = params.context("Missing params for tools/call")?;
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .context("Missing tool name")?;
    let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);

    debug!("Tool call: {} with args: {:?}", name, arguments);

    match name {
        "csa_session_list" => handle_session_list_tool(arguments).await,
        "csa_session_delete" => handle_session_delete_tool(arguments).await,
        "csa_gc" => handle_gc_tool(arguments).await,
        "csa_run" => handle_run_tool(arguments).await,
        _ => anyhow::bail!("Unknown tool: {}", name),
    }
}

/// Handle csa_session_list tool
async fn handle_session_list_tool(args: Value) -> Result<Value> {
    let tool_filter = args
        .get("tool_filter")
        .and_then(|v| v.as_str())
        .map(|s| s.split(',').collect::<Vec<&str>>());

    let project_root = crate::determine_project_root(None)?;
    let sessions = list_sessions(&project_root, tool_filter.as_deref())?;

    // Format as MCP content
    let mut content_text = String::new();
    if sessions.is_empty() {
        content_text.push_str("No sessions found.\n");
    } else {
        content_text.push_str(&format!(
            "{:<11}  {:<19}  {:<30}  {:<20}  TOKENS\n",
            "SESSION", "LAST ACCESSED", "DESCRIPTION", "TOOLS"
        ));
        content_text.push_str(&format!("{}\n", "-".repeat(100)));

        for session in sessions {
            let short_id = &session.meta_session_id[..11.min(session.meta_session_id.len())];
            let desc = session
                .description
                .as_deref()
                .filter(|d| !d.is_empty())
                .unwrap_or("-");
            let desc_display = if desc.len() > 30 {
                format!("{}...", &desc[..27])
            } else {
                desc.to_string()
            };
            let tools: Vec<&String> = session.tools.keys().collect();
            let tools_str = if tools.is_empty() {
                "-".to_string()
            } else {
                tools
                    .iter()
                    .map(|t| t.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            };

            let tokens_str = if let Some(ref usage) = session.total_token_usage {
                if let Some(total) = usage.total_tokens {
                    if let Some(cost) = usage.estimated_cost_usd {
                        format!("{}tok ${:.4}", total, cost)
                    } else {
                        format!("{}tok", total)
                    }
                } else if let (Some(input), Some(output)) =
                    (usage.input_tokens, usage.output_tokens)
                {
                    let total = input + output;
                    if let Some(cost) = usage.estimated_cost_usd {
                        format!("{}tok ${:.4}", total, cost)
                    } else {
                        format!("{}tok", total)
                    }
                } else {
                    "-".to_string()
                }
            } else {
                "-".to_string()
            };

            content_text.push_str(&format!(
                "{:<11}  {:<19}  {:<30}  {:<20}  {}\n",
                short_id,
                session.last_accessed.format("%Y-%m-%d %H:%M"),
                desc_display,
                tools_str,
                tokens_str,
            ));
        }
    }

    Ok(serde_json::json!({
        "content": [
            {
                "type": "text",
                "text": content_text
            }
        ]
    }))
}

/// Handle csa_session_delete tool
async fn handle_session_delete_tool(args: Value) -> Result<Value> {
    let session_id = args
        .get("session_id")
        .and_then(|v| v.as_str())
        .context("Missing session_id argument")?;

    let project_root = crate::determine_project_root(None)?;
    let sessions_dir = csa_session::get_session_root(&project_root)?.join("sessions");
    let resolved_id = csa_session::resolve_session_prefix(&sessions_dir, session_id)?;

    delete_session(&project_root, &resolved_id)?;

    Ok(serde_json::json!({
        "content": [
            {
                "type": "text",
                "text": format!("Deleted session: {}", resolved_id)
            }
        ]
    }))
}

/// Handle csa_gc tool
async fn handle_gc_tool(args: Value) -> Result<Value> {
    let dry_run = args
        .get("dry_run")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let max_age_days = args.get("max_age_days").and_then(|v| v.as_u64());

    // Call gc logic (MCP server always uses Text format, response is wrapped in JSON-RPC)
    crate::gc::handle_gc(dry_run, max_age_days, crate::OutputFormat::Text)?;

    let msg = if dry_run {
        "Garbage collection dry-run completed (see logs for details)"
    } else {
        "Garbage collection completed"
    };

    Ok(serde_json::json!({
        "content": [
            {
                "type": "text",
                "text": msg
            }
        ]
    }))
}

/// Handle csa_run tool
async fn handle_run_tool(args: Value) -> Result<Value> {
    // Extract arguments
    let tool_str = args.get("tool").and_then(|v| v.as_str());
    let prompt = args
        .get("prompt")
        .and_then(|v| v.as_str())
        .context("Missing prompt argument")?;
    let session_arg = args
        .get("session")
        .and_then(|v| v.as_str())
        .map(String::from);
    let model_spec = args.get("model_spec").and_then(|v| v.as_str());
    let ephemeral = args
        .get("ephemeral")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Parse tool if provided
    let tool = if let Some(tool_str) = tool_str {
        Some(parse_tool_name(tool_str)?)
    } else {
        None
    };

    // Determine project root
    let project_root = crate::determine_project_root(None)?;

    // Load config
    let config = ProjectConfig::load(&project_root)?;

    // Check recursion depth
    let current_depth: u32 = std::env::var("CSA_DEPTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let max_depth = config
        .as_ref()
        .map(|c| c.project.max_recursion_depth)
        .unwrap_or(5u32);

    if current_depth > max_depth {
        return Ok(serde_json::json!({
            "content": [
                {
                    "type": "text",
                    "text": format!(
                        "Error: Max recursion depth ({}) exceeded. Current: {}",
                        max_depth, current_depth
                    )
                }
            ]
        }));
    }

    // Resolve tool and model
    let (resolved_tool, resolved_model_spec, resolved_model) =
        crate::run_helpers::resolve_tool_and_model(
            tool,
            model_spec,
            None,
            config.as_ref(),
            &project_root,
        )?;

    // Build executor
    let executor = crate::run_helpers::build_executor(
        &resolved_tool,
        resolved_model_spec.as_deref(),
        resolved_model.as_deref(),
        None,
    )?;

    // Check tool is installed
    if csa_process::check_tool_installed(executor.executable_name())
        .await
        .is_err()
    {
        return Ok(serde_json::json!({
            "content": [
                {
                    "type": "text",
                    "text": format!(
                        "Error: Tool '{}' is not installed.\n\n{}\n\nOr disable it in .csa/config.toml",
                        executor.tool_name(),
                        executor.install_hint()
                    )
                }
            ]
        }));
    }

    // Check tool is enabled in config
    if let Some(ref cfg) = config {
        if !cfg.is_tool_enabled(executor.tool_name()) {
            return Ok(serde_json::json!({
                "content": [
                    {
                        "type": "text",
                        "text": format!(
                            "Error: Tool '{}' is disabled in project config",
                            executor.tool_name()
                        )
                    }
                ]
            }));
        }
    }

    // Load global config for env injection
    let global_config = csa_config::GlobalConfig::load().ok();
    let extra_env = global_config
        .as_ref()
        .and_then(|gc| gc.env_vars(executor.tool_name()).cloned());
    let extra_env_ref = extra_env.as_ref();

    // Execute
    let result = if ephemeral {
        // Ephemeral: use temp directory
        let temp_dir = TempDir::new()?;
        executor
            .execute_in(prompt, temp_dir.path(), extra_env_ref)
            .await?
    } else {
        // Persistent session
        crate::execute_with_session(
            &executor,
            &resolved_tool,
            prompt,
            session_arg.clone(),
            None, // description
            None, // parent
            &project_root,
            config.as_ref(),
            extra_env_ref,
        )
        .await?
    };

    // Format response
    let mut response_text = result.output.clone();

    // Add metadata section
    response_text.push_str("\n\n--- Execution Metadata ---\n");
    if !ephemeral {
        if let Some(ref sid) = session_arg {
            response_text.push_str(&format!("Session ID: {}\n", sid));
        } else {
            // For new sessions, we don't have the session ID here
            // since execute_with_session doesn't return it
            response_text.push_str("Session ID: (new session created)\n");
        }
    }
    response_text.push_str(&format!("Tool: {}\n", executor.tool_name()));
    response_text.push_str(&format!("Exit Code: {}\n", result.exit_code));

    Ok(serde_json::json!({
        "content": [
            {
                "type": "text",
                "text": response_text
            }
        ]
    }))
}

/// Parse tool name from string
fn parse_tool_name(tool_str: &str) -> Result<ToolName> {
    match tool_str {
        "gemini-cli" => Ok(ToolName::GeminiCli),
        "opencode" => Ok(ToolName::Opencode),
        "codex" => Ok(ToolName::Codex),
        "claude-code" => Ok(ToolName::ClaudeCode),
        _ => anyhow::bail!("Unknown tool: {}", tool_str),
    }
}

/// Write JSON-RPC response to stdout
fn write_response(stdout: &std::io::Stdout, response: &JsonRpcResponse) -> Result<()> {
    let mut out = stdout.lock();
    serde_json::to_writer(&mut out, response).context("Failed to serialize response")?;
    out.write_all(b"\n")
        .context("Failed to write newline to stdout")?;
    out.flush().context("Failed to flush stdout")?;
    Ok(())
}
