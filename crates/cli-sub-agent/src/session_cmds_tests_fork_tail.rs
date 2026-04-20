use super::format_file_size;

// ── format_file_size tests ────────────────────────────────────────

#[test]
fn format_file_size_covers_ranges() {
    assert_eq!(format_file_size(0), "0 B");
    assert_eq!(format_file_size(512), "512 B");
    assert_eq!(format_file_size(1024), "1.0 KB");
    assert_eq!(format_file_size(1536), "1.5 KB");
    assert_eq!(format_file_size(1048576), "1.0 MB");
}

// ── CLI --measure flag parsing ────────────────────────────────────

#[test]
fn session_measure_cli_parses() {
    let cli = Cli::try_parse_from(["csa", "session", "measure", "--session", "01ABCDEF"]).unwrap();
    match cli.command {
        Commands::Session {
            cmd: SessionCommands::Measure { session, json, .. },
        } => {
            assert_eq!(session, "01ABCDEF");
            assert!(!json);
        }
        _ => panic!("expected session measure command"),
    }
}

#[test]
fn session_measure_cli_parses_json_flag() {
    let cli = Cli::try_parse_from([
        "csa",
        "session",
        "measure",
        "--session",
        "01ABCDEF",
        "--json",
    ])
    .unwrap();
    match cli.command {
        Commands::Session {
            cmd: SessionCommands::Measure { json, .. },
        } => {
            assert!(json);
        }
        _ => panic!("expected session measure command"),
    }
}

// ── Fork display tests ────────────────────────────────────────────

fn sample_fork_session() -> MetaSessionState {
    let now = Utc::now();
    MetaSessionState {
        meta_session_id: "01KJ5CFQYE1AAAABBBBCCCCDD".to_string(),
        description: Some("Forked session".to_string()),
        project_path: "/tmp/project".to_string(),
        branch: Some("feat/fork".to_string()),
        created_at: now,
        last_accessed: now,
        csa_version: Some("0.1.450".to_string()),
        genealogy: Genealogy {
            parent_session_id: Some("01KJ5AFQYE9AAAABBBBCCCCDD".to_string()),
            depth: 1,
            fork_of_session_id: Some("01KJ5AFQYE9AAAABBBBCCCCDD".to_string()),
            fork_provider_session_id: Some("provider-session-xyz".to_string()),
        },
        tools: HashMap::new(),
        context_status: ContextStatus::default(),
        total_token_usage: None,
        phase: SessionPhase::Active,
        task_context: TaskContext {
            task_type: Some("run".to_string()),
            tier_name: None,
        },
        turn_count: 0,
        token_budget: None,
        sandbox_info: None,
        termination_reason: None,
        is_seed_candidate: false,
        git_head_at_creation: None,
        pre_session_porcelain: None,
        last_return_packet: None,
        change_id: None,
        spec_id: None,
        fork_call_timestamps: Vec::new(),
        vcs_identity: None,
        identity_version: 1,
    }
}

#[test]
fn session_to_json_includes_fork_fields() {
    let session = sample_fork_session();
    let value = session_to_json(&session);

    assert_eq!(value.get("is_fork").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(
        value.get("fork_of_session_id").and_then(|v| v.as_str()),
        Some("01KJ5AFQYE9AAAABBBBCCCCDD")
    );
    assert_eq!(
        value
            .get("fork_provider_session_id")
            .and_then(|v| v.as_str()),
        Some("provider-session-xyz")
    );
    assert_eq!(
        value.get("parent_session_id").and_then(|v| v.as_str()),
        Some("01KJ5AFQYE9AAAABBBBCCCCDD")
    );
    assert_eq!(value.get("depth").and_then(|v| v.as_u64()), Some(1));
}

#[test]
fn session_to_json_non_fork_has_is_fork_false() {
    let session = sample_session_state();
    let value = session_to_json(&session);

    assert_eq!(value.get("is_fork").and_then(|v| v.as_bool()), Some(false));
    assert!(value.get("fork_of_session_id").is_none());
    assert!(value.get("fork_provider_session_id").is_none());
}

#[test]
fn session_to_json_includes_depth_and_parent() {
    let mut session = sample_session_state();
    session.genealogy.parent_session_id = Some("01PARENT000000000000000000".to_string());
    session.genealogy.depth = 2;

    let value = session_to_json(&session);
    assert_eq!(
        value.get("parent_session_id").and_then(|v| v.as_str()),
        Some("01PARENT000000000000000000")
    );
    assert_eq!(value.get("depth").and_then(|v| v.as_u64()), Some(2));
}
