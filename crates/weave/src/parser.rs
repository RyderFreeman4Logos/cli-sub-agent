//! Weave skill-lang Markdown parser.
//!
//! Parses `SKILL.md` / `PATTERN.md` files containing TOML frontmatter
//! and a structured Markdown body with steps, control flow, and variable
//! placeholders.

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::Deserialize;
use std::sync::LazyLock;

// ---------------------------------------------------------------------------
// AST types
// ---------------------------------------------------------------------------

/// Parsed representation of a complete skill document.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillDocument {
    pub meta: SkillMeta,
    pub config: Option<SkillConfig>,
    pub body: Vec<Block>,
}

/// Frontmatter metadata extracted from `---` delimiters.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SkillMeta {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub allowed_tools: Option<String>,
    #[serde(default)]
    pub tier: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
}

/// Configuration from a `.skill.toml` sidecar file.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SkillConfig {
    pub skill: SkillConfigMeta,
    #[serde(default)]
    pub agent: Option<AgentConfig>,
}

/// `[skill]` section of `.skill.toml`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SkillConfigMeta {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
}

/// `[agent]` section of `.skill.toml`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AgentConfig {
    #[serde(default)]
    pub skip_context: Vec<String>,
    #[serde(default)]
    pub extra_context: Vec<String>,
    #[serde(default)]
    pub tier: Option<String>,
    #[serde(default)]
    pub max_turns: Option<u32>,
    #[serde(default)]
    pub token_budget: Option<u64>,
    #[serde(default)]
    pub tools: Vec<ToolEntry>,
}

/// `[[agent.tools]]` entry.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ToolEntry {
    pub tool: String,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub thinking_budget: Option<String>,
}

/// A block in the skill body AST.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Step {
        title: String,
        body: String,
        variables: Vec<String>,
    },
    If {
        condition: String,
        then_blocks: Vec<Block>,
        else_blocks: Vec<Block>,
    },
    For {
        variable: String,
        collection: String,
        body: Vec<Block>,
    },
    Include {
        path: String,
    },
    RawMarkdown(String),
}

// ---------------------------------------------------------------------------
// Regex patterns
// ---------------------------------------------------------------------------

static VAR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}").expect("valid regex"));

static STEP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^##\s+(?:Step\s+\d+:\s*)?(.+)$").expect("valid regex"));

static IF_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^##\s+IF\s+(.+)$").expect("valid regex"));

static ELSE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^##\s+ELSE\s*$").expect("valid regex"));

static ENDIF_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^##\s+ENDIF\s*$").expect("valid regex"));

static FOR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^##\s+FOR\s+(\w+)\s+IN\s+(.+)$").expect("valid regex"));

static ENDFOR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^##\s+ENDFOR\s*$").expect("valid regex"));

static INCLUDE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^##\s+INCLUDE\s+(.+)$").expect("valid regex"));

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse a `.skill.toml` sidecar file.
pub fn parse_skill_config(toml_content: &str) -> Result<SkillConfig> {
    toml::from_str(toml_content).context("failed to parse .skill.toml")
}

/// Parse a skill-lang Markdown document (SKILL.md / PATTERN.md).
pub fn parse_skill(content: &str) -> Result<SkillDocument> {
    let (meta, body_str) = parse_frontmatter(content)?;
    let body = parse_body(body_str)?;
    Ok(SkillDocument {
        meta,
        config: None,
        body,
    })
}

// ---------------------------------------------------------------------------
// Frontmatter
// ---------------------------------------------------------------------------

/// Split frontmatter from body and parse the TOML metadata.
fn parse_frontmatter(content: &str) -> Result<(SkillMeta, &str)> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        bail!("missing frontmatter: file must start with `---`");
    }

    // Find closing `---`
    let after_first = &trimmed[3..];
    let close_idx = after_first
        .find("\n---")
        .context("unclosed frontmatter: missing closing `---`")?;

    let fm_text = &after_first[..close_idx];
    let rest_start = close_idx + 4; // skip "\n---"
    let body = if rest_start < after_first.len() {
        after_first[rest_start..].trim_start_matches('\n')
    } else {
        ""
    };

    let meta: SkillMeta = toml::from_str(fm_text).context("failed to parse frontmatter TOML")?;

    Ok((meta, body))
}

