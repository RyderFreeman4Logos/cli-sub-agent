use csa_session::{
    ChangedFile, FileAction, RETURN_PACKET_MAX_SUMMARY_CHARS, RETURN_PACKET_SECTION_ID,
    ReturnPacket, ReturnStatus, parse_return_packet, persist_structured_output, read_section,
};

fn section_start(id: &str) -> String {
    format!("<!-- CSA:SECTION:{id} -->")
}

fn section_end(id: &str) -> String {
    format!("<!-- CSA:SECTION:{id}:END -->")
}

fn write_sections_to_tempdir(sections: &[(&str, &str)]) -> tempfile::TempDir {
    let tempdir = tempfile::tempdir().expect("create tempdir for session");
    let mut output = String::new();
    for (id, content) in sections {
        output.push_str(&section_start(id));
        output.push('\n');
        output.push_str(content);
        output.push('\n');
        output.push_str(&section_end(id));
        output.push('\n');
    }

    persist_structured_output(tempdir.path(), &output)
        .expect("persist structured output for return packet contract");
    tempdir
}

#[test]
fn test_return_packet_happy_path_round_trip_via_structured_output_pipeline() {
    let expected = ReturnPacket {
        status: ReturnStatus::Success,
        exit_code: 0,
        summary: "Child execution completed".to_string(),
        artifacts: vec!["logs/run.log".to_string(), "output/report.md".to_string()],
        changed_files: vec![
            ChangedFile {
                path: "src/main.rs".to_string(),
                action: FileAction::Modify,
            },
            ChangedFile {
                path: "src/new_file.rs".to_string(),
                action: FileAction::Add,
            },
        ],
        git_head_before: Some("abc123".to_string()),
        git_head_after: Some("def456".to_string()),
        next_actions: vec!["run tests".to_string(), "open PR".to_string()],
        error_context: None,
    };

    let return_packet_toml =
        toml::to_string(&expected).expect("serialize return packet to toml for section payload");
    let tempdir = write_sections_to_tempdir(&[(RETURN_PACKET_SECTION_ID, &return_packet_toml)]);

    let payload = read_section(tempdir.path(), RETURN_PACKET_SECTION_ID)
        .expect("read return packet section")
        .expect("return packet section exists");
    let actual = parse_return_packet(&payload).expect("parse return packet from stored section");

    assert_eq!(actual, expected);
    assert!(actual.validate().is_ok());
}

