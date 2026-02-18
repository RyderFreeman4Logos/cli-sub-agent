use super::*;
use crate::parser::parse_skill;

// ---------------------------------------------------------------------------
// Empty document
// ---------------------------------------------------------------------------

#[test]
fn test_compile_empty_document() {
    let input = "---\nname = \"empty\"\n---\n";
    let doc = parse_skill(input).unwrap();
    let plan = compile(&doc).unwrap();

    assert_eq!(plan.name, "empty");
    assert!(plan.steps.is_empty());
    assert!(plan.variables.is_empty());
}

// ---------------------------------------------------------------------------
// Single step
// ---------------------------------------------------------------------------

#[test]
fn test_compile_single_step() {
    let input = r#"---
name = "single"
description = "A single step"
---
## Greet
Hello world.
"#;
    let doc = parse_skill(input).unwrap();
    let plan = compile(&doc).unwrap();

    assert_eq!(plan.name, "single");
    assert_eq!(plan.description, "A single step");
    assert_eq!(plan.steps.len(), 1);

    let step = &plan.steps[0];
    assert_eq!(step.id, 1);
    assert_eq!(step.title, "Greet");
    assert_eq!(step.prompt, "Hello world.");
    assert!(step.tool.is_none());
    assert!(step.condition.is_none());
    assert!(step.loop_var.is_none());
    assert_eq!(step.on_fail, FailAction::Abort);
}

// ---------------------------------------------------------------------------
// Tool hint extraction
// ---------------------------------------------------------------------------

#[test]
fn test_compile_step_with_tool_hint() {
    let input = r#"---
name = "tooled"
---
## Build
Tool: codex
Build the project with cargo.
"#;
    let doc = parse_skill(input).unwrap();
    let plan = compile(&doc).unwrap();

    assert_eq!(plan.steps.len(), 1);
    let step = &plan.steps[0];
    assert_eq!(step.tool.as_deref(), Some("codex"));
    assert_eq!(step.prompt, "Build the project with cargo.");
}

#[test]
fn test_compile_step_with_tier_hint() {
    let input = r#"---
name = "tiered"
---
## Analyze
Tier: tier-3-complex
Tool: claude-code
Deep analysis of the codebase.
"#;
    let doc = parse_skill(input).unwrap();
    let plan = compile(&doc).unwrap();

    let step = &plan.steps[0];
    assert_eq!(step.tier.as_deref(), Some("tier-3-complex"));
    assert_eq!(step.tool.as_deref(), Some("claude-code"));
    assert_eq!(step.prompt, "Deep analysis of the codebase.");
}

#[test]
fn test_compile_step_with_onfail_skip() {
    let input = r#"---
name = "resilient"
---
## Optional Step
OnFail: skip
Try this but it's okay if it fails.
"#;
    let doc = parse_skill(input).unwrap();
    let plan = compile(&doc).unwrap();

    assert_eq!(plan.steps[0].on_fail, FailAction::Skip);
}

#[test]
fn test_compile_step_with_onfail_retry() {
    let input = r#"---
name = "retry-test"
---
## Flaky Step
OnFail: retry 5
Might need retries.
"#;
    let doc = parse_skill(input).unwrap();
    let plan = compile(&doc).unwrap();

    assert_eq!(plan.steps[0].on_fail, FailAction::Retry(5));
}

// ---------------------------------------------------------------------------
// IF / ELSE → conditional steps
// ---------------------------------------------------------------------------

#[test]
fn test_compile_if_else() {
    let input = r#"---
name = "conditional"
---
## IF has_tests
## Run Tests
Run the test suite.
## ELSE
## Skip Tests
No tests available.
## ENDIF
"#;
    let doc = parse_skill(input).unwrap();
    let plan = compile(&doc).unwrap();

    assert_eq!(plan.steps.len(), 2);

    let then_step = &plan.steps[0];
    assert_eq!(then_step.title, "Run Tests");
    assert_eq!(then_step.condition.as_deref(), Some("has_tests"));

    let else_step = &plan.steps[1];
    assert_eq!(else_step.title, "Skip Tests");
    assert_eq!(else_step.condition.as_deref(), Some("!(has_tests)"));
}

