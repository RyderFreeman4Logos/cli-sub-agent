use crate::cli::{EpicCommands, EpicFormat};
use anyhow::Result;
use csa_todo::{EpicPlan, Story, StoryStatus, TodoManager};

pub(crate) fn handle_epic_command(command: EpicCommands) -> Result<()> {
    match command {
        EpicCommands::Show {
            timestamp,
            epic_format,
            cd,
        } => handle_epic_show(timestamp, epic_format, cd),
        EpicCommands::Validate { timestamp, cd } => handle_epic_validate(timestamp, cd),
        EpicCommands::Next { timestamp, cd } => handle_epic_next(timestamp, cd),
    }
}

fn handle_epic_show(
    timestamp: Option<String>,
    format: EpicFormat,
    cd: Option<String>,
) -> Result<()> {
    let (_ts, epic_plan) = load_required_epic_plan(timestamp, cd)?;
    epic_plan.validate()?;

    let rendered = match format {
        EpicFormat::Terminal => render_epic_plan_terminal(&epic_plan)?,
        EpicFormat::Mermaid => epic_plan.to_dependency_graph().to_mermaid(),
        EpicFormat::Dot => epic_plan.to_dependency_graph().to_dot(),
        EpicFormat::Json => serde_json::to_string_pretty(&epic_plan)?,
    };

    println!("{rendered}");
    Ok(())
}

fn handle_epic_validate(timestamp: Option<String>, cd: Option<String>) -> Result<()> {
    let (ts, epic_plan) = load_required_epic_plan(timestamp, cd)?;

    epic_plan.validate()?;
    println!("Epic plan '{ts}' is valid.");
    Ok(())
}

fn handle_epic_next(timestamp: Option<String>, cd: Option<String>) -> Result<()> {
    let (_ts, epic_plan) = load_required_epic_plan(timestamp, cd)?;

    epic_plan.validate()?;
    let actionable = epic_plan.next_actionable();
    if actionable.is_empty() {
        println!("No actionable stories.");
        return Ok(());
    }

    println!("{:<20}  {:<10}  {:<40}  SUMMARY", "ID", "STATUS", "BRANCH");
    for story in actionable {
        println!(
            "{:<20}  {:<10}  {:<40}  {}",
            story.id,
            story_status_label(story.status),
            truncate_branch(&story.branch, 40),
            story.summary
        );
    }

    Ok(())
}

fn load_required_epic_plan(
    timestamp: Option<String>,
    cd: Option<String>,
) -> Result<(String, EpicPlan)> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let manager = TodoManager::new(&project_root)?;
    let ts = crate::todo_cmd::resolve_timestamp(&manager, timestamp.as_deref())?;
    manager.load(&ts)?;

    let Some(epic_plan) = manager.load_epic_plan(&ts)? else {
        anyhow::bail!("No epic plan found for plan '{ts}'");
    };

    Ok((ts, epic_plan))
}

fn render_epic_plan_terminal(epic_plan: &EpicPlan) -> Result<String> {
    let mut rendered = String::new();
    rendered.push_str(&format!("Epic: {}\n", epic_plan.epic.name));
    rendered.push_str(&format!("Branch prefix: {}\n", epic_plan.epic.prefix));
    if !epic_plan.epic.summary.is_empty() {
        rendered.push_str(&format!("Summary: {}\n", epic_plan.epic.summary));
    }

    rendered.push_str("\nStories:\n");
    for story in epic_plan.execution_order()? {
        rendered.push_str(&render_story_line(story));
    }

    let graph = epic_plan.to_dependency_graph();
    if graph.node_count() > 0 {
        rendered.push_str("\nDependency DAG:\n");
        rendered.push_str(&graph.to_terminal());
        rendered.push('\n');
    }

    Ok(rendered)
}

fn render_story_line(story: &Story) -> String {
    let dependencies = if story.depends_on.is_empty() {
        "-".to_string()
    } else {
        story.depends_on.join(", ")
    };

    format!(
        "- [{}] {}: {}\n  branch: {}\n  depends_on: {}\n",
        story_status_label(story.status),
        story.id,
        story.summary,
        story.branch,
        dependencies
    )
}

fn story_status_label(status: StoryStatus) -> &'static str {
    match status {
        StoryStatus::Pending => "pending",
        StoryStatus::InProgress => "inprogress",
        StoryStatus::Merged => "merged",
        StoryStatus::Skipped => "skipped",
    }
}

fn truncate_branch(value: &str, max_len: usize) -> String {
    let char_count = value.chars().count();
    if char_count <= max_len {
        value.to_string()
    } else {
        let truncated: String = value.chars().take(max_len - 1).collect();
        format!("{truncated}\u{2026}")
    }
}
