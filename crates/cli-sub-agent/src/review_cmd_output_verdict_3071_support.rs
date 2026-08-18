fn expand_unicode_escapes(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut expanded = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        if chars[index] != '\\' {
            expanded.push(chars[index]);
            index += 1;
            continue;
        }
        let mut cursor = index;
        while cursor < chars.len() && chars[cursor] == '\\' {
            cursor += 1;
        }
        if chars.get(cursor) == Some(&'u') && chars.get(cursor + 1) == Some(&'{') {
            let Some(end) = chars[cursor + 2..].iter().position(|ch| *ch == '}') else {
                expanded.extend(&chars[index..cursor]);
                index = cursor;
                continue;
            };
            let end = cursor + 2 + end;
            if let Ok(codepoint) = chars[cursor + 2..end].iter().collect::<String>().parse::<u32>() {
                if let Some(character) = char::from_u32(codepoint) {
                    expanded.push(character);
                    index = end + 1;
                    continue;
                }
            }
        }
        expanded.push('\\');
        index += 1;
    }
    expanded
}

fn persist_fixture_review(
    test_name: &str,
    session_id: &str,
    summary: &str,
    details: &str,
    findings_toml: &str,
    extracted_marker: bool,
) -> (OwnedMutexGuard<()>, PathBuf, PathBuf) {
    let summary = expand_unicode_escapes(summary);
    let details = expand_unicode_escapes(details);
    let findings_toml = expand_unicode_escapes(findings_toml);
    let (env_lock, project_root, session_dir) = lock_test_session(test_name, session_id);
    csa_session::persist_structured_output(
        &session_dir,
        &format!(
            "<!-- CSA:SECTION:summary -->\n{summary}\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:details -->\n{details}\n<!-- CSA:SECTION:details:END -->\n"
        ),
    )
    .expect("persist fixture review prose");
    fs::write(
        session_dir.join("output").join("findings.toml"),
        findings_toml,
    )
    .expect("write fixture findings.toml");
    if extracted_marker {
        fs::write(
            session_dir
                .join("output")
                .join(crate::review_cmd::findings_toml::FINDINGS_TOML_EXTRACTED_MARKER),
            b"",
        )
        .expect("write extracted marker");
    }
    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Fail, "HAS_ISSUES");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());
    (env_lock, project_root, session_dir)
}
