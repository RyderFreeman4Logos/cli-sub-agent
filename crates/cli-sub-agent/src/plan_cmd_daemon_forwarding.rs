//! Daemon-child argv forwarding for `csa plan run`.
//!
//! Split out of `plan_cmd_daemon.rs` to stay under the monolith token budget.
//! These helpers transform the parent process argv into the token list handed
//! to the daemon child; the daemon spawner re-injects
//! `--daemon-child --session-id <ID>` itself.

use crate::plan_cmd::{FEATURE_INPUT_VAR, ISSUE_NUMBER_VAR};

/// Build daemon-child args from the parent's argv.
///
/// `argv` looks like `["csa", ...global, "plan", "run", ...rest]`. We strip
/// everything up through `plan run`, drop the `--foreground` opt-out (the
/// child is the actual worker, not a re-spawn that should opt out again),
/// and forward the remainder. The daemon spawner re-injects
/// `--daemon-child --session-id <ID>` between `run` and the rest.
///
/// Filter contract: `--foreground` is the ONLY token stripped here, and
/// only because (a) clap parsed it as a top-level boolean flag with no
/// value-position semantics, and (b) it's a parent-only opt-out the daemon
/// child must not see. The filter stops at the first `--` so any literal
/// `--foreground` that appears AFTER a `--` positional separator (e.g. a
/// future workflow argument that happens to share the spelling) is left
/// untouched. DO NOT add other flag strips here without preserving this
/// `--`-aware behavior — naive `*a != "--xxx"` filters break value-position
/// usage and `--`-escaped positionals.
pub(crate) fn build_forwarded_plan_args(all_args: &[String]) -> Vec<String> {
    let plan_pos = all_args.iter().position(|a| a == "plan");
    let Some(plan_pos) = plan_pos else {
        return Vec::new();
    };
    // Skip `plan` and the immediately-following `run` verb.
    let after_plan = plan_pos + 1;
    let after_run = all_args
        .iter()
        .enumerate()
        .skip(after_plan)
        .find(|(_, a)| *a == "run")
        .map(|(idx, _)| idx + 1)
        .unwrap_or(after_plan);

    let mut forwarded = Vec::with_capacity(all_args.len().saturating_sub(after_run));
    let mut past_double_dash = false;
    for token in all_args.iter().skip(after_run) {
        if past_double_dash {
            forwarded.push(token.clone());
            continue;
        }
        if token == "--" {
            past_double_dash = true;
            forwarded.push(token.clone());
            continue;
        }
        if token == "--foreground" {
            continue;
        }
        forwarded.push(token.clone());
    }
    forwarded
}

/// Build daemon-child forwarded args for the `--issue` path.
///
/// Starts from the normal [`build_forwarded_plan_args`] output, drops the
/// already-resolved `--issue <N>` / `--issue=<N>` token(s), and appends the
/// fetched issue body plus numeric issue number as workflow variables. This
/// lets the daemon child consume the pre-fetched body instead of re-running
/// `gh issue view`, so the issue is fetched exactly once (in the parent).
///
/// Like [`build_forwarded_plan_args`], the `--issue` strip is `--`-aware: a
/// literal `--issue`/`--issue=` token appearing AFTER a `--` positional
/// separator is a workflow argument and is preserved intact.
pub(crate) fn forwarded_args_with_feature_input(
    feature_input: &str,
    issue_number: u64,
) -> Vec<String> {
    let argv: Vec<String> = std::env::args().collect();
    let base = build_forwarded_plan_args(&argv);
    forwarded_args_with_issue_vars(base, feature_input, issue_number)
}

fn forwarded_args_with_issue_vars(
    base: Vec<String>,
    feature_input: &str,
    issue_number: u64,
) -> Vec<String> {
    let mut forwarded = Vec::with_capacity(base.len() + 4);
    let mut post_double_dash = Vec::new();
    let mut tokens = base.into_iter();
    let mut past_double_dash = false;
    while let Some(token) = tokens.next() {
        if past_double_dash {
            post_double_dash.push(token);
            continue;
        }
        match token.as_str() {
            "--" => {
                past_double_dash = true;
            }
            // Space form `--issue <N>`: drop the flag and its value token.
            "--issue" => {
                tokens.next();
            }
            // Equals form `--issue=<N>`: drop the single token.
            t if t.starts_with("--issue=") => {}
            _ => forwarded.push(token),
        }
    }
    forwarded.push("--var".to_string());
    forwarded.push(format!("{FEATURE_INPUT_VAR}={feature_input}"));
    forwarded.push("--var".to_string());
    forwarded.push(format!("{ISSUE_NUMBER_VAR}={issue_number}"));
    if past_double_dash {
        forwarded.push("--".to_string());
        forwarded.extend(post_double_dash);
    }
    forwarded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_forwarding_injects_issue_number_before_double_dash() {
        let base = vec![
            "--pattern".to_string(),
            "dev2merge".to_string(),
            "--issue".to_string(),
            "1663".to_string(),
            "--".to_string(),
            "--issue".to_string(),
            "literal".to_string(),
        ];

        let forwarded = forwarded_args_with_issue_vars(base, "review body", 1663);

        assert_eq!(
            forwarded,
            vec![
                "--pattern",
                "dev2merge",
                "--var",
                "FEATURE_INPUT=review body",
                "--var",
                "ISSUE_NUMBER=1663",
                "--",
                "--issue",
                "literal",
            ]
        );
    }

    #[test]
    fn issue_forwarding_strips_equals_issue_form() {
        let base = vec!["workflow.toml".to_string(), "--issue=1663".to_string()];

        let forwarded = forwarded_args_with_issue_vars(base, "review body", 1663);

        assert_eq!(
            forwarded,
            vec![
                "workflow.toml",
                "--var",
                "FEATURE_INPUT=review body",
                "--var",
                "ISSUE_NUMBER=1663",
            ]
        );
    }
}
