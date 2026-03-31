//! CSA directive parsing from hook/step output.
//!
//! Directives are HTML-comment-style markers embedded in stdout:
//! `<!-- CSA:NEXT_STEP step_id=<id> -->` — instruct weave to jump to a step.

/// A parsed CSA directive from hook or step output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CsaDirective {
    /// Jump to the named weave step.
    NextStep { step_id: String },
}

/// Parse all `CSA:NEXT_STEP` directives from text output.
///
/// Returns the **last** `NEXT_STEP` directive found (later directives
/// override earlier ones, matching the "last writer wins" convention).
pub fn parse_next_step(output: &str) -> Option<String> {
    let mut last_step_id = None;

    for line in output.lines() {
        let trimmed = line.trim();
        // Match: <!-- CSA:NEXT_STEP step_id=<value> -->
        if let Some(rest) = trimmed.strip_prefix("<!-- CSA:NEXT_STEP")
            && let Some(rest) = rest.strip_suffix("-->")
        {
            let rest = rest.trim();
            if let Some(value) = rest.strip_prefix("step_id=") {
                let id = value.trim().trim_matches('"').trim();
                if !id.is_empty() {
                    last_step_id = Some(id.to_string());
                }
            }
        }
    }

    last_step_id
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
}
