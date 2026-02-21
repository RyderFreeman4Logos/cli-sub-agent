    // NOTE: CSA_SUPPRESS_NOTIFY is injected by the pipeline layer (not transport)
    // based on per-tool config via extra_env. See pipeline.rs suppress_notify logic.
    #[test]
    fn test_acp_build_env_propagates_extra_env() {
        let transport = AcpTransport::new("claude-code", None);
        let now = chrono::Utc::now();
        let session = csa_session::state::MetaSessionState {
            meta_session_id: "01HTEST000000000000000000".to_string(),
            description: Some("test".to_string()),
            project_path: "/tmp/test".to_string(),
            branch: None,
            created_at: now,
            last_accessed: now,
            genealogy: csa_session::state::Genealogy {
                parent_session_id: None,
                depth: 0,
            },
            tools: HashMap::new(),
            context_status: csa_session::state::ContextStatus::default(),
            total_token_usage: None,
            phase: csa_session::state::SessionPhase::Active,
            task_context: csa_session::state::TaskContext::default(),
            turn_count: 0,
            token_budget: None,
            sandbox_info: None,

            termination_reason: None,
        };

        let mut extra = HashMap::new();
        extra.insert("CSA_SUPPRESS_NOTIFY".to_string(), "1".to_string());
        let env = transport.build_env(&session, Some(&extra));
        assert_eq!(
            env.get("CSA_SUPPRESS_NOTIFY"),
            Some(&"1".to_string()),
            "ACP transport should propagate CSA_SUPPRESS_NOTIFY from extra_env"
        );

        // Without extra_env, suppress_notify should NOT be present.
        let env_no_extra = transport.build_env(&session, None);
        assert_eq!(
            env_no_extra.get("CSA_SUPPRESS_NOTIFY"),
            None,
            "ACP transport should not inject CSA_SUPPRESS_NOTIFY on its own"
        );
    }

    #[test]
    fn test_acp_build_env_includes_csa_session_dir() {
        let transport = AcpTransport::new("claude-code", None);
        let now = chrono::Utc::now();
        let session = csa_session::state::MetaSessionState {
            meta_session_id: "01HTEST000000000000000000".to_string(),
            description: Some("test".to_string()),
            project_path: "/tmp/test".to_string(),
            branch: None,
            created_at: now,
            last_accessed: now,
            genealogy: csa_session::state::Genealogy {
                parent_session_id: None,
                depth: 0,
            },
            tools: HashMap::new(),
            context_status: csa_session::state::ContextStatus::default(),
            total_token_usage: None,
            phase: csa_session::state::SessionPhase::Active,
            task_context: csa_session::state::TaskContext::default(),
            turn_count: 0,
            token_budget: None,
            sandbox_info: None,

            termination_reason: None,
        };

        let env = transport.build_env(&session, None);
        let session_dir = env
            .get("CSA_SESSION_DIR")
            .expect("CSA_SESSION_DIR should be present in env");
        assert!(
            session_dir.contains("/sessions/"),
            "CSA_SESSION_DIR should contain /sessions/ path segment, got: {session_dir}"
        );
        assert!(
            session_dir.contains("01HTEST000000000000000000"),
            "CSA_SESSION_DIR should contain the session ID, got: {session_dir}"
        );
    }

    #[test]
    fn test_resume_session_id_extraction() {
        let now = chrono::Utc::now();
        let tool_state = ToolState {
            provider_session_id: Some("test-session-123".to_string()),
            last_action_summary: String::new(),
            last_exit_code: 0,
            updated_at: now,
            token_usage: None,
        };
        let resume_id = tool_state.provider_session_id.as_deref();
        assert_eq!(resume_id, Some("test-session-123"));
    }

    #[test]
    fn test_resume_session_id_none_when_absent() {
        let now = chrono::Utc::now();
        let tool_state = ToolState {
            provider_session_id: None,
            last_action_summary: String::new(),
            last_exit_code: 0,
            updated_at: now,
            token_usage: None,
        };
        let resume_id = tool_state.provider_session_id.as_deref();
        assert!(resume_id.is_none());
    }
