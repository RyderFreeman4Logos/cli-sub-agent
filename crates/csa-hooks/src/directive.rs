//! CSA directive parsing from hook/step output.
//!
//! Directives are HTML-comment-style markers embedded in stdout/stderr:
//!
//! - `<!-- CSA:NEXT_STEP step_id=<id> -->` — instruct weave to jump to a step.
//! - `<!-- CSA:NEXT_STEP cmd="<command>" required=true|false -->` — suggest a
//!   follow-up command for orchestrators to chain steps mechanically.
//!
//! Both forms can coexist; the richer `cmd=` form is preferred for pipeline
//! enforcement while `step_id=` is used for intra-workflow jumps.

/// A parsed CSA next-step directive from hook or step output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NextStepDirective {
    /// Intra-workflow step ID to jump to (used by weave executor).
    pub step_id: Option<String>,
    /// Shell command for the orchestrator to run next.
    pub cmd: Option<String>,
    /// Whether this next step is required (pipeline enforcement).
    pub required: bool,
}

/// Parse all `CSA:NEXT_STEP` directives from text output.
///
/// Returns the **last** `NEXT_STEP` directive found (later directives
/// override earlier ones, matching the "last writer wins" convention).
pub fn parse_next_step(output: &str) -> Option<String> {
    parse_next_step_directive(output).and_then(|d| d.step_id)
}

/// Parse a rich `NextStepDirective` from text output.
///
/// Supports both legacy `step_id=<value>` and extended `cmd="..." required=true|false`.
/// Returns the **last** directive found.
pub fn parse_next_step_directive(output: &str) -> Option<NextStepDirective> {
    let mut last: Option<NextStepDirective> = None;

    for line in output.lines() {
        let trimmed = line.trim();
        // Match: <!-- CSA:NEXT_STEP ... -->
        if let Some(rest) = trimmed.strip_prefix("<!-- CSA:NEXT_STEP")
            && let Some(rest) = rest.strip_suffix("-->")
        {
            let rest = rest.trim();
            if rest.is_empty() {
                continue;
            }

            let mut step_id = None;
            let mut cmd = None;
            let mut required = false;

            // Parse key=value pairs from the directive body.
            // Handles both quoted ("value") and unquoted (value) forms.
            let mut remaining = rest;
            while !remaining.is_empty() {
                remaining = remaining.trim_start();
                if remaining.is_empty() {
                    break;
                }
                // Find key
                let eq_pos = match remaining.find('=') {
                    Some(pos) => pos,
                    None => break,
                };
                let key = remaining[..eq_pos].trim();
                remaining = &remaining[eq_pos + 1..];

                // Parse value (quoted or bare)
                let value = if remaining.starts_with('"') {
                    // Quoted value: find closing quote
                    remaining = &remaining[1..];
                    let end = remaining.find('"').unwrap_or(remaining.len());
                    let val = &remaining[..end];
                    remaining = if end < remaining.len() {
                        &remaining[end + 1..]
                    } else {
                        ""
                    };
                    val
                } else {
                    // Bare value: ends at whitespace
                    let end = remaining
                        .find(char::is_whitespace)
                        .unwrap_or(remaining.len());
                    let val = &remaining[..end];
                    remaining = &remaining[end..];
                    val
                };

                match key {
                    "step_id" => {
                        let v = value.trim();
                        if !v.is_empty() {
                            step_id = Some(v.to_string());
                        }
                    }
                    "cmd" => {
                        let v = value.trim();
                        if !v.is_empty() {
                            cmd = Some(v.to_string());
                        }
                    }
                    "required" => {
                        required = value.trim().eq_ignore_ascii_case("true");
                    }
                    _ => {} // Ignore unknown keys for forward-compat
                }
            }

            if step_id.is_some() || cmd.is_some() {
                last = Some(NextStepDirective {
                    step_id,
                    cmd,
                    required,
                });
            }
        }
    }

    last
}

