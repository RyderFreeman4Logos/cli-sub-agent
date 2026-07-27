// Gate-bound pass-signal classification, extracted from the blocked-output
// classifier so the anchor and concurrent-veto logic stays testable without
// bloating the parent module (#2806 R8-F2, monolith gate).
//
// Two responsibilities live here:
//
// 1. `gate_bound_pass_signal` — a pass token (`pass`/`passed`/`passes`) only
//    counts when syntactically bound to a gate/status/result outcome AND fully
//    anchored as a terminal clause. `passed the gate logs to the maintainer`
//    is forged prose, not a gate-outcome statement.
// 2. `reports_concurrent_status_unknown` — a present-tense `status
//    unknown/unavailable/lost` unconditionally vetoes a positive pass claim,
//    even when the pass signal is syntactically gate-bound. Historical
//    narration (`prior status unknown`, `status unknown earlier`) is excluded
//    so a genuine retry-after-fix still resolves.
//
// This file is compiled via `include!` inside `mod gate_signal { ... }` in
// pipeline_post_exec_blocked.rs; `pub(super)` therefore refers to the parent
// blocked-output module.

/// Pass/passed/passes only count when syntactically bound to a gate, status, or
/// result outcome AND fully anchored as a terminal clause — not embedded
/// mid-sentence in free-form prose. Bare `now passed` / `reports passed` /
/// `report passed` are unbound English, and `passed the gate logs` / `passed the
/// gate report` are gate-object references, not gate-outcome statements. Both
/// must be rejected so a real unresolved gate diagnostic is not suppressed
/// (#2806 R6-002, R8-F2).
pub(super) fn gate_bound_pass_signal(lower: &str) -> bool {
    // Tokenize so bare "passed the logs" / "password" cannot match.
    let tokens: Vec<&str> = lower
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect();
    let len = tokens.len();
    for start in 0..len {
        // "gate pass" / "status pass" / "result pass" — a bare outcome
        // statement. These are inherently terminal (no object follows the pass
        // token that would reclassify it as prose), so no anchor check.
        if start + 1 < len
            && matches!(tokens[start], "gate" | "status" | "result")
            && is_pass_token(tokens[start + 1])
        {
            return true;
        }
        // "pass gate" / "passed gate".
        if start + 1 < len
            && is_pass_token(tokens[start])
            && tokens[start + 1] == "gate"
            && gate_clause_is_anchored(&tokens, start + 1)
        {
            return true;
        }
        // "gate status pass" / "status is pass" / "result is pass".
        if start + 2 < len
            && tokens[start] == "gate"
            && tokens[start + 1] == "status"
            && is_pass_token(tokens[start + 2])
        {
            return true;
        }
        if start + 2 < len
            && matches!(tokens[start], "status" | "result")
            && tokens[start + 1] == "is"
            && is_pass_token(tokens[start + 2])
        {
            return true;
        }
        // "pass the gate" / "passed the gate" / "passes the gate" — a genuine
        // gate-outcome clause ONLY when not followed by a gate-object noun
        // (logs/report/output/data/results/...). "passed the gate logs to the
        // maintainer" is forged prose, not a pass signal (#2806 R8-F2).
        if start + 2 < len
            && is_pass_token(tokens[start])
            && tokens[start + 1] == "the"
            && tokens[start + 2] == "gate"
            && gate_clause_is_anchored(&tokens, start + 2)
        {
            return true;
        }
    }
    false
}

/// A `pass ... gate` / `pass the gate` clause is anchored (a real gate-outcome
/// statement) when the token immediately after the `gate` token is end-of-string
/// or a non-object token. A trailing gate-object noun (`logs`, `report`,
/// `output`, `data`, `to`, `downstream`, ...) proves the clause is embedded
/// mid-sentence prose and must not be treated as a pass signal (#2806 R8-F2).
fn gate_clause_is_anchored(tokens: &[&str], gate_index: usize) -> bool {
    match tokens.get(gate_index + 1) {
        None => true,
        Some(after) => !is_gate_object_noun(after),
    }
}

/// Nouns/particles that, when following `gate` in a `pass ... gate` clause,
/// prove the clause is mid-sentence prose rather than a gate-outcome statement.
fn is_gate_object_noun(token: &str) -> bool {
    matches!(
        token,
        "logs"
            | "log"
            | "report"
            | "reports"
            | "output"
            | "outputs"
            | "data"
            | "result"
            | "results"
            | "to"
            | "downstream"
            | "upstream"
            | "file"
            | "files"
            | "command"
            | "commands"
            | "string"
            | "strings"
            | "argument"
            | "args"
            | "handoff"
    )
}

fn is_pass_token(token: &str) -> bool {
    matches!(token, "pass" | "passed" | "passes")
}

/// Detects a CONCURRENT "status unknown/unavailable/lost" claim. A present-tense
/// `status <unknown|unavailable|lost>` (optionally via `status is <state>`)
/// vetoes a positive pass signal — `gate passed, but gate status is unknown`
/// cannot resolve. Historical narration (`prior status unknown`, `previous
/// status unknown`, `status unknown earlier`) is explicitly excluded so a
/// genuine retry-after-fix still resolves (#2806 R8-F2).
pub(super) fn reports_concurrent_status_unknown(lower: &str) -> bool {
    let tokens: Vec<&str> = lower
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect();
    let len = tokens.len();
    for index in 0..len {
        if tokens[index] != "status" {
            continue;
        }
        // The state token follows directly, or one slot after an "is".
        let mut state_index = index + 1;
        if state_index < len && tokens[state_index] == "is" {
            state_index += 1;
        }
        if state_index >= len || !matches!(tokens[state_index], "unknown" | "unavailable" | "lost")
        {
            continue;
        }
        // Historical qualifiers within two tokens before "status", or in the two
        // tokens after the state word, mark this as past narration, not a
        // concurrent veto.
        let before = tokens[index.saturating_sub(2)..index].to_vec();
        let after = tokens
            .get(state_index + 1..state_index + 3)
            .map(|slice| slice.to_vec())
            .unwrap_or_default();
        if before.iter().any(|token| is_historical_qualifier(token))
            || after.iter().any(|token| is_historical_qualifier(token))
        {
            continue;
        }
        return true;
    }
    false
}

/// Qualifiers that mark a "status unknown" claim as historical (past) rather
/// than concurrent (present).
fn is_historical_qualifier(token: &str) -> bool {
    matches!(
        token,
        "prior"
            | "previous"
            | "earlier"
            | "former"
            | "initial"
            | "originally"
            | "initially"
            | "past"
    )
}
