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
//
// R12-1H (clause-boundary-aware tokenization): the terminal-anchor check now
// operates per-CLAUSE, not over the whole text. Sentence/clause boundaries
// (`.`, `;`, `!`, `?`, newline) are preserved during tokenization so a
// gate-object noun in a LATER clause cannot collapse across the boundary and
// forge a false pass or wrongly reject a genuine terminal pass. Within a
// clause the matched phrase must sit at the clause end or be followed ONLY by
// an explicitly-allowed modifier (`after`/`now`/`on`/`retry`). The earlier
// R8-R11 denylist model (`is_gate_object_noun`) was an open-ended list that
// kept finding new bypass words; the finite allowlist is strict and complete.

/// Pass/passed/passes only count when syntactically bound to a gate, status, or
/// result outcome AND fully anchored as a terminal clause — not embedded
/// mid-sentence in free-form prose. Bare `now passed` / `reports passed` /
/// `report passed` are unbound English, and `passed the gate logs` / `passed the
/// gate report` are gate-object references, not gate-outcome statements. Both
/// must be rejected so a real unresolved gate diagnostic is not suppressed
/// (#2806 R6-002, R8-F2).
///
/// R12-1H (clause-boundary-aware scan): the terminal-anchor check is now
/// PER-CLAUSE. `tokenize_by_clauses` preserves sentence/clause boundaries so a
/// pass clause followed by a period and unrelated prose ("gate passed. The
/// completion report was attached") is anchored — the later `report` lives in a
/// different clause and cannot veto the terminal pass. Within the pass clause
/// itself, any trailing token that is not an explicitly-allowed modifier
/// (`after`/`now`/`on`/`retry`) still rejects the clause as mid-sentence prose.
pub(super) fn gate_bound_pass_signal(lower: &str) -> bool {
    // Tokenize WITH clause boundaries so a late gate-object noun in a LATER
    // clause does not collapse across the boundary (#2806 R12-1H).
    for clause in tokenize_by_clauses(lower) {
        let tokens: &[&str] = &clause;
        let len = tokens.len();
        for start in 0..len {
            // "gate pass" / "status pass" / "result pass" — a bare outcome
            // statement. The pass token MUST be terminal-anchored WITHIN THIS
            // CLAUSE: "gate passed logs to the maintainer" is forged prose (a
            // trailing non-allowlisted token), not a gate-outcome statement
            // (#2806 R9b-F4, R12-1H). "gate passed. This turn ..." is anchored
            // (a clause boundary ends the clause) — see clause_is_terminal_anchored.
            if start + 1 < len
                && matches!(tokens[start], "gate" | "status" | "result")
                && is_pass_token(tokens[start + 1])
                && clause_is_terminal_anchored(tokens, start + 1)
            {
                return true;
            }
            // "pass gate" / "passed gate".
            if start + 1 < len
                && is_pass_token(tokens[start])
                && tokens[start + 1] == "gate"
                && clause_is_terminal_anchored(tokens, start + 1)
            {
                return true;
            }
            // "gate status pass" / "status is pass" / "result is pass" — a bare
            // outcome statement. The pass token MUST be terminal-anchored within
            // this clause the same way the two-token forms are (#2806 R10-F1,
            // R12-1H): "status is pass data downstream" is forged prose.
            if start + 2 < len
                && tokens[start] == "gate"
                && tokens[start + 1] == "status"
                && is_pass_token(tokens[start + 2])
                && clause_is_terminal_anchored(tokens, start + 2)
            {
                return true;
            }
            if start + 2 < len
                && matches!(tokens[start], "status" | "result")
                && tokens[start + 1] == "is"
                && is_pass_token(tokens[start + 2])
                && clause_is_terminal_anchored(tokens, start + 2)
            {
                return true;
            }
            // "pass the gate" / "passed the gate" / "passes the gate" — a
            // genuine gate-outcome clause ONLY when the clause ends at the gate
            // token or is followed solely by an allowed modifier
            // (#2806 R8-F2, R12-1H).
            if start + 2 < len
                && is_pass_token(tokens[start])
                && tokens[start + 1] == "the"
                && tokens[start + 2] == "gate"
                && clause_is_terminal_anchored(tokens, start + 2)
            {
                return true;
            }
        }
    }
    false
}

/// Split `lower` into clauses on sentence/clause boundaries (`.`, `;`, `!`,
/// `?`, newline), then tokenize each clause by non-alphanumeric characters.
/// Clause boundaries are PRESERVED so the terminal-anchor check only examines
/// tokens within the SAME clause as the matched phrase (#2806 R12-1H).
///
/// Before R12-1H, tokenization split the whole text on every
/// non-alphanumeric character, dropping clause delimiters. That collapsed
/// distinct clauses together: a gate-object noun (or any non-allowlisted
/// word) in a LATER clause rejected a genuine terminal pass in an EARLIER
/// clause ("gate passed. Completion confirmed." → rejected), while an
/// arbitrary trailing word absent from the denylist forged a false success
/// ("gate success was merely quoted by the assistant"). Preserving the
/// delimiters scopes the anchor check to the matched phrase's own clause.
fn tokenize_by_clauses(lower: &str) -> Vec<Vec<&str>> {
    lower
        .split(['.', ';', '!', '?', '\n', '\r'])
        .map(|clause| {
            clause
                .split(|c: char| !c.is_ascii_alphanumeric())
                .filter(|token| !token.is_empty())
                .collect::<Vec<&str>>()
        })
        .filter(|tokens| !tokens.is_empty())
        .collect()
}

