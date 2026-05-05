use anyhow::{Result, anyhow};
use csa_core::types::{OutputFormat, ToolArg};

pub(crate) const ARCH_PROMPT_PREFIX: &str = r#"ARCHITECTURE MODE - DEEP MODULE ANALYSIS

You are running an Ousterhout-style deep module analysis. Do not implement
production code changes unless the caller explicitly asks for implementation.
Your job is to find places where interfaces are shallow, pass-through, or
misplaced, then help the caller choose a design that increases depth.

GLOSSARY
- Module: anything with an interface and an implementation
- Interface: everything a caller must know (types, invariants, error modes, ordering, config)
- Implementation: the code inside
- Depth: leverage at the interface. Deep = high leverage. Shallow = interface nearly as complex as implementation
- Seam: where an interface lives; behavior can be altered without editing in place
- Adapter: a concrete thing satisfying an interface at a seam
- Leverage: what callers get from depth
- Locality: what maintainers get from depth (change/bugs/knowledge concentrated in one place)

KEY PRINCIPLES
- Deletion test: imagine deleting the module. If complexity vanishes -> pass-through. If it reappears across N callers -> earning its keep
- The interface is the test surface
- One adapter = hypothetical seam. Two adapters = real seam

PROCESS
1. Explore: walk codebase noting friction (bouncing between modules, shallow modules, pure functions extracted just for testability)
2. Present candidates: numbered list with files, problem, solution, benefits (in terms of locality and leverage)
3. Grilling loop: design conversation for selected candidate

EXPLORE GUIDANCE
- Read from call sites inward. The caller-facing obligations define the real interface.
- Track every detail callers must remember: ordering, setup, teardown, retries,
  error interpretation, configuration defaults, mutation timing, and hidden invariants.
- Prefer examples backed by file paths and current behavior over abstract critique.
- Treat scattered tests as evidence about the interface: if every caller repeats the
  same setup or assertion shape, the module may not be carrying enough complexity.
- Do not count a wrapper as deep merely because it has a trait or a named type.

CANDIDATE FORMAT
For each candidate, include:
1. Files:
2. Problem:
3. Current interface burden:
4. Proposed module/interface:
5. Expected locality benefit:
6. Expected leverage benefit:
7. Deletion test result:
8. Adapter count:
9. Risks or tradeoffs:

GRILLING LOOP
- Ask which candidate the caller wants to pursue.
- For the selected candidate, challenge the design before implementation:
  1. What exact caller knowledge disappears?
  2. What invariant moves behind the interface?
  3. What new interface would tests exercise?
  4. Is the seam real now, or only justified by a second adapter soon?
  5. What would make this candidate a pass-through abstraction?
- Keep the loop concrete: propose a narrow next patch only after the interface is clear.

SIDE EFFECTS
- Update CONTEXT.md when naming new concepts that future agents must reuse.
- Offer an ADR when rejecting a candidate for a load-bearing reason.

OUTPUT REQUIREMENTS
- Start with a short summary of the strongest candidate.
- Include a numbered candidate list using the candidate format.
- End with: RECOMMENDED_NEXT_STEP: <specific candidate or no-change rationale>"#;

pub(crate) fn build_arch_prompt(description: &str) -> String {
    format!("{ARCH_PROMPT_PREFIX}\n\nARCHITECTURE ANALYSIS REQUEST:\n{description}")
}

pub(crate) async fn handle_arch(
    description: String,
    tool: Option<String>,
    timeout: u64,
    allow_base_branch_working: bool,
    current_depth: u32,
    output_format: OutputFormat,
) -> Result<i32> {
    let tool = tool
        .map(|raw| {
            raw.parse::<ToolArg>()
                .map_err(|err| anyhow!("invalid arch tool `{raw}`: {err}"))
        })
        .transpose()?;
    let prompt = build_arch_prompt(&description);

    crate::run_cmd::SubagentRunConfig::new(prompt, output_format)
        .tool(tool)
        .timeout(timeout)
        .allow_base_branch_working(allow_base_branch_working)
        .current_depth(current_depth)
        .run()
        .await
}

pub(crate) async fn handle_arch_args(
    args: crate::cli::ArchArgs,
    current_depth: u32,
    output_format: OutputFormat,
) -> Result<i32> {
    handle_arch(
        args.description,
        args.tool,
        args.timeout,
        args.allow_base_branch_working,
        current_depth,
        output_format,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::{ARCH_PROMPT_PREFIX, build_arch_prompt};

    #[test]
    fn arch_prompt_contains_deep_module_vocabulary_and_process() {
        let prompt = build_arch_prompt("Analyze command dispatch depth");

        assert!(prompt.starts_with("ARCHITECTURE MODE - DEEP MODULE ANALYSIS"));
        assert!(
            ARCH_PROMPT_PREFIX
                .contains("- Module: anything with an interface and an implementation")
        );
        assert!(ARCH_PROMPT_PREFIX.contains("- Interface: everything a caller must know (types, invariants, error modes, ordering, config)"));
        assert!(ARCH_PROMPT_PREFIX.contains("- Implementation: the code inside"));
        assert!(ARCH_PROMPT_PREFIX.contains("- Depth: leverage at the interface. Deep = high leverage. Shallow = interface nearly as complex as implementation"));
        assert!(ARCH_PROMPT_PREFIX.contains(
            "- Seam: where an interface lives; behavior can be altered without editing in place"
        ));
        assert!(
            ARCH_PROMPT_PREFIX
                .contains("- Adapter: a concrete thing satisfying an interface at a seam")
        );
        assert!(ARCH_PROMPT_PREFIX.contains("- Leverage: what callers get from depth"));
        assert!(ARCH_PROMPT_PREFIX.contains("- Locality: what maintainers get from depth (change/bugs/knowledge concentrated in one place)"));
        assert!(ARCH_PROMPT_PREFIX.contains("Deletion test: imagine deleting the module"));
        assert!(ARCH_PROMPT_PREFIX.contains("The interface is the test surface"));
        assert!(
            ARCH_PROMPT_PREFIX
                .contains("One adapter = hypothetical seam. Two adapters = real seam")
        );
        assert!(ARCH_PROMPT_PREFIX.contains("Explore: walk codebase noting friction"));
        assert!(ARCH_PROMPT_PREFIX.contains("bouncing between modules"));
        assert!(ARCH_PROMPT_PREFIX.contains("shallow modules"));
        assert!(ARCH_PROMPT_PREFIX.contains("pure functions extracted just for testability"));
        assert!(
            ARCH_PROMPT_PREFIX.contains(
                "Present candidates: numbered list with files, problem, solution, benefits"
            )
        );
        assert!(
            ARCH_PROMPT_PREFIX
                .contains("Grilling loop: design conversation for selected candidate")
        );
        assert!(ARCH_PROMPT_PREFIX.contains("Update CONTEXT.md when naming new concepts"));
        assert!(ARCH_PROMPT_PREFIX.contains("Offer an ADR when rejecting a candidate"));
        assert!(prompt.ends_with("Analyze command dispatch depth"));
    }
}
