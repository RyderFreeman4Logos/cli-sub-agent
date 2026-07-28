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
/// `status <unknown|unavailable|lost>` (optionally via `status is <state>`, or
/// with a current-tense modifier `status currently unknown` / `status now
/// unavailable`) vetoes a positive pass signal — `gate passed, but gate status
/// is unknown` cannot resolve. Historical narration (`prior status unknown`,
/// `previous status unknown`, `status unknown earlier`) is explicitly excluded
/// so a genuine retry-after-fix still resolves (#2806 R8-F2). The qualifier and
/// status claim must occur in the same clause; historical narration in a prior
/// clause cannot exempt a later current unknown (#2806 R13-P1). A current-tense
/// modifier anywhere in the SAME clause marks the status claim as concurrent,
/// even when a distant historical qualifier also sits in the clause (#2806
/// R14-F1): `prior status is currently unknown` is a concurrent unknown.
pub(super) fn reports_concurrent_status_unknown(lower: &str) -> bool {
    for clause_tokens in tokenize_by_clauses(lower) {
        for index in 0..clause_tokens.len() {
            if clause_tokens[index] != "status" {
                continue;
            }
            // The state token follows directly, optionally via `status is
            // <state>` and optionally with a current-tense modifier
            // (`currently`/`now`) between `status`/`is` and the state token:
            // "status currently unknown", "status is now unavailable",
            // "status now unavailable" (#2806 R14-F1).
            let mut state_index = index + 1;
            if state_index < clause_tokens.len() && clause_tokens[state_index] == "is" {
                state_index += 1;
            }
            state_index = skip_current_tense_modifiers(&clause_tokens, state_index);
            if state_index >= clause_tokens.len()
                || !matches!(
                    clause_tokens[state_index],
                    "unknown" | "unavailable" | "lost"
                )
            {
                continue;
            }
            // Any current-tense modifier in the SAME clause marks the claim
            // concurrent, even if a historical qualifier also appears in that
            // clause (#2806 R14-F1).
            if clause_has_current_tense_modifier(&clause_tokens) {
                return true;
            }
            if clause_has_historical_qualifier(&clause_tokens) {
                continue;
            }
            return true;
        }
    }
    false
}

/// True when a `failed`/`failure` word is a present outcome rather than
/// historical narration. This uses the same clause-scoped historical qualifier
/// check as status-unknown detection: any qualifier in the SAME clause marks
/// the occurrence as historical (not just ±2 neighboring tokens), so a prior
/// clause's failure does not veto a later terminal gate pass (#2806 R13-P2,
/// R14-F2).
pub(super) fn reports_current_failure(lower: &str) -> bool {
    tokenize_by_clauses(lower).into_iter().any(|clause_tokens| {
        clause_tokens.iter().any(|token| {
            matches!(*token, "failed" | "failure")
                && !clause_has_historical_qualifier(&clause_tokens)
        })
    })
}

/// True when the message contains any unresolved-gate signal
/// (`remains unknown`, `could not confirm`, `did not pass`, ...) in a CURRENT
/// clause. The signal is matched per-clause as a contiguous token subsequence
/// — a historical qualifier ANYWHERE in the SAME clause marks the occurrence as
/// past narration, so a prior clause's unresolved signal does not veto a later
/// terminal gate pass (#2806 R14-F2).
///
/// Before R14-F2 these phrases used a whole-message `contains()` substring
/// test. That had no clause scoping at all, so a historical phrase in an
/// earlier clause (e.g. "Previous attempt could not confirm gate pass. Gate
/// passed.") blocked a genuine later terminal pass — the historical phrase sat
/// in the wrong scope and exempted nothing.
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
        !clause_has_historical_qualifier(&clause_tokens)
            && signal_token_lists
                .iter()
                .any(|signal_tokens| phrase_occurs(&clause_tokens, signal_tokens))
    })
}