/// Format a `CSA:NEXT_STEP` directive for emission to stderr.
///
/// This is the canonical way for weave steps and hooks to emit next-step
/// directives that orchestrators can parse mechanically.
pub fn format_next_step_directive(cmd: &str, required: bool) -> String {
    format!(
        "<!-- CSA:NEXT_STEP cmd=\"{}\" required={} -->",
        cmd, required
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_next_step_basic() {
        let output = "some output\n<!-- CSA:NEXT_STEP step_id=merge -->\nmore output";
        assert_eq!(parse_next_step(output), Some("merge".to_string()));
    }

    #[test]
    fn parse_next_step_quoted() {
        let output = "<!-- CSA:NEXT_STEP step_id=\"push_and_review\" -->";
        assert_eq!(parse_next_step(output), Some("push_and_review".to_string()));
    }

    #[test]
    fn parse_next_step_last_wins() {
        let output = "<!-- CSA:NEXT_STEP step_id=first -->\n<!-- CSA:NEXT_STEP step_id=second -->";
        assert_eq!(parse_next_step(output), Some("second".to_string()));
    }

    #[test]
    fn parse_next_step_none() {
        assert_eq!(parse_next_step("no directives here"), None);
    }

    #[test]
    fn parse_next_step_empty_id() {
        assert_eq!(parse_next_step("<!-- CSA:NEXT_STEP step_id= -->"), None);
    }

    #[test]
    fn parse_directive_cmd_and_required() {
        let output = r#"<!-- CSA:NEXT_STEP cmd="csa plan run patterns/pr-bot/workflow.toml" required=true -->"#;
        let d = parse_next_step_directive(output).unwrap();
        assert_eq!(
            d.cmd.as_deref(),
            Some("csa plan run patterns/pr-bot/workflow.toml")
        );
        assert!(d.required);
        assert!(d.step_id.is_none());
    }

    #[test]
    fn parse_directive_cmd_required_false() {
        let output = r#"<!-- CSA:NEXT_STEP cmd="echo done" required=false -->"#;
        let d = parse_next_step_directive(output).unwrap();
        assert_eq!(d.cmd.as_deref(), Some("echo done"));
        assert!(!d.required);
    }

    #[test]
    fn parse_directive_mixed_step_id_and_cmd() {
        let output = r#"<!-- CSA:NEXT_STEP step_id=merge cmd="gh pr merge" required=true -->"#;
        let d = parse_next_step_directive(output).unwrap();
        assert_eq!(d.step_id.as_deref(), Some("merge"));
        assert_eq!(d.cmd.as_deref(), Some("gh pr merge"));
        assert!(d.required);
    }

    #[test]
    fn parse_directive_last_wins() {
        let output = "<!-- CSA:NEXT_STEP cmd=\"first\" required=false -->\n<!-- CSA:NEXT_STEP cmd=\"second\" required=true -->";
        let d = parse_next_step_directive(output).unwrap();
        assert_eq!(d.cmd.as_deref(), Some("second"));
        assert!(d.required);
    }

    #[test]
    fn parse_directive_none_on_empty() {
        assert!(parse_next_step_directive("no directives here").is_none());
    }

    #[test]
    fn format_directive_required() {
        let s = format_next_step_directive("csa plan run workflow.toml", true);
        assert_eq!(
            s,
            "<!-- CSA:NEXT_STEP cmd=\"csa plan run workflow.toml\" required=true -->"
        );
    }

    #[test]
    fn format_directive_not_required() {
        let s = format_next_step_directive("echo done", false);
        assert_eq!(s, "<!-- CSA:NEXT_STEP cmd=\"echo done\" required=false -->");
    }

    #[test]
    fn format_roundtrip() {
        let cmd = "csa review --diff";
        let directive_str = format_next_step_directive(cmd, true);
        let parsed = parse_next_step_directive(&directive_str).unwrap();
        assert_eq!(parsed.cmd.as_deref(), Some(cmd));
        assert!(parsed.required);
    }
}
