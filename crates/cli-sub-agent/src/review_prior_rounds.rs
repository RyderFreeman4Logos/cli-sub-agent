use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

pub(crate) const REVIEW_FINDINGS_TOML_INSTRUCTION: &str = "After the CSA summary/details sections, append exactly one fenced TOML block labeled `findings.toml` for machine parsing. Keep that fenced block OUTSIDE the CSA sections so `details.md` remains unchanged. Use `findings = []` when there are no findings.";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct PriorRoundsSummary {
    #[serde(default)]
    pub(crate) round: Vec<PriorRound>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct PriorRound {
    pub(crate) number: u32,
    pub(crate) commit: String,
    pub(crate) summary: String,
    pub(crate) invariant: String,
}

pub(crate) fn load_prior_rounds_summary(path: &Path) -> Result<PriorRoundsSummary> {
    let content = fs::read_to_string(path).with_context(|| {
        format!(
            "Failed to read prior-rounds summary file: {}",
            path.display()
        )
    })?;
    toml::from_str(&content).with_context(|| {
        format!(
            "Failed to parse prior-rounds summary TOML: {}",
            path.display()
        )
    })
}

pub(crate) fn load_prior_rounds_section(path: &Path) -> Result<String> {
    let summary = load_prior_rounds_summary(path)?;
    Ok(render_prior_rounds_section(&summary))
}

pub(crate) fn render_prior_rounds_section(summary: &PriorRoundsSummary) -> String {
    let mut rendered = String::from("## Prior-Round Invariant Verification\n\n");
    rendered.push_str("Prior rounds of review have fired on this branch and their fixes are now\n");
    rendered.push_str("part of the diff you are reviewing. Verify each prior-round fix did NOT\n");
    rendered.push_str("introduce a regression:\n\n");

    if summary.round.is_empty() {
        rendered.push_str("No prior-round summaries were provided.\n\n");
    } else {
        for round in &summary.round {
            rendered.push_str(&format!(
                "- Round {} (commit {}): {} - Invariant: {}\n",
                round.number, round.commit, round.summary, round.invariant
            ));
        }
        rendered.push('\n');
    }

    rendered.push_str("For each invariant above, explicitly check:\n");
    rendered
        .push_str("1. The fix from that round still enforces the invariant in the current diff.\n");
    rendered.push_str(
        "2. No later commit accidentally loosened the fix (e.g., made a guard too broad, reintroduced a race).\n",
    );
    rendered.push_str(
        "3. If a fix narrowed a bug, the narrowing still covers every reachable code path.\n\n",
    );
    rendered.push_str("FAIL findings that break any prior-round invariant. Preserve PASS for\n");
    rendered.push_str("invariants that hold under the current diff.");
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_prior_rounds_section_includes_round_summaries_and_invariants() {
        let rendered = render_prior_rounds_section(&PriorRoundsSummary {
            round: vec![
                PriorRound {
                    number: 6,
                    commit: "29b6c34c".to_string(),
                    summary: "narrowed legacy ACP fallback to tool == \"codex\"".to_string(),
                    invariant: "ACP codex sessions route to output.log".to_string(),
                },
                PriorRound {
                    number: 7,
                    commit: "2fdcba62".to_string(),
                    summary: "moved runtime_binary write behind lock".to_string(),
                    invariant: "lock-losing resume cannot mutate metadata".to_string(),
                },
            ],
        });

        assert!(rendered.contains("## Prior-Round Invariant Verification"));
        assert!(rendered.contains("Round 6 (commit 29b6c34c)"));
        assert!(rendered.contains("ACP codex sessions route to output.log"));
        assert!(rendered.contains("Round 7 (commit 2fdcba62)"));
        assert!(rendered.contains("lock-losing resume cannot mutate metadata"));
    }
}