#[test]
fn test_compile_if_without_else() {
    let input = r#"---
name = "if-only"
---
## IF debug_mode
## Debug Output
Print debug info.
## ENDIF
"#;
    let doc = parse_skill(input).unwrap();
    let plan = compile(&doc).unwrap();

    assert_eq!(plan.steps.len(), 1);
    let step = &plan.steps[0];
    assert_eq!(step.condition.as_deref(), Some("debug_mode"));
}

#[test]
fn test_compile_nested_if_in_else_conjoins_parent_condition() {
    let input = r#"---
name = "nested-else-if"
---
## IF ${USER_APPROVES}
## Apply Plan
Proceed with implementation.
## ELSE
## IF ${USER_MODIFIES}
## Resume with Feedback
Resume with revised input.
## ELSE
## Abandon Plan
Stop and ask for new direction.
## ENDIF
## ENDIF
"#;
    let doc = parse_skill(input).unwrap();
    let plan = compile(&doc).unwrap();

    assert_eq!(plan.steps.len(), 3);
    assert_eq!(plan.steps[0].condition.as_deref(), Some("${USER_APPROVES}"));
    assert_eq!(
        plan.steps[1].condition.as_deref(),
        Some("(!(${USER_APPROVES})) && (${USER_MODIFIES})")
    );
    assert_eq!(
        plan.steps[2].condition.as_deref(),
        Some("(!(${USER_APPROVES})) && (!(${USER_MODIFIES}))")
    );
}

#[test]
fn test_compile_if_inside_for_preserves_outer_gate() {
    let input = r#"---
name = "loop-conditional"
---
## IF !(${BOT_UNAVAILABLE})
## FOR comment IN ${BOT_COMMENTS}
## IF ${COMMENT_IS_FALSE_POSITIVE}
## Arbitrate via Debate
Run independent arbitration.
## ELSE
## Fix Real Issue
Apply a real fix.
## ENDIF
## ENDFOR
## ENDIF
"#;
    let doc = parse_skill(input).unwrap();
    let plan = compile(&doc).unwrap();

    assert_eq!(plan.steps.len(), 2);
    assert_eq!(
        plan.steps[0].condition.as_deref(),
        Some("(!(${BOT_UNAVAILABLE})) && (${COMMENT_IS_FALSE_POSITIVE})")
    );
    assert_eq!(
        plan.steps[1].condition.as_deref(),
        Some("(!(${BOT_UNAVAILABLE})) && (!(${COMMENT_IS_FALSE_POSITIVE}))")
    );
    assert_eq!(plan.steps[0].loop_var.as_ref().unwrap().variable, "comment");
    assert_eq!(
        plan.steps[0].loop_var.as_ref().unwrap().collection,
        "${BOT_COMMENTS}"
    );
    assert_eq!(plan.steps[1].loop_var.as_ref().unwrap().variable, "comment");
    assert_eq!(
        plan.steps[1].loop_var.as_ref().unwrap().collection,
        "${BOT_COMMENTS}"
    );
}

// ---------------------------------------------------------------------------
// FOR → loop steps
// ---------------------------------------------------------------------------

#[test]
fn test_compile_for_loop() {
    let input = r#"---
name = "loop"
---
## FOR file IN source_files
## Process
Handle ${file}.
## ENDFOR
"#;
    let doc = parse_skill(input).unwrap();
    let plan = compile(&doc).unwrap();

    assert_eq!(plan.steps.len(), 1);
    let step = &plan.steps[0];
    assert_eq!(step.title, "Process");
    assert!(step.loop_var.is_some());
    let lv = step.loop_var.as_ref().unwrap();
    assert_eq!(lv.variable, "file");
    assert_eq!(lv.collection, "source_files");
}

// ---------------------------------------------------------------------------
// INCLUDE → weave sub-step
// ---------------------------------------------------------------------------