/// True when a clause contains a current (present) omission of required work —
/// `omitted` alongside `test` and `commit` in the SAME clause, without a
/// historical qualifier in that clause. Historical narration
/// (`previously omitted tests and commit; this turn completed both`) is
/// excluded clause-by-clause, so a prior clause's historical omission does not
/// exempt a LATER clause's current omission on the same line (#2806 R10-F2,
/// R14-F3).
///
/// Before R14-F3 the check was LINE-scoped: a historical marker on a line
/// exempted every clause on that line, so a mixed line such as "Gate passed.
/// Previous turn omitted tests and commit; tests and commit omitted this turn."
/// failed to flag the current omission after the semicolon.
pub(super) fn reports_current_omitted_required_work(lower: &str) -> bool {
    tokenize_by_clauses(lower)
        .into_iter()
        .any(|clause_tokens| {
            clause_contains_omitted_test_commit(&clause_tokens)
                && !clause_has_historical_qualifier(&clause_tokens)
        })
}

/// True when `phrase_tokens` appears as a contiguous subsequence of
/// `clause_tokens` anywhere in the clause. This is the same matching primitive
/// [`phrase_matches_terminal_anchored`] uses, but WITHOUT the terminal-anchor
/// constraint — an unresolved signal vetoes a pass even when it is not clause
/// terminal.
fn phrase_occurs(clause_tokens: &[&str], phrase_tokens: &[&str]) -> bool {
    let plen = phrase_tokens.len();
    if plen == 0 || plen > clause_tokens.len() {
        return false;
    }
    clause_tokens
        .windows(plen)
        .any(|window| window == phrase_tokens)
}

/// True when a clause contains `omitted` alongside `test` and `commit`. The
/// historical qualifier is evaluated separately so a mixed clause
/// (historical + current) still flags the current part (#2806 R14-F3).
fn clause_contains_omitted_test_commit(clause_tokens: &[&str]) -> bool {
    clause_tokens.contains(&"omitted")
        && (clause_tokens.contains(&"test") || clause_tokens.contains(&"tests"))
        && clause_tokens.contains(&"commit")
}

/// Skip any current-tense modifiers (`currently`/`now`) starting at `index`,
/// returning the index of the next non-modifier token. Allows "status
/// currently unknown", "status is now unavailable", and "status now
/// unavailable" (#2806 R14-F1).
fn skip_current_tense_modifiers(clause_tokens: &[&str], mut index: usize) -> usize {
    while index < clause_tokens.len() && is_current_tense_modifier(clause_tokens[index]) {
        index += 1;
    }
    index
}

/// True when the clause contains ANY historical qualifier token. The scan is
/// whole-clause, not ±2 tokens around the matched subject: a qualifier anywhere
/// in the same clause marks the claim as historical narration. Callers supply
/// tokens from a single clause (via [`tokenize_by_clauses`]) so a
/// punctuation-separated qualifier in a prior clause cannot suppress a later
/// current claim (#2806 R14-F2, R14-F3).
fn clause_has_historical_qualifier(clause_tokens: &[&str]) -> bool {
    clause_tokens.iter().any(|token| is_historical_qualifier(token))
}

/// True when the clause contains ANY current-tense modifier. A current-tense
/// modifier anywhere in the same clause overrides a historical qualifier so a
/// concurrent claim is not wrongly exempted (#2806 R14-F1).
fn clause_has_current_tense_modifier(clause_tokens: &[&str]) -> bool {
    clause_tokens.iter().any(|token| is_current_tense_modifier(token))
}

/// Qualifiers that mark a claim as historical (past) rather than concurrent
/// (present).
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

/// Current-tense modifiers that mark a claim as concurrent (present). These
/// override a historical qualifier when both sit in the SAME clause, because a
/// phrase such as "prior status is currently unknown" reports a concurrent
/// unknown with a historical subject (#2806 R14-F1).
fn is_current_tense_modifier(token: &str) -> bool {
    matches!(token, "currently" | "now")
}
