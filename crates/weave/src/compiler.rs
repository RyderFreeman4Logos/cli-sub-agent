//! Weave skill-lang compiler.
//!
//! Transforms a parsed [`SkillDocument`] AST into an [`ExecutionPlan`] that
//! can be serialized to TOML for inspection or consumed by a runtime.

use anyhow::{Result, bail};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

use crate::parser::{Block, SkillDocument};

// ---------------------------------------------------------------------------
// Plan types
// ---------------------------------------------------------------------------

/// An executable plan produced by compiling a skill document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionPlan {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variables: Vec<VariableDecl>,
    pub steps: Vec<PlanStep>,
}

/// A single step in the execution plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: usize,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<usize>,
    #[serde(default)]
    pub on_fail: FailAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_var: Option<LoopSpec>,
}

/// How to handle a step failure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FailAction {
    #[default]
    Abort,
    Retry(u32),
    Skip,
    Delegate(String),
}

/// Loop specification for FOR blocks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoopSpec {
    pub variable: String,
    pub collection: String,
}

/// A variable declaration collected from the plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VariableDecl {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

// ---------------------------------------------------------------------------
// Regex for tool-hint extraction
// ---------------------------------------------------------------------------

/// Matches a `Tool: <name>` line at the start of a step body.
static TOOL_HINT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^Tool:\s*(\S+)\s*$").expect("valid regex"));

/// Matches a `Tier: <name>` line at the start of a step body.
static TIER_HINT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^Tier:\s*(\S+)\s*$").expect("valid regex"));

/// Matches a `OnFail: <action>` line at the start of a step body.
static ONFAIL_HINT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^OnFail:\s*(.+)\s*$").expect("valid regex"));

/// Matches `${VAR_NAME}` placeholders.
static VAR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}").expect("valid regex"));

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Compile a parsed skill document into an execution plan.
pub fn compile(doc: &SkillDocument) -> Result<ExecutionPlan> {
    let mut ctx = CompileCtx::new();
    compile_blocks(&doc.body, &mut ctx)?;

    let mut all_vars: Vec<String> = ctx.variables;
    all_vars.sort();
    all_vars.dedup();

    let variables = all_vars
        .into_iter()
        .map(|name| VariableDecl {
            name,
            default: None,
        })
        .collect();

    Ok(ExecutionPlan {
        name: doc.meta.name.clone(),
        description: doc.meta.description.clone().unwrap_or_default(),
        variables,
        steps: ctx.steps,
    })
}

// ---------------------------------------------------------------------------
// Internal compilation context
// ---------------------------------------------------------------------------

struct CompileCtx {
    steps: Vec<PlanStep>,
    variables: Vec<String>,
    next_id: usize,
}

impl CompileCtx {
    fn new() -> Self {
        Self {
            steps: Vec::new(),
            variables: Vec::new(),
            next_id: 1,
        }
    }

    fn alloc_id(&mut self) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn collect_vars(&mut self, text: &str) {
        for cap in VAR_RE.captures_iter(text) {
            self.variables.push(cap[1].to_string());
        }
    }
}

/// Compile a slice of AST blocks into plan steps.
fn compile_blocks(blocks: &[Block], ctx: &mut CompileCtx) -> Result<()> {
    for block in blocks {
        match block {
            Block::Step {
                title,
                body,
                variables,
            } => {
                compile_step(title, body, variables, ctx)?;
            }
            Block::If {
                condition,
                then_blocks,
                else_blocks,
            } => {
                compile_if(condition, then_blocks, else_blocks, ctx)?;
            }
            Block::For {
                variable,
                collection,
                body,
            } => {
                compile_for(variable, collection, body, ctx)?;
            }
            Block::Include { path } => {
                compile_include(path, ctx);
            }
            Block::RawMarkdown(_) => {
                // Raw markdown is informational; not compiled into steps.
            }
        }
    }
    Ok(())
}

/// Extract metadata hints (Tool, Tier, OnFail) from the first lines of a step
/// body and return the remaining prompt text.
fn extract_hints(body: &str) -> (Option<String>, Option<String>, FailAction, String) {
    let mut tool = None;
    let mut tier = None;
    let mut on_fail = FailAction::Abort;
    let mut prompt_lines = Vec::new();
    let mut in_hints = true;

    for line in body.lines() {
        if in_hints {
            if let Some(caps) = TOOL_HINT_RE.captures(line) {
                tool = Some(caps[1].to_string());
                continue;
            }
            if let Some(caps) = TIER_HINT_RE.captures(line) {
                tier = Some(caps[1].to_string());
                continue;
            }
            if let Some(caps) = ONFAIL_HINT_RE.captures(line) {
                on_fail = parse_fail_action(caps[1].trim());
                continue;
            }
            // First non-hint line ends hint extraction.
            if !line.trim().is_empty() {
                in_hints = false;
            }
        }
        prompt_lines.push(line);
    }

    let prompt = prompt_lines.join("\n").trim().to_string();
    (tool, tier, on_fail, prompt)
}