#[test]
fn test_compile_include() {
    let input = r#"---
name = "composed"
---
## INCLUDE shared/setup.md
## Main Work
Do the main thing.
## INCLUDE shared/teardown.md
"#;
    let doc = parse_skill(input).unwrap();
    let plan = compile(&doc).unwrap();

    assert_eq!(plan.steps.len(), 3);

    assert_eq!(plan.steps[0].tool.as_deref(), Some("weave"));
    assert_eq!(plan.steps[0].prompt, "shared/setup.md");
    assert_eq!(plan.steps[0].title, "Include shared/setup.md");

    assert_eq!(plan.steps[1].title, "Main Work");

    assert_eq!(plan.steps[2].tool.as_deref(), Some("weave"));
    assert_eq!(plan.steps[2].prompt, "shared/teardown.md");
}

// ---------------------------------------------------------------------------
// Variable collection
// ---------------------------------------------------------------------------

#[test]
fn test_variable_collection_across_steps() {
    let input = r#"---
name = "vars"
---
## Deploy
Deploy ${APP_NAME} to ${ENVIRONMENT}.

## Verify
Check ${APP_NAME} on ${ENDPOINT}.
"#;
    let doc = parse_skill(input).unwrap();
    let plan = compile(&doc).unwrap();

    let var_names: Vec<&str> = plan.variables.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(var_names, vec!["APP_NAME", "ENDPOINT", "ENVIRONMENT"]);
}

#[test]
fn test_variable_deduplication() {
    let input = r#"---
name = "dedup"
---
## Step A
Use ${VAR} here.

## Step B
Use ${VAR} again and ${VAR} once more.
"#;
    let doc = parse_skill(input).unwrap();
    let plan = compile(&doc).unwrap();

    assert_eq!(plan.variables.len(), 1);
    assert_eq!(plan.variables[0].name, "VAR");
}

// ---------------------------------------------------------------------------
// Sequential IDs
// ---------------------------------------------------------------------------

#[test]
fn test_sequential_step_ids() {
    let input = r#"---
name = "multi"
---
## Step 1: First
Do first.

## Step 2: Second
Do second.

## Step 3: Third
Do third.
"#;
    let doc = parse_skill(input).unwrap();
    let plan = compile(&doc).unwrap();

    let ids: Vec<usize> = plan.steps.iter().map(|s| s.id).collect();
    assert_eq!(ids, vec![1, 2, 3]);
}

// ---------------------------------------------------------------------------
// Raw markdown is skipped
// ---------------------------------------------------------------------------

#[test]
fn test_raw_markdown_ignored() {
    let input = r#"---
name = "raw"
---
Some intro text that is not a step.

## Actual Step
Do something.
"#;
    let doc = parse_skill(input).unwrap();
    let plan = compile(&doc).unwrap();

    assert_eq!(plan.steps.len(), 1);
    assert_eq!(plan.steps[0].title, "Actual Step");
}

// ---------------------------------------------------------------------------
// TOML round-trip
// ---------------------------------------------------------------------------

#[test]
fn test_toml_round_trip() {
    let input = r#"---
name = "roundtrip"
description = "Test TOML serialization"
---
## INCLUDE shared/setup.md

## Build
Tool: codex
OnFail: retry 3
Build ${PROJECT} with cargo.

## IF has_tests
## Test
Tool: claude-code
Tier: tier-2-standard
Run tests for ${PROJECT}.
## ENDIF

## FOR mod IN modules
## Lint Module
Tool: codex
Lint ${mod}.
## ENDFOR
"#;
    let doc = parse_skill(input).unwrap();
    let plan = compile(&doc).unwrap();

    // Serialize to TOML.
    let toml_str = plan_to_toml(&plan).unwrap();

    // Deserialize back.
    let restored = plan_from_toml(&toml_str).unwrap();

    assert_eq!(plan.name, restored.name);
    assert_eq!(plan.description, restored.description);
    assert_eq!(plan.steps.len(), restored.steps.len());
    assert_eq!(plan.variables.len(), restored.variables.len());

    // Verify key fields survive the round trip.
    for (orig, rest) in plan.steps.iter().zip(restored.steps.iter()) {
        assert_eq!(orig.id, rest.id);
        assert_eq!(orig.title, rest.title);
        assert_eq!(orig.tool, rest.tool);
        assert_eq!(orig.prompt, rest.prompt);
        assert_eq!(orig.condition, rest.condition);
    }
}

