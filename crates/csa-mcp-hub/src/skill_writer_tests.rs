use std::path::Path;

use anyhow::Result;
use rmcp::model::Tool;
use serde_json::json;

use crate::skill_writer::{McpServerSnapshot, RegistryFile, SkillWriter};

fn tool(name: &str, description: &str) -> Tool {
    serde_json::from_value(json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": {
                "project_root": {"type": "string"},
                "limit": {"type": "integer"}
            },
            "required": ["project_root"]
        }
    }))
    .expect("tool json should deserialize")
}

async fn read_registry(path: &Path) -> Result<RegistryFile> {
    let raw = tokio::fs::read_to_string(path).await?;
    Ok(toml::from_str::<RegistryFile>(&raw)?)
}

#[tokio::test]
async fn skill_writer_generates_routing_guide_files() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let writer = SkillWriter::new(tmp.path().to_path_buf(), Vec::new(), Vec::new());

    writer
        .regenerate(
            vec![
                McpServerSnapshot {
                    transport_type: "stdio".to_string(),
                    name: "echo".to_string(),
                    status: "ready".to_string(),
                    tools: vec![tool("echo_tool", "Echo input")],
                },
                McpServerSnapshot {
                    transport_type: "stdio".to_string(),
                    name: "deepwiki".to_string(),
                    status: "ready".to_string(),
                    tools: vec![tool("ask_question", "Ask DeepWiki")],
                },
            ],
            true,
        )
        .await?;

    let skill_root = tmp.path().join(".claude/skills/mcp-hub-routing-guide");
    let skill = tokio::fs::read_to_string(skill_root.join("SKILL.md")).await?;
    assert!(skill.contains("L0 - Hub Basics"));
    assert!(skill.contains("L1 - Enabled MCP Overview"));
    assert!(skill.contains("L2 - Per-MCP Tool Definitions"));
    assert!(skill.contains("`echo`"));

    let registry = read_registry(&skill_root.join("references/mcp-registry.toml")).await?;
    assert_eq!(registry.mcps.len(), 2);
    assert!(skill_root.join("mcps/echo.md").exists());
    assert!(skill_root.join("mcps/deepwiki.md").exists());

    let before = registry
        .mcps
        .iter()
        .find(|entry| entry.name == "echo")
        .expect("echo entry")
        .updated_at
        .clone();
    writer
        .regenerate(
            vec![
                McpServerSnapshot {
                    transport_type: "stdio".to_string(),
                    name: "echo".to_string(),
                    status: "ready".to_string(),
                    tools: vec![tool("echo_tool", "Echo input")],
                },
                McpServerSnapshot {
                    transport_type: "stdio".to_string(),
                    name: "deepwiki".to_string(),
                    status: "ready".to_string(),
                    tools: vec![tool("ask_question", "Ask DeepWiki")],
                },
            ],
            false,
        )
        .await?;
    let after_registry = read_registry(&skill_root.join("references/mcp-registry.toml")).await?;
    let after = after_registry
        .mcps
        .iter()
        .find(|entry| entry.name == "echo")
        .expect("echo entry")
        .updated_at
        .clone();
    assert_eq!(before, after);
    Ok(())
}

#[tokio::test]
async fn skill_writer_applies_visibility_filters() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let writer = SkillWriter::new(
        tmp.path().to_path_buf(),
        vec!["echo".to_string(), "memory".to_string()],
        vec!["memory".to_string()],
    );

    writer
        .regenerate(
            vec![
                McpServerSnapshot {
                    transport_type: "stdio".to_string(),
                    name: "echo".to_string(),
                    status: "ready".to_string(),
                    tools: vec![tool("echo_tool", "Echo input")],
                },
                McpServerSnapshot {
                    transport_type: "stdio".to_string(),
                    name: "memory".to_string(),
                    status: "ready".to_string(),
                    tools: vec![tool("remember", "Store notes")],
                },
                McpServerSnapshot {
                    transport_type: "stdio".to_string(),
                    name: "deepwiki".to_string(),
                    status: "ready".to_string(),
                    tools: vec![tool("ask_question", "Ask DeepWiki")],
                },
            ],
            true,
        )
        .await?;

    let skill_root = tmp.path().join(".claude/skills/mcp-hub-routing-guide");
    let registry = read_registry(&skill_root.join("references/mcp-registry.toml")).await?;
    let names = registry
        .mcps
        .iter()
        .map(|entry| entry.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["echo"]);
    assert!(skill_root.join("mcps/echo.md").exists());
    assert!(!skill_root.join("mcps/memory.md").exists());
    assert!(!skill_root.join("mcps/deepwiki.md").exists());
    Ok(())
}

#[tokio::test]
async fn skill_writer_generates_distinct_doc_names_for_similar_server_names() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let writer = SkillWriter::new(tmp.path().to_path_buf(), Vec::new(), Vec::new());

    writer
        .regenerate(
            vec![
                McpServerSnapshot {
                    transport_type: "stdio".to_string(),
                    name: "a.b".to_string(),
                    status: "ready".to_string(),
                    tools: vec![tool("x", "x")],
                },
                McpServerSnapshot {
                    transport_type: "stdio".to_string(),
                    name: "a-b".to_string(),
                    status: "ready".to_string(),
                    tools: vec![tool("y", "y")],
                },
            ],
            true,
        )
        .await?;

    let skill_root = tmp.path().join(".claude/skills/mcp-hub-routing-guide");
    assert!(skill_root.join("mcps/a_b.md").exists());
    assert!(skill_root.join("mcps/a-b.md").exists());
    Ok(())
}