/// A gate-outcome clause is terminal-anchored ONLY when every token AFTER the
/// matched phrase within the SAME clause is an explicitly-allowed modifier
/// (`after`/`now`/`on`/`retry`), or the clause ends right at the phrase
/// (#2806 R8-F2, R9b-F4, R10-F1, R12-1H).
///
/// R12-1H replaces the open-ended denylist model (`is_gate_object_noun`) with
/// a finite, strict allowlist. The denylist kept finding new bypass words
/// because natural language is infinite: "gate success was merely quoted by
/// the assistant" has no denylist noun in its trailing tokens yet is forged
/// prose. The allowlist is the inverse — only an enumerated set of small
/// modifiers may legitimately trail a terminal pass clause ("gate passed after
/// retry", "result: pass on retry", "status: success now"), so any other
/// trailing token rejects the clause. The caller scopes `clause_tokens` to a
/// single clause via [`tokenize_by_clauses`], so a reject word in a LATER
/// clause no longer interferes.
fn clause_is_terminal_anchored(clause_tokens: &[&str], index: usize) -> bool {
    clause_tokens[index + 1..]
        .iter()
        .all(|token| is_allowed_terminal_modifier(token))
}

/// The only tokens that may legitimately trail a terminal pass/success phrase
/// WITHIN its own clause. The set is derived from genuine resolution prose:
/// "gate passed after retry", "result: pass on retry", "status: success now".
/// Any other trailing token proves the clause is mid-sentence prose
/// (#2806 R12-1H).
fn is_allowed_terminal_modifier(token: &str) -> bool {
    matches!(token, "after" | "now" | "on" | "retry")
}

fn is_pass_token(token: &str) -> bool {
    matches!(token, "pass" | "passed" | "passes")
}

/// True when `lower` contains any of `phrases` as a contiguous token
/// subsequence within a single clause AND the clause is terminal-anchored at
/// the match (the phrase sits at the clause end or is followed only by an
/// allowed modifier). This applies the same clause-scoped anchoring as
/// [`gate_bound_pass_signal`] to positive-completion phrases ("gate
/// succeeded", "completed successfully", "status: success", ...) so forged
/// prose like "gate success report forwarded downstream" is rejected — the
/// trailing `report`/`downstream` are not allowed modifiers — while a genuine
/// terminal clause resolves (#2806 R11-F1, R12-1H).
///
/// Before R11-F1 these phrases used a bare `contains()` substring match that
/// bypassed the trailing-token scan entirely. R12-1H scopes the scan to the
/// matched phrase's own clause so "gate success. Completion confirmed." no
/// longer collapses the later `completion` across the boundary and wrongly
/// rejects a genuine success.
pub(super) fn any_terminal_anchored_phrase(lower: &str, phrases: &[&str]) -> bool {
    let clauses = tokenize_by_clauses(lower);
    let phrase_token_lists: Vec<Vec<&str>> = phrases
        .iter()
        .map(|phrase| {
            phrase
                .split(|character: char| !character.is_ascii_alphanumeric())
                .filter(|token| !token.is_empty())
                .collect()
        })
        .collect();
    clauses
        .iter()
        .any(|clause| phrase_token_lists.iter().any(|phrase_tokens| {
            phrase_matches_terminal_anchored(clause, phrase_tokens)
        }))
}

/// True when `phrase_tokens` appears as a contiguous subsequence of
/// `clause_tokens` and the clause is terminal-anchored at the subsequence's
/// last token. The match and anchor check are confined to a single clause, so
/// a phrase can never span a clause boundary (#2806 R12-1H). Multiple
/// occurrences within the clause are all checked; any terminal-anchored
/// occurrence resolves.
fn phrase_matches_terminal_anchored(clause_tokens: &[&str], phrase_tokens: &[&str]) -> bool {
    let plen = phrase_tokens.len();
    if plen == 0 || plen > clause_tokens.len() {
        return false;
    }
    for start in 0..=(clause_tokens.len() - plen) {
        if clause_tokens[start..start + plen] == phrase_tokens[..]
            && clause_is_terminal_anchored(clause_tokens, start + plen - 1)
        {
            return true;
        }
    }
    false
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
        if state_index >= len || !matches!(tokens[state_index], "unknown" | "unavailable" | "lost") {
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
        "prior" | "previous" | "earlier" | "former" | "initial" | "originally" | "initially"
            | "past"
    )
}