#[test]
fn test_return_packet_malformed_toml_returns_failure_packet_with_error_context() {
    let malformed = r#"
status = "Success"
exit_code = [not, valid, toml
"#;

    let packet = parse_return_packet(malformed).expect("parse should degrade to failure packet");

    assert_eq!(packet.status, ReturnStatus::Failure);
    assert_eq!(packet.exit_code, 1);
    assert!(
        packet.error_context.is_some(),
        "malformed payload should preserve parse failure reason"
    );
}

#[test]
fn test_return_packet_missing_fields_degrades_to_defaults() {
    let tempdir =
        write_sections_to_tempdir(&[(RETURN_PACKET_SECTION_ID, "status = \"Cancelled\"")]);
    let payload = read_section(tempdir.path(), RETURN_PACKET_SECTION_ID)
        .expect("read return packet section")
        .expect("return packet section exists");

    let packet = parse_return_packet(&payload).expect("parse partial return packet payload");

    assert_eq!(packet.status, ReturnStatus::Cancelled);
    assert_eq!(packet.exit_code, 1);
    assert!(packet.summary.is_empty());
    assert!(packet.artifacts.is_empty());
    assert!(packet.changed_files.is_empty());
    assert!(packet.next_actions.is_empty());
}

#[test]
fn test_return_packet_oversized_summary_is_truncated_by_sanitize_summary() {
    let oversized = "x".repeat(RETURN_PACKET_MAX_SUMMARY_CHARS + 512);
    let content = format!(
        r#"
status = "Success"
exit_code = 0
summary = "{oversized}"
"#
    );

    let packet = parse_return_packet(&content).expect("parse packet with oversized summary");

    assert_eq!(packet.status, ReturnStatus::Success);
    assert_eq!(
        packet.summary.chars().count(),
        RETURN_PACKET_MAX_SUMMARY_CHARS
    );
    assert_eq!(packet.summary, "x".repeat(RETURN_PACKET_MAX_SUMMARY_CHARS));
}

#[test]
fn test_return_packet_validate_rejects_path_traversal_in_changed_files() {
    let packet = ReturnPacket {
        status: ReturnStatus::Success,
        exit_code: 0,
        summary: "attempted traversal".to_string(),
        changed_files: vec![ChangedFile {
            path: "../secret.txt".to_string(),
            action: FileAction::Modify,
        }],
        ..ReturnPacket::default()
    };

    let err = packet
        .validate()
        .expect_err("validate should reject parent-directory traversal");
    assert!(err.to_string().contains("repo-relative"));
}

#[test]
fn test_return_packet_empty_section_degrades_to_failure_packet_through_pipeline() {
    let tempdir = write_sections_to_tempdir(&[(RETURN_PACKET_SECTION_ID, "")]);
    let payload = read_section(tempdir.path(), RETURN_PACKET_SECTION_ID)
        .expect("read return packet section")
        .expect("return packet section exists");
    assert!(payload.trim().is_empty());

    let packet = parse_return_packet(&payload).expect("parse should degrade to failure packet");
    assert_eq!(packet.status, ReturnStatus::Failure);
    assert_eq!(packet.exit_code, 1);
    assert!(packet.summary.is_empty());
    assert!(packet.changed_files.is_empty());
    assert!(packet.error_context.is_none());
}

#[test]
fn test_return_packet_path_traversal_in_section_fails_validation_through_pipeline() {
    let packet_with_traversal = ReturnPacket {
        status: ReturnStatus::Success,
        exit_code: 0,
        summary: "attempt traversal".to_string(),
        changed_files: vec![ChangedFile {
            path: "../secret.txt".to_string(),
            action: FileAction::Modify,
        }],
        ..ReturnPacket::default()
    };

    let return_packet_toml = toml::to_string(&packet_with_traversal)
        .expect("serialize traversal packet to toml for section payload");
    let tempdir = write_sections_to_tempdir(&[(RETURN_PACKET_SECTION_ID, &return_packet_toml)]);
    let payload = read_section(tempdir.path(), RETURN_PACKET_SECTION_ID)
        .expect("read return packet section")
        .expect("return packet section exists");

    let packet = parse_return_packet(&payload)
        .expect("invalid changed_files path should degrade to failure packet");
    assert_eq!(packet.status, ReturnStatus::Failure);
    assert_eq!(packet.exit_code, 1);
    assert!(packet.changed_files.is_empty());
    assert!(
        packet
            .error_context
            .as_deref()
            .is_some_and(|err| err.contains("validation failed")),
        "error context should include validation failure reason"
    );
}

#[test]
fn test_return_packet_isolated_when_multiple_sections_exist() {
    let expected = ReturnPacket {
        status: ReturnStatus::Success,
        exit_code: 0,
        summary: "only return section data".to_string(),
        changed_files: vec![ChangedFile {
            path: "src/lib.rs".to_string(),
            action: FileAction::Modify,
        }],
        ..ReturnPacket::default()
    };
    let return_packet_toml =
        toml::to_string(&expected).expect("serialize return packet for multi-section output");

    let tempdir = write_sections_to_tempdir(&[
        ("summary", "human-readable summary for caller"),
        (
            "details",
            "implementation details that must not leak into return packet",
        ),
        (RETURN_PACKET_SECTION_ID, &return_packet_toml),
    ]);

    let summary = read_section(tempdir.path(), "summary")
        .expect("read summary section")
        .expect("summary section exists");
    let details = read_section(tempdir.path(), "details")
        .expect("read details section")
        .expect("details section exists");
    let return_payload = read_section(tempdir.path(), RETURN_PACKET_SECTION_ID)
        .expect("read return packet section")
        .expect("return packet section exists");

    assert!(summary.contains("human-readable summary"));
    assert!(details.contains("implementation details"));
    assert!(!return_payload.contains("human-readable summary"));
    assert!(!return_payload.contains("implementation details"));

    let parsed =
        parse_return_packet(&return_payload).expect("parse isolated return packet section");
    assert_eq!(parsed, expected);
}
