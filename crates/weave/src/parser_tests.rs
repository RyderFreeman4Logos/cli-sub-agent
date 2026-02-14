use super::*;

// -- Frontmatter tests ------------------------------------------------------

#[test]
fn test_parse_minimal_frontmatter() {
    let input = r#"---
name = "my-skill"
---
Some body text.
"#;
    let doc = parse_skill(input).unwrap();
    assert_eq!(doc.meta.name, "my-skill");
    assert!(doc.meta.description.is_none());
    assert!(doc.meta.allowed_tools.is_none());
    assert!(doc.meta.tier.is_none());
}

#[test]
fn test_parse_full_frontmatter() {
    let input = r#"---
name = "security-audit"
description = "Adversarial security analysis"
allowed-tools = "Bash, Read, Grep"
tier = "tier-3-complex"
model = "claude-opus-4-6"
version = "1.0.0"
---
Body here.
"#;
    let doc = parse_skill(input).unwrap();
    assert_eq!(doc.meta.name, "security-audit");
    assert_eq!(
        doc.meta.description.as_deref(),
        Some("Adversarial security analysis")
    );
    assert_eq!(doc.meta.allowed_tools.as_deref(), Some("Bash, Read, Grep"));
    assert_eq!(doc.meta.tier.as_deref(), Some("tier-3-complex"));
    assert_eq!(doc.meta.model.as_deref(), Some("claude-opus-4-6"));
    assert_eq!(doc.meta.version.as_deref(), Some("1.0.0"));
}

#[test]
fn test_error_on_missing_frontmatter() {
    let input = "# No frontmatter\nJust text.";
    let err = parse_skill(input).unwrap_err();
    assert!(
        err.to_string().contains("missing frontmatter"),
        "unexpected error: {err}"
    );
}

#[test]
fn test_error_on_unclosed_frontmatter() {
    let input = "---\nname = \"broken\"\nNo closing delimiter.";
    let err = parse_skill(input).unwrap_err();
    assert!(
        err.to_string().contains("unclosed frontmatter"),
        "unexpected error: {err}"
    );
}

#[test]
fn test_error_on_malformed_frontmatter_toml() {
    let input = "---\nnot valid toml {{{\n---\nBody.";
    let err = parse_skill(input).unwrap_err();
    assert!(
        err.to_string().contains("failed to parse frontmatter"),
        "unexpected error: {err}"
    );
}

// -- .skill.toml tests ------------------------------------------------------

#[test]
fn test_parse_skill_config_full() {
    let toml_str = r#"
[skill]
name = "my-skill"
version = "0.1.0"

[agent]
skip_context = ["CLAUDE.md"]
extra_context = ["rules/security.md"]
tier = "tier-3-complex"
max_turns = 20
token_budget = 150000

[[agent.tools]]
tool = "claude-code"
provider = "anthropic"
model = "claude-opus-4-6"
thinking_budget = "high"
"#;
    let cfg = parse_skill_config(toml_str).unwrap();
    assert_eq!(cfg.skill.name, "my-skill");
    assert_eq!(cfg.skill.version.as_deref(), Some("0.1.0"));

    let agent = cfg.agent.as_ref().unwrap();
    assert_eq!(agent.skip_context, vec!["CLAUDE.md"]);
    assert_eq!(agent.extra_context, vec!["rules/security.md"]);
    assert_eq!(agent.tier.as_deref(), Some("tier-3-complex"));
    assert_eq!(agent.max_turns, Some(20));
    assert_eq!(agent.token_budget, Some(150_000));
    assert_eq!(agent.tools.len(), 1);
    assert_eq!(agent.tools[0].tool, "claude-code");
    assert_eq!(agent.tools[0].provider.as_deref(), Some("anthropic"));
    assert_eq!(agent.tools[0].model.as_deref(), Some("claude-opus-4-6"));
    assert_eq!(agent.tools[0].thinking_budget.as_deref(), Some("high"));
}

#[test]
fn test_parse_skill_config_minimal() {
    let toml_str = r#"
[skill]
name = "tiny"
"#;
    let cfg = parse_skill_config(toml_str).unwrap();
    assert_eq!(cfg.skill.name, "tiny");
    assert!(cfg.agent.is_none());
}

