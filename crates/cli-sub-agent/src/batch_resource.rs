fn check_level_resource_availability(
    level: &[String],
    task_map: &HashMap<&str, &BatchTask>,
    resource_guard: &mut Option<ResourceGuard>,
) -> Result<()> {
    let Some(guard) = resource_guard else {
        return Ok(());
    };

    for task_name in level {
        let Some(task) = task_map.get(task_name.as_str()) else {
            continue;
        };
        let tool_name = parse_tool_name(&task.tool)?;
        guard
            .check_availability(tool_name.as_str())
            .with_context(|| format!("task='{}' tool='{}'", task.name, tool_name.as_str()))?;
    }

    Ok(())
}
