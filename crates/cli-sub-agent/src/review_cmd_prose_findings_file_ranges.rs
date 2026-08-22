fn parse_file_reference_line(line: &str) -> Option<ReviewFindingFileRange> {
    let line = strip_unordered_list_prefix(line);
    if line.trim_start().starts_with('`') {
        return parse_leading_file_range(line).map(|(range, _)| range);
    }
    let (label, rest) = line
        .split_once('\u{ff1a}')
        .or_else(|| line.split_once(':'))?;
    let label = label.trim();
    if !(label.eq_ignore_ascii_case("file")
        || label.eq_ignore_ascii_case("location")
        || label == "\u{4f4d}\u{7f6e}")
    {
        return None;
    }
    parse_leading_file_range(rest.trim()).map(|(range, _)| range)
}

fn strip_unordered_list_prefix(line: &str) -> &str {
    let trimmed = line.trim_start();
    if matches!(trimmed.as_bytes().first(), Some(b'-' | b'*')) {
        trimmed[1..].trim_start()
    } else {
        trimmed
    }
}

fn parse_leading_file_range(body: &str) -> Option<(ReviewFindingFileRange, String)> {
    let trimmed = body.trim_start();
    let trimmed = if trimmed.starts_with('`') {
        trimmed
    } else {
        trimmed
            .trim_start_matches(|ch: char| {
                matches!(ch, '`' | '(' | '[' | '（' | '）')
            })
            .trim_start()
    };
    let (token, description, quoted) = if trimmed.starts_with('`') {
        let delimiter_len = backtick_run_length(trimmed, 0);
        let content_start = delimiter_len;
        let end = find_closing_backtick(trimmed, content_start, delimiter_len)?;
        (
            &trimmed[content_start..end],
            &trimmed[end + delimiter_len..],
            true,
        )
    } else {
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        (parts.next()?, parts.next().unwrap_or_default(), false)
    };
    let token = if quoted {
        token
    } else {
        token.trim_matches(|ch: char| {
            matches!(ch, '`' | ',' | '.' | ')' | ']' | '（' | '）')
        })
    };
    let description = description
        .trim_start_matches(['-', ':'])
        .trim()
        .to_string();
    let (path, line) = parse_file_reference_token(token)?;
    Some((
        ReviewFindingFileRange {
            path,
            start: line,
            end: None,
        },
        description,
    ))
}

fn parse_embedded_file_range(body: &str) -> Option<ReviewFindingFileRange> {
    for token in body.split(char::is_whitespace) {
        let mut search_start = 0;
        let mut found_span = false;
        while let Some(relative_start) = token[search_start..].find('`') {
            let opening_start = search_start + relative_start;
            let delimiter_len = backtick_run_length(token, opening_start);
            let content_start = opening_start + delimiter_len;
            let Some(end) = find_closing_backtick(token, content_start, delimiter_len) else {
                search_start = content_start;
                continue;
            };
            found_span = true;
            if let Some((path, start)) = parse_file_reference_token(&token[content_start..end]) {
                return Some(ReviewFindingFileRange {
                    path,
                    start,
                    end: None,
                });
            }
            search_start = end + delimiter_len;
        }
        if !found_span {
            let token = token.trim_matches(|ch: char| {
                matches!(
                    ch,
                    '`' | '(' | ')' | '[' | ']' | ',' | '.' | ';' | '（' | '）'
                )
            });
            if let Some((path, start)) = parse_file_reference_token(token) {
                return Some(ReviewFindingFileRange {
                    path,
                    start,
                    end: None,
                });
            }
        }
    }
    None
}

fn backtick_run_length(text: &str, start: usize) -> usize {
    text.as_bytes()[start..]
        .iter()
        .take_while(|&&byte| byte == b'`')
        .count()
}

fn find_closing_backtick(
    text: &str,
    mut search_start: usize,
    delimiter_len: usize,
) -> Option<usize> {
    while let Some(relative_end) = text[search_start..].find('`') {
        let end = search_start + relative_end;
        let closing_len = backtick_run_length(text, end);
        if closing_len == delimiter_len {
            return Some(end);
        }
        search_start = end + closing_len;
    }
    None
}

fn parse_file_reference_token(token: &str) -> Option<(String, u32)> {
    parse_markdown_file_line_token(token).or_else(|| parse_file_line_token(token))
}

fn parse_markdown_file_line_token(token: &str) -> Option<(String, u32)> {
    let token = token.trim_matches(['`', '(', ')', ',', '.', ';']);
    let (label, target) = token.split_once("](")?;
    let label = label.trim_start_matches('[');
    if !looks_like_file_path(label) {
        return None;
    }
    let (_, line) = target.rsplit_once(':')?;
    Some((label.to_string(), parse_line_number(line)?))
}

fn parse_file_line_token(token: &str) -> Option<(String, u32)> {
    let (path, line) = token.rsplit_once(':')?;
    if path.is_empty() || !looks_like_file_path(path) {
        return None;
    }
    Some((path.to_string(), parse_line_number(line)?))
}

fn parse_line_number(value: &str) -> Option<u32> {
    value
        .split([',', '-'])
        .next()?
        .trim_matches(|ch: char| !ch.is_ascii_digit())
        .parse()
        .ok()
}

fn looks_like_file_path(path: &str) -> bool {
    if path.chars().any(char::is_whitespace) {
        return false;
    }
    path.contains('/')
        || path.contains('.')
        || path.eq_ignore_ascii_case("justfile")
        || path == "Makefile"
        || path == "Dockerfile"
}
