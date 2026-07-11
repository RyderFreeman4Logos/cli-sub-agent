const FALLBACK_LINES: usize = 20;

/// Display a bounded pre-exec result artifact without falling back to raw logs.
pub(super) fn display_pre_exec_summary_if_present(session_dir: &Path, json: bool) -> Result<bool> {
    let unavailable_reason = review_unavailable_reason_label(session_dir);
    pre_exec_summary::display_if_present(session_dir, unavailable_reason.as_deref(), json)
}

pub(super) fn display_structured_output(
    session_dir: &Path,
    session_id: &str,
    opts: &StructuredOutputOpts,
    json: bool,
) -> Result<()> {
    if opts.summary {
        return display_summary_section(session_dir, session_id, json);
    }

    if let Some(ref section_id) = opts.section {
        return display_single_section(session_dir, session_id, section_id, json);
    }

    if opts.full {
        return display_all_sections(session_dir, session_id, json);
    }

    Ok(())
}

/// Show only the summary section, with fallback to first N lines of output.log.
pub(super) fn display_summary_section(
    session_dir: &Path,
    session_id: &str,
    json: bool,
) -> Result<()> {
    let unavailable_reason = review_unavailable_reason_label(session_dir);
    let post_exec_gate = load_structured_post_exec_gate_report(session_dir);
    let provider_quota = provider_quota_display_for_session_dir(session_dir);
    let (section_id, content) = match csa_session::read_section(session_dir, "summary")? {
        Some(content) => ("summary", content),
        None => match csa_session::read_section(session_dir, "full")? {
            Some(content) => ("full", content),
            None => {
                if let Some(report) = post_exec_gate.as_ref() {
                    return display_gate_summary_override(report, None, unavailable_reason, json);
                }
                if let Some(provider_quota) = provider_quota.as_ref() {
                    return display_provider_quota_summary(
                        provider_quota,
                        unavailable_reason,
                        json,
                    );
                }
                return display_summary_fallback(session_dir, session_id, json);
            }
        },
    };

    if let Some(report) = post_exec_gate.as_ref() {
        return display_gate_summary_override(
            report,
            gate_summary_employee_section(section_id, content.as_str()),
            unavailable_reason,
            json,
        );
    }

    if json {
        if let (true, Some(provider_quota)) = (section_id == "full", provider_quota.as_ref()) {
            return display_provider_quota_summary(provider_quota, unavailable_reason, json);
        }
        let payload = serde_json::json!({
            "section": section_id,
            "content": content,
            "tokens": csa_session::estimate_tokens(&content),
            "unavailable_reason": unavailable_reason,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else if section_id == "full" {
        if let Some(provider_quota) = provider_quota.as_ref() {
            return display_provider_quota_summary(provider_quota, unavailable_reason, json);
        }
        print_truncated_content(&content, FALLBACK_LINES);
        print_unavailable_reason(unavailable_reason.as_deref());
    } else {
        println!("{content}");
        print_unavailable_reason(unavailable_reason.as_deref());
    }
    Ok(())
}

fn display_gate_summary_override(
    report: &csa_session::PostExecGateReport,
    employee_section: Option<(&str, &str)>,
    unavailable_reason: Option<String>,
    json: bool,
) -> Result<()> {
    if json {
        let payload =
            build_summary_section_json_payload(employee_section, unavailable_reason, Some(report))?;
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        println!(
            "{}",
            build_gate_aware_summary_content(report, employee_section)
        );
        print_unavailable_reason(unavailable_reason.as_deref());
    }
    Ok(())
}

fn display_provider_quota_summary(
    provider_quota: &ProviderQuotaDisplay,
    unavailable_reason: Option<String>,
    json: bool,
) -> Result<()> {
    let content = format!("{}\nHint: {}", provider_quota.summary, provider_quota.hint);
    if json {
        let payload = serde_json::json!({
            "section": "summary",
            "source": "provider_quota",
            "content": content,
            "tokens": csa_session::estimate_tokens(&provider_quota.summary),
            "unavailable_reason": unavailable_reason,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        println!("{content}");
        print_unavailable_reason(unavailable_reason.as_deref());
    }
    Ok(())
}

fn display_summary_fallback(session_dir: &Path, session_id: &str, json: bool) -> Result<()> {
    let unavailable_reason = review_unavailable_reason_label(session_dir);
    if let Some(provider_quota) = provider_quota_display_for_session_dir(session_dir) {
        return display_provider_quota_summary(&provider_quota, unavailable_reason, json);
    }
    let output_log = session_dir.join("output.log");
    if output_log.is_file() {
        let content = fs::read_to_string(&output_log)?;
        if !content.is_empty() {
            if json {
                let payload = serde_json::json!({
                    "section": "summary",
                    "source": "output.log",
                    "content": content.lines().take(FALLBACK_LINES).collect::<Vec<_>>().join("\n"),
                    "truncated": content.lines().count() > FALLBACK_LINES,
                    "unavailable_reason": unavailable_reason,
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                print_truncated_content(&content, FALLBACK_LINES);
                print_unavailable_reason(unavailable_reason.as_deref());
            }
            return Ok(());
        }
    }
    if let Some(reason) = unavailable_reason.as_deref() {
        if json {
            let payload = serde_json::json!({
                "section": "summary",
                "content": serde_json::Value::Null,
                "unavailable_reason": reason,
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
        } else {
            print_unavailable_reason(Some(reason));
        }
        return Ok(());
    }
    if pre_exec_summary::display_if_present(session_dir, unavailable_reason.as_deref(), json)? {
        return Ok(());
    }
    eprintln!("No output found for session '{session_id}'");
    Ok(())
}

fn print_unavailable_reason(reason: Option<&str>) {
    if let Some(reason) = reason {
        println!("Unavailable reason: {reason}");
    }
}

fn print_truncated_content(content: &str, max_lines: usize) {
    let lines: Vec<&str> = content.lines().take(max_lines).collect();
    println!("{}", lines.join("\n"));
    if content.lines().count() > max_lines {
        eprintln!(
            "... ({} more lines, use --full to see all)",
            content.lines().count() - max_lines
        );
    }
}

/// Show a single section by ID.
pub(super) fn display_single_section(
    session_dir: &Path,
    session_id: &str,
    section_id: &str,
    json: bool,
) -> Result<()> {
    match csa_session::read_section(session_dir, section_id)? {
        Some(content) => {
            if json {
                let payload = serde_json::json!({
                    "section": section_id,
                    "content": content,
                    "tokens": csa_session::estimate_tokens(&content),
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                println!("{content}");
            }
        }
        None => match csa_session::load_output_index(session_dir)? {
            Some(index) => {
                let available: Vec<&str> = index.sections.iter().map(|s| s.id.as_str()).collect();
                anyhow::bail!(
                    "Section '{}' not found in session '{}'. Available sections: {}",
                    section_id,
                    session_id,
                    available.join(", ")
                );
            }
            None => {
                anyhow::bail!(
                    "No structured output for session '{session_id}'. Run without --section to see raw result."
                );
            }
        },
    }
    Ok(())
}

/// Show all sections in index order.
pub(super) fn display_all_sections(session_dir: &Path, session_id: &str, json: bool) -> Result<()> {
    let sections = csa_session::read_all_sections(session_dir)?;
    let post_exec_gate = load_structured_post_exec_gate_report(session_dir);
    if sections.is_empty() {
        let output_log = session_dir.join("output.log");
        if output_log.is_file() {
            let content = fs::read_to_string(&output_log)?;
            if !content.is_empty() {
                if json {
                    let payload = if post_exec_gate.is_some() {
                        build_all_sections_json_payload(&rendered_fallback_sections(
                            content.as_str(),
                            post_exec_gate.as_ref(),
                        ))?
                    } else {
                        serde_json::json!({
                            "sections": [{
                                "section": "full",
                                "content": content,
                                "tokens": csa_session::estimate_tokens(&content),
                            }]
                        })
                    };
                    println!("{}", serde_json::to_string_pretty(&payload)?);
                } else {
                    if let Some(report) = post_exec_gate.as_ref() {
                        print_rendered_sections(&[gate_failure_rendered_section(report)]);
                        println!();
                    }
                    print!("{content}");
                }
                return Ok(());
            }
        }
        if let Some(report) = post_exec_gate.as_ref() {
            let gate_section = gate_failure_rendered_section(report);
            if json {
                let payload = build_all_sections_json_payload(&[gate_section])?;
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                print_rendered_sections(&[gate_section]);
            }
            return Ok(());
        }
        eprintln!("No output found for session '{session_id}'");
        return Ok(());
    }

    let rendered_sections = structured_sections_with_gate_first(&sections, post_exec_gate.as_ref());
    if json {
        let payload = build_all_sections_json_payload(&rendered_sections)?;
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        print_rendered_sections(&rendered_sections);
    }
    Ok(())
}