// -- Step parsing -----------------------------------------------------------

#[test]
fn test_parse_steps_from_headers() {
    let input = r#"---
name = "multi-step"
---
## Step 1: Initialize
Set up the environment.

## Step 2: Execute
Run the main process.

## Cleanup
Remove temporary files.
"#;
    let doc = parse_skill(input).unwrap();
    let steps: Vec<&str> = doc
        .body
        .iter()
        .filter_map(|b| match b {
            Block::Step { title, .. } => Some(title.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(steps, vec!["Initialize", "Execute", "Cleanup"]);
}

#[test]
fn test_step_body_content() {
    let input = r#"---
name = "body-test"
---
## Do Something
Line one.
Line two.
"#;
    let doc = parse_skill(input).unwrap();
    match &doc.body[0] {
        Block::Step { body, .. } => {
            assert_eq!(body, "Line one.\nLine two.");
        }
        other => panic!("expected Step, got {other:?}"),
    }
}

// -- Variable extraction ----------------------------------------------------

#[test]
fn test_extract_variables() {
    let input = r#"---
name = "vars"
---
## Deploy
Deploy ${APP_NAME} to ${ENVIRONMENT}.
Use ${APP_NAME} again.
"#;
    let doc = parse_skill(input).unwrap();
    match &doc.body[0] {
        Block::Step { variables, .. } => {
            assert_eq!(variables, &["APP_NAME", "ENVIRONMENT"]);
        }
        other => panic!("expected Step, got {other:?}"),
    }
}

// -- IF / ELSE / ENDIF ------------------------------------------------------

#[test]
fn test_parse_if_else_endif() {
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
    assert_eq!(doc.body.len(), 1);
    match &doc.body[0] {
        Block::If {
            condition,
            then_blocks,
            else_blocks,
        } => {
            assert_eq!(condition, "has_tests");
            assert_eq!(then_blocks.len(), 1);
            assert!(matches!(&then_blocks[0], Block::Step { title, .. } if title == "Run Tests"));
            assert_eq!(else_blocks.len(), 1);
            assert!(matches!(&else_blocks[0], Block::Step { title, .. } if title == "Skip Tests"));
        }
        other => panic!("expected If, got {other:?}"),
    }
}

#[test]
fn test_parse_if_without_else() {
    let input = r#"---
name = "if-only"
---
## IF debug_mode
## Debug Output
Print debug info.
## ENDIF
"#;
    let doc = parse_skill(input).unwrap();
    match &doc.body[0] {
        Block::If {
            else_blocks,
            then_blocks,
            ..
        } => {
            assert_eq!(then_blocks.len(), 1);
            assert!(else_blocks.is_empty());
        }
        other => panic!("expected If, got {other:?}"),
    }
}

#[test]
fn test_error_on_unclosed_if() {
    let input = r#"---
name = "broken-if"
---
## IF some_condition
## Step Inside
Content.
"#;
    let err = parse_skill(input).unwrap_err();
    assert!(
        err.to_string().contains("unclosed IF"),
        "unexpected error: {err}"
    );
}

// -- FOR / ENDFOR -----------------------------------------------------------

#[test]
fn test_parse_for_endfor() {
    let input = r#"---
name = "loop"
---
## FOR file IN source_files
## Process
Handle ${file}.
## ENDFOR
"#;
    let doc = parse_skill(input).unwrap();
    assert_eq!(doc.body.len(), 1);
    match &doc.body[0] {
        Block::For {
            variable,
            collection,
            body,
        } => {
            assert_eq!(variable, "file");
            assert_eq!(collection, "source_files");
            assert_eq!(body.len(), 1);
            assert!(matches!(&body[0], Block::Step { title, .. } if title == "Process"));
        }
        other => panic!("expected For, got {other:?}"),
    }
}

#[test]
fn test_error_on_unclosed_for() {
    let input = r#"---
name = "broken-for"
---
## FOR x IN items
## Work
Do stuff.
"#;
    let err = parse_skill(input).unwrap_err();
    assert!(
        err.to_string().contains("unclosed FOR"),
        "unexpected error: {err}"
    );
}

// -- INCLUDE ----------------------------------------------------------------

#[test]
fn test_parse_include() {
    let input = r#"---
name = "composed"
---
## INCLUDE shared/setup.md
## Main Work
Do the main thing.
## INCLUDE shared/teardown.md
"#;
    let doc = parse_skill(input).unwrap();
    assert_eq!(doc.body.len(), 3);
    assert!(matches!(&doc.body[0], Block::Include { path } if path == "shared/setup.md"));
    assert!(matches!(&doc.body[1], Block::Step { .. }));
    assert!(matches!(&doc.body[2], Block::Include { path } if path == "shared/teardown.md"));
}

// -- Raw Markdown -----------------------------------------------------------

#[test]
fn test_raw_markdown_preserved() {
    let input = r#"---
name = "raw"
---
Some introductory text before any steps.

More text here.

## A Step
Step content.
"#;
    let doc = parse_skill(input).unwrap();
    assert_eq!(doc.body.len(), 2);
    assert!(matches!(&doc.body[0], Block::RawMarkdown(s) if s.contains("introductory")));
    assert!(matches!(&doc.body[1], Block::Step { .. }));
}

// -- Edge cases -------------------------------------------------------------

#[test]
fn test_empty_body() {
    let input = "---\nname = \"empty\"\n---\n";
    let doc = parse_skill(input).unwrap();
    assert!(doc.body.is_empty());
}

#[test]
fn test_frontmatter_only_no_trailing_newline() {
    let input = "---\nname = \"minimal\"\n---";
    let doc = parse_skill(input).unwrap();
    assert_eq!(doc.meta.name, "minimal");
    assert!(doc.body.is_empty());
}

#[test]
fn test_error_on_excessive_nesting_depth() {
    let depth = super::MAX_NESTING_DEPTH + 1;
    let mut input = String::from("---\nname = \"deep\"\n---\n");
    for _ in 0..depth {
        input.push_str("## IF cond\n");
    }
    for _ in 0..depth {
        input.push_str("## ENDIF\n");
    }
    let err = parse_skill(&input).unwrap_err();
    assert!(
        err.to_string().contains("nesting depth exceeds maximum"),
        "unexpected error: {err}"
    );
}

// -- Input size limits ------------------------------------------------------

#[test]
fn test_parse_skill_rejects_oversized_input() {
    let oversized = "x".repeat(super::MAX_INPUT_BYTES + 1);
    let err = parse_skill(&oversized).unwrap_err();
    assert!(
        err.to_string().contains("input exceeds maximum size"),
        "unexpected error: {err}"
    );
}

#[test]
fn test_parse_skill_accepts_input_at_limit() {
    // Build valid content that is exactly MAX_INPUT_BYTES.
    let header = "---\nname = \"big\"\n---\n";
    let padding_len = super::MAX_INPUT_BYTES - header.len();
    let input = format!("{header}{}", "a".repeat(padding_len));
    assert_eq!(input.len(), super::MAX_INPUT_BYTES);
    // Should not fail due to size — may still parse fine (body is raw text).
    assert!(parse_skill(&input).is_ok());
}

#[test]
fn test_parse_skill_config_rejects_oversized_input() {
    let oversized = "x".repeat(super::MAX_INPUT_BYTES + 1);
    let err = parse_skill_config(&oversized).unwrap_err();
    assert!(
        err.to_string().contains("input exceeds maximum size"),
        "unexpected error: {err}"
    );
}

#[test]
fn test_parse_skill_config_accepts_input_at_limit() {
    let toml_str = "[skill]\nname = \"big\"\n";
    let padding_len = super::MAX_INPUT_BYTES - toml_str.len();
    // Pad with TOML-valid whitespace (newlines).
    let input = format!("{toml_str}{}", "\n".repeat(padding_len));
    assert_eq!(input.len(), super::MAX_INPUT_BYTES);
    assert!(parse_skill_config(&input).is_ok());
}