// ---------------------------------------------------------------------------
// Full workflow
// ---------------------------------------------------------------------------

#[test]
fn test_compile_full_workflow() {
    let input = r#"---
name = "deploy-pipeline"
description = "Full deployment workflow"
---
## INCLUDE shared/prereqs.md

## Build
Tool: codex
Build the application.

## IF needs_migration
## Migrate
Tool: claude-code
Tier: tier-3-complex
Run database migrations for ${DB_NAME}.
## ELSE
## Skip Migration
No migration needed.
## ENDIF

## FOR svc IN services
## Deploy Service
Tool: codex
OnFail: retry 2
Deploy ${svc} to ${ENVIRONMENT}.
## ENDFOR

## Verify
Check deployment health.
"#;
    let doc = parse_skill(input).unwrap();
    let plan = compile(&doc).unwrap();

    assert_eq!(plan.name, "deploy-pipeline");
    // Steps: include(1) + build(2) + migrate(3) + skip(4) + deploy(5) + verify(6)
    assert_eq!(plan.steps.len(), 6);

    // Include step
    assert_eq!(plan.steps[0].tool.as_deref(), Some("weave"));

    // Build step
    assert_eq!(plan.steps[1].tool.as_deref(), Some("codex"));

    // Conditional then-step
    assert_eq!(plan.steps[2].condition.as_deref(), Some("needs_migration"));
    assert_eq!(plan.steps[2].tier.as_deref(), Some("tier-3-complex"));

    // Conditional else-step
    assert_eq!(
        plan.steps[3].condition.as_deref(),
        Some("!(needs_migration)")
    );

    // Loop step
    assert!(plan.steps[4].loop_var.is_some());
    assert_eq!(plan.steps[4].on_fail, FailAction::Retry(2));

    // Final verify (no tool, no condition)
    assert!(plan.steps[5].tool.is_none());
    assert!(plan.steps[5].condition.is_none());

    // Variables from across all steps
    let var_names: Vec<&str> = plan.variables.iter().map(|v| v.name.as_str()).collect();
    assert!(var_names.contains(&"DB_NAME"));
    assert!(var_names.contains(&"ENVIRONMENT"));
    assert!(var_names.contains(&"svc"));
}

// ---------------------------------------------------------------------------
// Loop safety: max_iterations default
// ---------------------------------------------------------------------------

#[test]
fn test_for_loop_gets_default_max_iterations() {
    let input = r#"---
name = "loop-default"
---
## FOR file IN source_files
## Process
Handle ${file}.
## ENDFOR
"#;
    let doc = parse_skill(input).unwrap();
    let plan = compile(&doc).unwrap();

    assert_eq!(plan.steps.len(), 1);
    let lv = plan.steps[0].loop_var.as_ref().unwrap();
    assert_eq!(lv.max_iterations, 10, "default max_iterations should be 10");
}

// ---------------------------------------------------------------------------
// Loop safety: explicit MaxIterations hint
// ---------------------------------------------------------------------------

#[test]
fn test_for_loop_with_explicit_max_iterations() {
    let input = r#"---
name = "loop-explicit"
---
## FOR file IN source_files
## Process
MaxIterations: 25
Handle ${file}.
## ENDFOR
"#;
    let doc = parse_skill(input).unwrap();
    let plan = compile(&doc).unwrap();

    let lv = plan.steps[0].loop_var.as_ref().unwrap();
    assert_eq!(lv.max_iterations, 25);
}

// ---------------------------------------------------------------------------
// Loop safety: max_iterations > 50 produces warning
// ---------------------------------------------------------------------------

#[test]
fn test_for_loop_high_max_iterations_warns() {
    let input = r#"---
name = "loop-warn"
---
## FOR file IN source_files
## Process
MaxIterations: 100
Handle ${file}.
## ENDFOR
"#;
    let doc = parse_skill(input).unwrap();
    let output = compile_with_warnings(&doc).unwrap();

    assert_eq!(output.warnings.len(), 1);
    assert!(
        output.warnings[0].message.contains("100"),
        "warning should mention the limit: {}",
        output.warnings[0].message
    );
    assert!(output.warnings[0].message.contains("50"));

    // The plan should still compile successfully.
    let lv = output.plan.steps[0].loop_var.as_ref().unwrap();
    assert_eq!(lv.max_iterations, 100);
}

