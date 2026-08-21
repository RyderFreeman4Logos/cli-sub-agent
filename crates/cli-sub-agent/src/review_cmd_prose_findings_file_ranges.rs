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
    let trimmed = body
        .trim_start_matches(|ch: char| {
            matches!(ch, '`' | '(' | '[' | '（' | '）')
        })
        .trim_start();
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let token = parts.next()?.trim_matches(|ch: char| {
        matches!(ch, '`' | ',' | '.' | ')' | ']' | '（' | '）')
    });
    let description = parts
        .next()
        .unwrap_or_default()
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
    body.split(char::is_whitespace)
        .map(|token| {
            if let Some(start) = token.find('`')
                && let Some(relative_end) = token[start + 1..].find('`')
            {
                let end = start + 1 + relative_end;
                return &token[start + 1..end];
            }
            token.trim_matches(|ch: char| {
                matches!(
                    ch,
                    '`' | '(' | ')' | '[' | ']' | ',' | '.' | ';' | '（' | '）'
                )
            })
        })
        .find_map(|token| {
            parse_file_reference_token(token).map(|(path, start)| ReviewFindingFileRange {
                path,
                start,
                end: None,
            })
        })
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