// ---------------------------------------------------------------------------
// Body parsing
// ---------------------------------------------------------------------------

/// Intermediate line classification.
#[derive(Debug)]
enum LineKind<'a> {
    If(&'a str),
    Else,
    EndIf,
    For { var: &'a str, collection: &'a str },
    EndFor,
    Include(&'a str),
    Step(&'a str),
    Text(&'a str),
}

/// Helper to extract a numbered capture group, returning `Text` fallback on
/// missing group (should never happen with correct regexes but avoids panic).
fn cap_str<'a>(caps: &regex::Captures<'a>, group: usize) -> Option<&'a str> {
    caps.get(group).map(|m| m.as_str())
}

fn classify_line(line: &str) -> LineKind<'_> {
    let trimmed = line.trim_end();

    if let Some(caps) = IF_RE.captures(trimmed) {
        if let Some(cond) = cap_str(&caps, 1) {
            return LineKind::If(cond);
        }
    }
    if ELSE_RE.is_match(trimmed) {
        return LineKind::Else;
    }
    if ENDIF_RE.is_match(trimmed) {
        return LineKind::EndIf;
    }
    if let Some(caps) = FOR_RE.captures(trimmed) {
        if let (Some(var), Some(col)) = (cap_str(&caps, 1), cap_str(&caps, 2)) {
            return LineKind::For {
                var,
                collection: col,
            };
        }
    }
    if ENDFOR_RE.is_match(trimmed) {
        return LineKind::EndFor;
    }
    if let Some(caps) = INCLUDE_RE.captures(trimmed) {
        if let Some(path) = cap_str(&caps, 1) {
            return LineKind::Include(path);
        }
    }
    if let Some(caps) = STEP_RE.captures(trimmed) {
        if let Some(title) = cap_str(&caps, 1) {
            return LineKind::Step(title);
        }
    }

    LineKind::Text(line)
}

/// Extract unique `${VAR}` placeholders from text.
fn extract_variables(text: &str) -> Vec<String> {
    let mut vars: Vec<String> = VAR_RE
        .captures_iter(text)
        .map(|c| c[1].to_string())
        .collect();
    vars.sort();
    vars.dedup();
    vars
}

/// Maximum nesting depth for IF/FOR blocks to prevent stack overflow on
/// adversarial input.
const MAX_NESTING_DEPTH: usize = 64;

/// Parse body lines into blocks. Operates recursively for nested structures.
fn parse_body(body: &str) -> Result<Vec<Block>> {
    let lines: Vec<&str> = body.lines().collect();
    let (blocks, remaining) = parse_blocks(&lines, 0, &[], 0)?;
    if !remaining.is_empty() {
        bail!(
            "unexpected closing directive: `{}`",
            remaining.first().unwrap_or(&"")
        );
    }
    Ok(blocks)
}

/// Terminal tokens that cause `parse_blocks` to return control to its caller.
const STOP_ELSE: &str = "ELSE";
const STOP_ENDIF: &str = "ENDIF";
const STOP_ENDFOR: &str = "ENDFOR";

