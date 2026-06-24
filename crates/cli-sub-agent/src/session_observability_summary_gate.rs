use std::path::Path;

const REVIEW_SUMMARY_FAIL_TOKENS: &[&str] = &["FAIL", "HAS_ISSUES", "REJECT"];

pub(crate) fn human_review_summary_requires_failed_gate(
    session_dir: &Path,
    raw_summary: &str,
) -> bool {
    crate::session_summary_text::human_session_summary(session_dir, raw_summary).is_some_and(
        |summary| {
            review_summary_has_fail_verdict(&summary)
                || human_review_summary_can_apply_blocking_outcomes(session_dir)
                    && review_summary_has_blocking_outcome(&summary)
        },
    )
}

fn human_review_summary_can_apply_blocking_outcomes(session_dir: &Path) -> bool {
    session_dir.join("review_meta.json").is_file()
        || super::legacy_review_pass::is_review_session_dir(session_dir)
}

fn review_summary_has_fail_verdict(summary: &str) -> bool {
    summary.lines().map(str::trim).any(|line| {
        REVIEW_SUMMARY_FAIL_TOKENS
            .iter()
            .any(|token| summary_line_has_verdict_prefix(line, token))
    })
}

fn review_summary_has_blocking_outcome(summary: &str) -> bool {
    summary.lines().any(summary_line_has_blocking_outcome)
}

fn summary_line_has_blocking_outcome(line: &str) -> bool {
    let normalized = line.to_ascii_lowercase();
    summary_line_has_unnegated_high_severity(&normalized)
        || summary_line_has_unnegated_critical_severity(&normalized)
        || summary_line_has_unnegated_blocking_outcome(&normalized)
        || summary_line_has_unnegated_p1_outcome(&normalized)
}

fn summary_line_has_unnegated_high_severity(normalized: &str) -> bool {
    (normalized.contains("high-severity") || normalized.contains("high severity"))
        && !summary_line_negates_high_severity(normalized)
        && (summary_line_has_nonzero_count_metric(normalized, &["high severity", "high-severity"])
            || summary_line_has_blocking_result_signal(normalized))
}

fn summary_line_negates_high_severity(normalized: &str) -> bool {
    normalized.contains("no high")
        || summary_line_has_zero_count(normalized, "0 high")
        || summary_line_has_zero_count_metric(normalized, &["high severity", "high-severity"])
}

fn summary_line_has_unnegated_critical_severity(normalized: &str) -> bool {
    (normalized.contains("critical-severity") || normalized.contains("critical severity"))
        && !summary_line_negates_critical_severity(normalized)
        && (summary_line_has_nonzero_count_metric(
            normalized,
            &["critical severity", "critical-severity"],
        ) || summary_line_has_blocking_result_signal(normalized))
}

fn summary_line_negates_critical_severity(normalized: &str) -> bool {
    normalized.contains("no critical")
        || summary_line_has_zero_count(normalized, "0 critical")
        || summary_line_has_zero_count_metric(
            normalized,
            &["critical severity", "critical-severity"],
        )
}

fn summary_line_has_unnegated_blocking_outcome(normalized: &str) -> bool {
    (normalized.contains("blocking finding") || normalized.contains("blocking issue"))
        && !summary_line_negates_blocking_outcome(normalized)
        && (summary_line_has_nonzero_count_metric(normalized, &["blocking"])
            || summary_line_has_blocking_result_signal(normalized))
}

fn summary_line_negates_blocking_outcome(normalized: &str) -> bool {
    normalized.contains("non-blocking")
        || normalized.contains("no blocking")
        || summary_line_has_zero_count(normalized, "0 blocking")
        || summary_line_has_zero_count_metric(normalized, &["blocking"])
        || normalized.contains("no correctness, regression, security, or blocking")
}

fn summary_line_has_unnegated_p1_outcome(normalized: &str) -> bool {
    (summary_line_has_metric_label(normalized, "p1")
        || normalized.contains("p1 finding")
        || normalized.contains("p1 issue")
        || normalized.contains("p1 correctness"))
        && !summary_line_negates_p1_outcome(normalized)
        && (summary_line_has_nonzero_count_metric(normalized, &["p1"])
            || summary_line_has_blocking_result_signal(normalized))
}

fn summary_line_negates_p1_outcome(normalized: &str) -> bool {
    normalized.contains("no p1")
        || summary_line_has_zero_count(normalized, "0 p1")
        || summary_line_has_zero_count_metric(normalized, &["p1"])
}

fn summary_line_has_zero_count(normalized: &str, prefix: &str) -> bool {
    normalized.starts_with(prefix) || normalized.contains(&format!(" {prefix}"))
}

