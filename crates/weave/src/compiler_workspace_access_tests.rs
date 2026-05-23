use super::*;
use crate::parser::{WorkspaceAccess, parse_skill};

#[test]
fn test_plan_from_toml_preserves_step_workspace_access() {
    let input = r#"
[workflow]
name = "readonly-step"

[[workflow.steps]]
id = 1
title = "Recon"
tool = "csa"
workspace_access = "read-only"
prompt = "Inspect only."
"#;

    let plan = plan_from_toml(input).unwrap();

    assert_eq!(
        plan.steps[0].workspace_access,
        Some(WorkspaceAccess::ReadOnly)
    );
}

#[test]
fn test_compile_step_with_workspace_access_hint() {
    let input = r#"---
name = "readonly"
---
## Recon
Tool: csa
Workspace Access: read-only
Inspect the repo without editing.
"#;
    let doc = parse_skill(input).unwrap();
    let plan = compile(&doc).unwrap();

    assert_eq!(
        plan.steps[0].workspace_access,
        Some(WorkspaceAccess::ReadOnly)
    );
    assert_eq!(plan.steps[0].prompt, "Inspect the repo without editing.");
}