/// Parse a sequence of blocks, stopping when a line matches one of `stop_on`.
///
/// Returns the parsed blocks and the remaining (unconsumed) lines starting
/// from the stop token line. `depth` tracks nesting to prevent stack overflow.
fn parse_blocks<'a>(
    lines: &[&'a str],
    mut pos: usize,
    stop_on: &[&str],
    depth: usize,
) -> Result<(Vec<Block>, Vec<&'a str>)> {
    if depth > MAX_NESTING_DEPTH {
        bail!(
            "nesting depth exceeds maximum ({MAX_NESTING_DEPTH}): \
             too many nested IF/FOR blocks"
        );
    }
    let mut blocks: Vec<Block> = Vec::new();
    let mut raw_buf = String::new();

    while pos < lines.len() {
        let line = lines[pos];
        let kind = classify_line(line);

        // Check for stop tokens.
        let is_stop = match &kind {
            LineKind::Else => stop_on.contains(&STOP_ELSE),
            LineKind::EndIf => stop_on.contains(&STOP_ENDIF),
            LineKind::EndFor => stop_on.contains(&STOP_ENDFOR),
            _ => false,
        };
        if is_stop {
            flush_raw(&mut raw_buf, &mut blocks);
            let remaining: Vec<&str> = lines[pos..].to_vec();
            return Ok((blocks, remaining));
        }

        match kind {
            LineKind::If(condition) => {
                flush_raw(&mut raw_buf, &mut blocks);
                pos += 1;

                // Parse then-branch (stops at ELSE or ENDIF).
                let (then_blocks, rest) =
                    parse_blocks(lines, pos, &[STOP_ELSE, STOP_ENDIF], depth + 1)?;

                let (else_blocks, rest2) =
                    if rest.first().is_some_and(|l| ELSE_RE.is_match(l.trim_end())) {
                        // Skip ELSE line, parse else-branch until ENDIF.
                        let else_start = lines.len() - rest.len() + 1;
                        parse_blocks(lines, else_start, &[STOP_ENDIF], depth + 1)?
                    } else {
                        (Vec::new(), rest)
                    };

                // Consume ENDIF.
                if rest2
                    .first()
                    .is_some_and(|l| ENDIF_RE.is_match(l.trim_end()))
                {
                    pos = lines.len() - rest2.len() + 1;
                } else {
                    bail!("unclosed IF block: missing ## ENDIF");
                }

                blocks.push(Block::If {
                    condition: condition.to_string(),
                    then_blocks,
                    else_blocks,
                });
                continue;
            }

            LineKind::For { var, collection } => {
                flush_raw(&mut raw_buf, &mut blocks);
                pos += 1;

                let (body_blocks, rest) = parse_blocks(lines, pos, &[STOP_ENDFOR], depth + 1)?;

                if rest
                    .first()
                    .is_some_and(|l| ENDFOR_RE.is_match(l.trim_end()))
                {
                    pos = lines.len() - rest.len() + 1;
                } else {
                    bail!("unclosed FOR block: missing ## ENDFOR");
                }

                blocks.push(Block::For {
                    variable: var.to_string(),
                    collection: collection.to_string(),
                    body: body_blocks,
                });
                continue;
            }

            LineKind::Include(path) => {
                flush_raw(&mut raw_buf, &mut blocks);
                blocks.push(Block::Include {
                    path: path.trim().to_string(),
                });
            }

            LineKind::Step(title) => {
                flush_raw(&mut raw_buf, &mut blocks);
                let title = title.to_string();
                pos += 1;

                // Collect body until next ## header or EOF.
                let mut step_body = String::new();
                while pos < lines.len() {
                    let next = lines[pos];
                    let next_trimmed = next.trim_end();
                    if next_trimmed.starts_with("## ") {
                        break;
                    }
                    if !step_body.is_empty() {
                        step_body.push('\n');
                    }
                    step_body.push_str(next);
                    pos += 1;
                }

                // Trim leading/trailing blank lines from step body.
                let step_body = step_body.trim().to_string();
                let variables = extract_variables(&step_body);

                blocks.push(Block::Step {
                    title,
                    body: step_body,
                    variables,
                });
                continue;
            }

            LineKind::Else => {
                bail!("unexpected ## ELSE without matching ## IF");
            }
            LineKind::EndIf => {
                bail!("unexpected ## ENDIF without matching ## IF");
            }
            LineKind::EndFor => {
                bail!("unexpected ## ENDFOR without matching ## FOR");
            }

            LineKind::Text(t) => {
                if !raw_buf.is_empty() {
                    raw_buf.push('\n');
                }
                raw_buf.push_str(t);
            }
        }

        pos += 1;
    }

    flush_raw(&mut raw_buf, &mut blocks);
    Ok((blocks, Vec::new()))
}

/// Flush accumulated raw text into a `RawMarkdown` block if non-empty.
fn flush_raw(buf: &mut String, blocks: &mut Vec<Block>) {
    let trimmed = buf.trim();
    if !trimmed.is_empty() {
        blocks.push(Block::RawMarkdown(trimmed.to_string()));
    }
    buf.clear();
}

#[cfg(test)]
#[path = "parser_tests.rs"]
mod tests;
