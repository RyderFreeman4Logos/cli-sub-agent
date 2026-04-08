use super::*;
use crate::parser::parse_skill;

// ---------------------------------------------------------------------------
// Literal strings for bash prompts (regression: backslashes)
// ---------------------------------------------------------------------------

#[test]
fn test_bash_prompt_uses_literal_string() {
    let input = r#"---
name = "literal-test"
---
## Run Build
Tool: bash
cargo build --release 2>&1 | grep -E "error\[E"
"#;
    let doc = parse_skill(input).unwrap();
    let plan = compile(&doc).unwrap();

    let toml_str = plan_to_toml(&plan).unwrap();

    // The serialized TOML must use literal strings (''') for the bash prompt
    // so that backslashes are not interpreted as escape sequences.
    assert!(
        toml_str.contains("'''"),
        "bash prompt should use literal strings, got:\n{toml_str}"
    );
    // Must NOT contain escaped backslash sequences like \\[
    assert!(
        !toml_str.contains(r#"\\["#),
        "bash prompt should not double-escape backslashes, got:\n{toml_str}"
    );

    // Round-trip: the deserialized prompt must match the original.
    let restored = plan_from_toml(&toml_str).unwrap();
    assert_eq!(plan.steps[0].prompt, restored.steps[0].prompt);
}

#[test]
fn test_non_bash_prompt_without_backslash_unchanged() {
    let input = r#"---
name = "no-literal"
---
## Review Code
Tool: claude-code
Review the code for issues.
"#;
    let doc = parse_skill(input).unwrap();
    let plan = compile(&doc).unwrap();

    let toml_str = plan_to_toml(&plan).unwrap();

    // Non-bash prompts without backslashes should NOT use literal strings.
    assert!(
        !toml_str.contains("'''"),
        "non-bash prompt without backslash should use basic string, got:\n{toml_str}"
    );

    let restored = plan_from_toml(&toml_str).unwrap();
    assert_eq!(plan.steps[0].prompt, restored.steps[0].prompt);
}

#[test]
fn test_prompt_with_triple_quote_fallback() {
    // If a prompt contains ''', it cannot be a literal string.
    let plan = ExecutionPlan {
        name: "fallback-test".into(),
        description: String::new(),
        variables: vec![],
        steps: vec![PlanStep {
            id: 1,
            title: "Tricky".into(),
            tool: Some("bash".into()),
            prompt: "echo '''hello'''".into(),
            tier: None,
            depends_on: vec![],
            on_fail: FailAction::Abort,
            condition: None,
            loop_var: None,
            session: None,
        }],
    };

    let toml_str = plan_to_toml(&plan).unwrap();
    // Should fall back to basic string (no literal ''').
    // The prompt should still round-trip correctly.
    let restored = plan_from_toml(&toml_str).unwrap();
    assert_eq!(plan.steps[0].prompt, restored.steps[0].prompt);
}

#[test]
fn test_backslash_in_non_bash_prompt_uses_literal() {
    let input = r#"---
name = "backslash-nonbash"
---
## Analyze
Tool: claude-code
Find files matching pattern: src\main\java
"#;
    let doc = parse_skill(input).unwrap();
    let plan = compile(&doc).unwrap();

    let toml_str = plan_to_toml(&plan).unwrap();

    // Even non-bash prompts with backslashes should get literal strings.
    assert!(
        toml_str.contains("'''"),
        "prompt with backslash should use literal string, got:\n{toml_str}"
    );

    let restored = plan_from_toml(&toml_str).unwrap();
    assert_eq!(plan.steps[0].prompt, restored.steps[0].prompt);
}
