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

/// Concurrent `status [is] [currently|now] unknown|unavailable|lost` vetoes a
/// pass. Historical narration in the same coordination segment is excluded
/// (#2806 R8/R13). R15-F2: only a current-tense modifier *inside* the matched
/// status claim overrides history — a later comma-joined `now gate passed`
/// must not reclassify a historical unknown as concurrent.
pub(super) fn reports_concurrent_status_unknown(lower: &str) -> bool {
    for clause_tokens in tokenize_by_clauses(lower) {
        for index in 0..clause_tokens.len() {
            if clause_tokens[index] != "status" {
                continue;
            }
            // Optional `is` and local `currently`/`now` before the state token.
            let mut state_index = index + 1;
            if state_index < clause_tokens.len() && clause_tokens[state_index] == "is" {
                state_index += 1;
            }
            let after_modifiers = skip_current_tense_modifiers(&clause_tokens, state_index);
            let has_local_current_modifier = after_modifiers != state_index;
            state_index = after_modifiers;
            if state_index >= clause_tokens.len()
                || !matches!(
                    clause_tokens[state_index],
                    "unknown" | "unavailable" | "lost"
                )
            {
                continue;
            }
            // Local current-tense modifier forces concurrent (R14-F1, R15-F2).
            if has_local_current_modifier {
                return true;
            }
            // Historical qualifier only exempts its own coordination segment.
            if segment_has_qualifier_at(&clause_tokens, index) {
                continue;
            }
            return true;
        }
    }
    false
}

/// Present-tense `failed`/`failure` veto; historical only when a qualifier is
/// in the same coordination segment as the occurrence (#2806 R13/R14/R15-F1).
pub(super) fn reports_current_failure(lower: &str) -> bool {
    tokenize_by_clauses(lower).into_iter().any(|clause_tokens| {
        clause_tokens.iter().enumerate().any(|(index, token)| {
            matches!(*token, "failed" | "failure")
                && !segment_has_qualifier_at(&clause_tokens, index)
        })
    })
}

/// Current unresolved-gate signals (`remains blocked`, `could not confirm`,
/// ...) as contiguous token subsequences. Historical only when a qualifier is
/// in the same coordination segment as the matched phrase (#2806 R14/R15-F1).
pub(super) fn reports_current_unresolved_signal(lower: &str) -> bool {
    let unresolved_signals: &[&str] = &[
        "remains unknown",
        "still unknown",
        "remains unavailable",
        "still unavailable",
        "could not confirm",
        "unable to confirm",
        "cannot confirm",
        "remains blocked",
        "still blocked",
        "did not pass",
        "not pass",
    ];
    let signal_token_lists: Vec<Vec<&str>> = unresolved_signals
        .iter()
        .map(|signal| {
            signal
                .split(|character: char| !character.is_ascii_alphanumeric())
                .filter(|token| !token.is_empty())
                .collect()
        })
        .collect();
    tokenize_by_clauses(lower).into_iter().any(|clause_tokens| {
        // R16-F1: inspect EVERY occurrence of each signal phrase. First-match
        // only would treat "Previous gate remains blocked but gate remains
        // blocked" as historical and miss the concurrent second occurrence.
        signal_token_lists.iter().any(|signal_tokens| {
            phrase_occurrence_indices(&clause_tokens, signal_tokens).any(|occurrence_index| {
                !segment_has_qualifier_at(&clause_tokens, occurrence_index)
            })
        })
    })
}

/// Current omission of required work (`omitted` + `test`/`tests` + `commit`).
/// R16-F2: evaluate each coordination segment independently — a historical
/// first `omitted` must not exempt a concurrent omission after `but`.
pub(super) fn reports_current_omitted_required_work(lower: &str) -> bool {
    tokenize_by_clauses(lower).into_iter().any(|clause_tokens| {
        segment_boundaries(&clause_tokens).into_iter().any(|seg| {
            let segment = &clause_tokens[seg.start..seg.end];
            clause_contains_omitted_test_commit(segment)
                && !segment
                    .iter()
                    .any(|token| is_historical_qualifier(token))
        })
    })
}

/// Starting indices of every contiguous match of `phrase_tokens`.
fn phrase_occurrence_indices<'a>(
    clause_tokens: &'a [&str],
    phrase_tokens: &'a [&str],
) -> impl Iterator<Item = usize> + 'a {
    let plen = phrase_tokens.len();
    let max_start = clause_tokens.len().saturating_sub(plen);
    // Empty phrase or phrase longer than the clause: no matches. The
    // saturating_sub alone is not enough — when plen > len, max_start is 0 and
    // `0..=0` would still attempt an out-of-range slice.
    let has_room = plen > 0 && plen <= clause_tokens.len();
    (0..=max_start).filter(move |start| {
        has_room && clause_tokens[*start..*start + plen] == *phrase_tokens
    })
}

/// Tokens contain `omitted` plus `test`/`tests` and `commit`.
fn clause_contains_omitted_test_commit(tokens: &[&str]) -> bool {
    tokens.contains(&"omitted")
        && (tokens.contains(&"test") || tokens.contains(&"tests"))
        && tokens.contains(&"commit")
}

/// Skip leading `currently`/`now` tokens from `index`.
fn skip_current_tense_modifiers(clause_tokens: &[&str], mut index: usize) -> usize {
    while index < clause_tokens.len() && is_current_tense_modifier(clause_tokens[index]) {
        index += 1;
    }
    index
}

/// Historical qualifier in the coordination segment containing `index` (R15-F1).
fn segment_has_qualifier_at(clause_tokens: &[&str], index: usize) -> bool {
    segment_boundaries(clause_tokens).into_iter().any(|seg| {
        seg.start <= index
            && index < seg.end
            && clause_tokens[seg.start..seg.end]
                .iter()
                .any(|token| is_historical_qualifier(token))
    })
}

/// Split on `but`, or on `and` before a gate-outcome subject (R15-F1).
fn segment_boundaries(clause_tokens: &[&str]) -> Vec<std::ops::Range<usize>> {
    let mut segments: Vec<std::ops::Range<usize>> = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < clause_tokens.len() {
        let token = clause_tokens[index];
        let is_coordination_boundary = (token == "but")
            || (token == "and"
                && index + 1 < clause_tokens.len()
                && matches!(clause_tokens[index + 1], "gate" | "status" | "result"));
        if is_coordination_boundary {
            segments.push(start..index);
            start = index;
        }
        index += 1;
    }
    segments.push(start..clause_tokens.len());
    segments
}

fn is_historical_qualifier(token: &str) -> bool {
    matches!(
        token,
        "prior"
            | "previous"
            | "previously"
            | "earlier"
            | "former"
            | "initial"
            | "originally"
            | "initially"
            | "past"
    )
}

/// Current-tense modifiers; only override history when inside the matched
/// status claim (R15-F2).
fn is_current_tense_modifier(token: &str) -> bool {
    matches!(token, "currently" | "now")
}