/// Parse a fail action string.
fn parse_fail_action(s: &str) -> FailAction {
    let lower = s.to_lowercase();
    if lower == "skip" {
        return FailAction::Skip;
    }
    if lower == "abort" {
        return FailAction::Abort;
    }
    if let Some(rest) = lower.strip_prefix("retry") {
        let count: u32 = rest.trim().parse().unwrap_or(3);
        return FailAction::Retry(count);
    }
    if let Some(rest) = lower.strip_prefix("delegate") {
        let target = rest.trim().to_string();
        return FailAction::Delegate(if target.is_empty() {
            "auto".to_string()
        } else {
            target
        });
    }
    FailAction::Abort
}

fn compile_step(title: &str, body: &str, variables: &[String], ctx: &mut CompileCtx) -> Result<()> {
    let id = ctx.alloc_id();
    let (tool, tier, on_fail, prompt) = extract_hints(body);

    for var in variables {
        ctx.variables.push(var.clone());
    }

    ctx.steps.push(PlanStep {
        id,
        title: title.to_string(),
        tool,
        prompt,
        tier,
        depends_on: Vec::new(),
        on_fail,
        condition: None,
        loop_var: None,
    });
    Ok(())
}

fn compile_if(
    condition: &str,
    then_blocks: &[Block],
    else_blocks: &[Block],
    ctx: &mut CompileCtx,
) -> Result<()> {
    ctx.collect_vars(condition);

    // Compile then-branch steps with the condition.
    let then_start = ctx.steps.len();
    compile_blocks(then_blocks, ctx)?;
    let then_end = ctx.steps.len();

    // Tag all then-branch steps with the condition.
    for step in &mut ctx.steps[then_start..then_end] {
        step.condition = Some(condition.to_string());
    }

    if then_start == then_end {
        // Empty then-branch: emit a placeholder step.
        let id = ctx.alloc_id();
        ctx.steps.push(PlanStep {
            id,
            title: format!("(if {condition})"),
            tool: None,
            prompt: String::new(),
            tier: None,
            depends_on: Vec::new(),
            on_fail: FailAction::Skip,
            condition: Some(condition.to_string()),
            loop_var: None,
        });
    }

    // Compile else-branch with negated condition.
    if !else_blocks.is_empty() {
        let negated = format!("!({condition})");
        let else_start = ctx.steps.len();
        compile_blocks(else_blocks, ctx)?;
        let else_end = ctx.steps.len();

        for step in &mut ctx.steps[else_start..else_end] {
            step.condition = Some(negated.clone());
        }
    }

    Ok(())
}

fn compile_for(
    variable: &str,
    collection: &str,
    body: &[Block],
    ctx: &mut CompileCtx,
) -> Result<()> {
    ctx.collect_vars(collection);

    let for_start = ctx.steps.len();
    compile_blocks(body, ctx)?;
    let for_end = ctx.steps.len();

    if for_start == for_end {
        bail!("FOR block over `{collection}` has no compilable steps");
    }

    let loop_spec = LoopSpec {
        variable: variable.to_string(),
        collection: collection.to_string(),
    };

    // Tag all loop body steps with the loop spec.
    for step in &mut ctx.steps[for_start..for_end] {
        step.loop_var = Some(loop_spec.clone());
    }

    Ok(())
}

fn compile_include(path: &str, ctx: &mut CompileCtx) {
    let id = ctx.alloc_id();
    ctx.steps.push(PlanStep {
        id,
        title: format!("Include {path}"),
        tool: Some("weave".to_string()),
        prompt: path.to_string(),
        tier: None,
        depends_on: Vec::new(),
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
    });
}

// ---------------------------------------------------------------------------
// TOML serialization wrapper
// ---------------------------------------------------------------------------

/// Wrapper for TOML serialization of the execution plan.
#[derive(Serialize, Deserialize)]
struct PlanWrapper {
    plan: ExecutionPlan,
}

/// Serialize an execution plan to TOML.
pub fn plan_to_toml(plan: &ExecutionPlan) -> Result<String> {
    let wrapper = PlanWrapper { plan: plan.clone() };
    toml::to_string_pretty(&wrapper).map_err(|e| anyhow::anyhow!("TOML serialization failed: {e}"))
}

/// Deserialize an execution plan from TOML.
pub fn plan_from_toml(toml_str: &str) -> Result<ExecutionPlan> {
    let wrapper: PlanWrapper = toml::from_str(toml_str)
        .map_err(|e| anyhow::anyhow!("TOML deserialization failed: {e}"))?;
    Ok(wrapper.plan)
}

#[cfg(test)]
#[path = "compiler_tests.rs"]
mod tests;
