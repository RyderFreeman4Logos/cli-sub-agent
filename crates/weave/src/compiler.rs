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
    /// Maximum iterations allowed before forced termination (default: 10).
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
}

fn default_max_iterations() -> u32 {
    10
}

/// A variable declaration collected from the plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VariableDecl {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

/// A non-fatal warning produced during compilation.
#[derive(Debug, Clone, PartialEq)]
pub struct CompileWarning {
    pub message: String,
}

/// Result of compilation: the plan plus any warnings.
#[derive(Debug, Clone)]
pub struct CompileOutput {
    pub plan: ExecutionPlan,
    pub warnings: Vec<CompileWarning>,
}

/// Maximum safe value for `max_iterations`; above this triggers a warning.
const MAX_ITERATIONS_WARN_THRESHOLD: u32 = 50;

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

/// Matches a `MaxIterations: <n>` line at the start of a step body (FOR loops).
static MAXITER_HINT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^MaxIterations:\s*(\d+)\s*$").expect("valid regex"));

/// Matches `${VAR_NAME}` placeholders.
static VAR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}").expect("valid regex"));

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Compile a parsed skill document into an execution plan.
pub fn compile(doc: &SkillDocument) -> Result<ExecutionPlan> {
    let output = compile_with_warnings(doc)?;
    Ok(output.plan)
}

/// Compile a parsed skill document, returning the plan together with any
/// non-fatal warnings (e.g. suspiciously high `max_iterations`).
pub fn compile_with_warnings(doc: &SkillDocument) -> Result<CompileOutput> {
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

    let plan = ExecutionPlan {
        name: doc.meta.name.clone(),
        description: doc.meta.description.clone().unwrap_or_default(),
        variables,
        steps: ctx.steps,
    };

    Ok(CompileOutput {
        plan,
        warnings: ctx.warnings,
    })
}

// ---------------------------------------------------------------------------
// Internal compilation context
// ---------------------------------------------------------------------------

struct CompileCtx {
    steps: Vec<PlanStep>,
    variables: Vec<String>,
    warnings: Vec<CompileWarning>,
    next_id: usize,
    /// Temporary storage for a MaxIterations hint found in a step body,
    /// consumed by `compile_for` when building the loop spec.
    pending_max_iterations: Option<u32>,
}

impl CompileCtx {
    fn new() -> Self {
        Self {
            steps: Vec::new(),
            variables: Vec::new(),
            warnings: Vec::new(),
            next_id: 1,
            pending_max_iterations: None,
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

/// Extracted step hints from the leading lines of a step body.
struct StepHints {
    tool: Option<String>,
    tier: Option<String>,
    on_fail: FailAction,
    max_iterations: Option<u32>,
    prompt: String,
}

/// Extract metadata hints (Tool, Tier, OnFail, MaxIterations) from the first
/// lines of a step body and return the remaining prompt text.
fn extract_hints(body: &str) -> StepHints {
    let mut tool = None;
    let mut tier = None;
    let mut on_fail = FailAction::Abort;
    let mut max_iterations = None;
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
            if let Some(caps) = MAXITER_HINT_RE.captures(line) {
                max_iterations = caps[1].parse().ok();
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
    StepHints {
        tool,
        tier,
        on_fail,
        max_iterations,
        prompt,
    }
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
    let hints = extract_hints(body);

    for var in variables {
        ctx.variables.push(var.clone());
    }

    // Stash max_iterations hint — will be applied to the LoopSpec by
    // compile_for if this step ends up inside a FOR block.
    ctx.pending_max_iterations = hints.max_iterations;

    ctx.steps.push(PlanStep {
        id,
        title: title.to_string(),
        tool: hints.tool,
        prompt: hints.prompt,
        tier: hints.tier,
        depends_on: Vec::new(),
        on_fail: hints.on_fail,
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
        step.condition = Some(conjoin_condition(step.condition.as_deref(), condition));
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
            step.condition = Some(conjoin_condition(step.condition.as_deref(), &negated));
        }
    }

    Ok(())
}

fn conjoin_condition(existing: Option<&str>, new_condition: &str) -> String {
    match existing {
        Some(prev) => format!("({new_condition}) && ({prev})"),
        None => new_condition.to_string(),
    }
}

fn compile_for(
    variable: &str,
    collection: &str,
    body: &[Block],
    ctx: &mut CompileCtx,
) -> Result<()> {
    ctx.collect_vars(collection);

    // Reset pending max_iterations before compiling body steps so we can
    // detect if any body step set it via a `MaxIterations:` hint.
    ctx.pending_max_iterations = None;

    let for_start = ctx.steps.len();
    compile_blocks(body, ctx)?;
    let for_end = ctx.steps.len();

    if for_start == for_end {
        bail!("FOR block over `{collection}` has no compilable steps");
    }

    // Determine max_iterations: use hint from body step, or default.
    let max_iterations = ctx
        .pending_max_iterations
        .take()
        .unwrap_or(default_max_iterations());

    // Validate max_iterations.
    if max_iterations == 0 {
        bail!("FOR block over `{collection}`: max_iterations must be >= 1, got 0");
    }
    if max_iterations > MAX_ITERATIONS_WARN_THRESHOLD {
        ctx.warnings.push(CompileWarning {
            message: format!(
                "FOR block over `{collection}`: max_iterations={max_iterations} exceeds \
                 recommended threshold of {MAX_ITERATIONS_WARN_THRESHOLD} — possible misconfiguration"
            ),
        });
    }

    let loop_spec = LoopSpec {
        variable: variable.to_string(),
        collection: collection.to_string(),
        max_iterations,
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