fn summary_line_has_zero_count_metric(normalized: &str, labels: &[&str]) -> bool {
    const ZERO_COUNT_NOUNS: &[&str] = &[
        "bug",
        "bugs",
        "defect",
        "defects",
        "finding",
        "findings",
        "issue",
        "issues",
        "violation",
        "violations",
        "vulnerability",
        "vulnerabilities",
    ];

    labels.iter().any(|label| {
        summary_line_has_zero_metric(normalized, label)
            || ZERO_COUNT_NOUNS
                .iter()
                .any(|noun| summary_line_has_zero_metric(normalized, &format!("{label} {noun}")))
    })
}

fn summary_line_has_nonzero_count_metric(normalized: &str, labels: &[&str]) -> bool {
    const NONZERO_COUNT_NOUNS: &[&str] = &[
        "bug",
        "bugs",
        "defect",
        "defects",
        "finding",
        "findings",
        "issue",
        "issues",
        "violation",
        "violations",
        "vulnerability",
        "vulnerabilities",
    ];

    labels.iter().any(|label| {
        summary_line_has_nonzero_metric(normalized, label)
            || summary_line_has_nonzero_count_before_label(normalized, label)
            || NONZERO_COUNT_NOUNS.iter().any(|noun| {
                let label_with_noun = format!("{label} {noun}");
                summary_line_has_nonzero_metric(normalized, &label_with_noun)
                    || summary_line_has_nonzero_count_before_label(normalized, &label_with_noun)
            })
    })
}

fn summary_line_has_zero_metric(normalized: &str, label: &str) -> bool {
    summary_metric_label_variants(label).iter().any(|variant| {
        normalized.contains(&format!("{variant}: 0"))
            || normalized.contains(&format!("{variant} = 0"))
    })
}

fn summary_line_has_nonzero_metric(normalized: &str, label: &str) -> bool {
    summary_metric_label_variants(label).iter().any(|variant| {
        [format!("{variant}: "), format!("{variant} = ")]
            .iter()
            .any(|marker| summary_line_has_nonzero_value_after(normalized, marker))
    })
}

fn summary_line_has_nonzero_value_after(normalized: &str, marker: &str) -> bool {
    normalized
        .match_indices(marker)
        .any(|(idx, _)| parse_leading_nonzero(&normalized[idx + marker.len()..]))
}

fn summary_line_has_nonzero_count_before_label(normalized: &str, label: &str) -> bool {
    normalized.match_indices(label).any(|(idx, _)| {
        let before = normalized[..idx].trim_end();
        let digits_start = before
            .char_indices()
            .rev()
            .find(|(_, ch)| !ch.is_ascii_digit())
            .map_or(0, |(pos, ch)| pos + ch.len_utf8());
        parse_leading_nonzero(&before[digits_start..])
    })
}

fn parse_leading_nonzero(input: &str) -> bool {
    let digits: String = input
        .trim_start()
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    digits.parse::<u64>().is_ok_and(|value| value > 0)
}

fn summary_line_has_blocking_result_signal(normalized: &str) -> bool {
    const SIGNALS: &[&str] = &[
        " found",
        " remains",
        " remain",
        " reported",
        " present",
        " was found",
        " were found",
    ];
    const POST_NEGATIONS: &[&str] = &[" none", " nothing", " no ", " zero", " 0 ", " previously"];

    SIGNALS.iter().any(|signal| {
        normalized.match_indices(signal).any(|(idx, _)| {
            let after = &normalized[idx + signal.len()..];
            !POST_NEGATIONS.iter().any(|neg| after.starts_with(neg))
        })
    })
}

fn summary_line_has_metric_label(normalized: &str, label: &str) -> bool {
    summary_metric_label_variants(label).iter().any(|variant| {
        normalized.starts_with(&format!("{variant}:"))
            || normalized.starts_with(&format!("{variant} ="))
            || normalized.contains(&format!(" {variant}:"))
            || normalized.contains(&format!(" {variant} ="))
    })
}

fn summary_metric_label_variants(label: &str) -> [String; 4] {
    [
        label.to_string(),
        format!("**{label}**"),
        format!("__{label}__"),
        format!("`{label}`"),
    ]
}

fn summary_line_has_verdict_prefix(line: &str, token: &str) -> bool {
    let stripped = line.trim_start_matches(|ch: char| {
        ch.is_whitespace() || matches!(ch, '*' | '_' | '`' | '#' | '-' | '>')
    });
    let Some(prefix) = stripped.get(..token.len()) else {
        return false;
    };
    if !prefix.eq_ignore_ascii_case(token) {
        return false;
    }

    summary_verdict_token_is_bounded(&stripped[token.len()..])
}

fn summary_verdict_token_is_bounded(rest: &str) -> bool {
    let mut chars = rest.chars();
    match chars.next() {
        None => true,
        Some(ch) if ch.is_ascii_alphanumeric() || ch == '_' => false,
        Some('-') | Some('/') => chars
            .next()
            .is_none_or(|next| !next.is_ascii_alphanumeric() && next != '_'),
        _ => true,
    }
}