// ---------------------------------------------------------------------------
// Loop safety: max_iterations = 0 is rejected
// ---------------------------------------------------------------------------

#[test]
fn test_for_loop_zero_max_iterations_rejected() {
    let input = r#"---
name = "loop-zero"
---
## FOR file IN source_files
## Process
MaxIterations: 0
Handle ${file}.
## ENDFOR
"#;
    let doc = parse_skill(input).unwrap();
    let result = compile(&doc);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("max_iterations must be >= 1"),
        "error should explain: {err}"
    );
}

// ---------------------------------------------------------------------------
// Loop safety: no warning at threshold boundary (50)
// ---------------------------------------------------------------------------

#[test]
fn test_for_loop_at_threshold_no_warning() {
    let input = r#"---
name = "loop-boundary"
---
## FOR file IN source_files
## Process
MaxIterations: 50
Handle ${file}.
## ENDFOR
"#;
    let doc = parse_skill(input).unwrap();
    let output = compile_with_warnings(&doc).unwrap();

    assert!(
        output.warnings.is_empty(),
        "max_iterations=50 should not trigger a warning"
    );
    let lv = output.plan.steps[0].loop_var.as_ref().unwrap();
    assert_eq!(lv.max_iterations, 50);
}

// ---------------------------------------------------------------------------
// Loop safety: TOML round-trip preserves max_iterations
// ---------------------------------------------------------------------------

#[test]
fn test_max_iterations_toml_round_trip() {
    let input = r#"---
name = "loop-toml"
---
## FOR mod IN modules
## Lint Module
MaxIterations: 20
Lint ${mod}.
## ENDFOR
"#;
    let doc = parse_skill(input).unwrap();
    let plan = compile(&doc).unwrap();

    let toml_str = plan_to_toml(&plan).unwrap();
    let restored = plan_from_toml(&toml_str).unwrap();

    let orig_lv = plan.steps[0].loop_var.as_ref().unwrap();
    let rest_lv = restored.steps[0].loop_var.as_ref().unwrap();
    assert_eq!(orig_lv.max_iterations, rest_lv.max_iterations);
    assert_eq!(rest_lv.max_iterations, 20);
}

// ---------------------------------------------------------------------------
// Loop safety: TOML deserialization defaults max_iterations for legacy plans
// ---------------------------------------------------------------------------

#[test]
fn test_max_iterations_toml_backward_compat() {
    // Simulate a legacy plan.toml that has no max_iterations field.
    let legacy_toml = r#"
[plan]
name = "legacy"

[[plan.steps]]
id = 1
title = "Process"
prompt = "Do something"

[plan.steps.loop_var]
variable = "item"
collection = "items"
"#;
    let plan = plan_from_toml(legacy_toml).unwrap();
    let lv = plan.steps[0].loop_var.as_ref().unwrap();
    assert_eq!(
        lv.max_iterations, 10,
        "legacy plans without max_iterations should default to 10"
    );
}

// ---------------------------------------------------------------------------
// LoopTermination enum variants
// ---------------------------------------------------------------------------

#[test]
fn test_loop_termination_variants() {
    use super::LoopTermination;

    let converged = LoopTermination::Converged { iterations: 3 };
    let max_reached = LoopTermination::MaxIterationsReached { limit: 10 };
    let success = LoopTermination::SuccessCondition { iterations: 5 };
    let exhausted = LoopTermination::CollectionExhausted { iterations: 7 };

    // Verify each variant is distinguishable.
    assert_ne!(converged, max_reached);
    assert_ne!(success, exhausted);

    // Verify serde round-trip.
    let json = serde_json::to_string(&converged).unwrap();
    let restored: LoopTermination = serde_json::from_str(&json).unwrap();
    assert_eq!(converged, restored);

    let json = serde_json::to_string(&max_reached).unwrap();
    let restored: LoopTermination = serde_json::from_str(&json).unwrap();
    assert_eq!(max_reached, restored);
}
