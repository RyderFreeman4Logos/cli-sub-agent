//! Test for GitHub issue #1636 fix: gate timeout and session retirement

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use chrono::Utc;
    use csa_session::{SessionResult, create_session_fresh, load_session, save_result};
    use tempfile::TempDir;

    #[test]
    fn test_gate_timeout_field_serialization() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path();

        // Create a session
        let session = create_session_fresh(project_root, None, None, None).unwrap();
        let session_id = &session.meta_session_id;

        // Test 1: default gate_timeout should be false
        let now = Utc::now();
        let result_no_timeout = SessionResult {
            status: "success".to_string(),
            exit_code: 0,
            summary: "Test completed".to_string(),
            tool: "test-tool".to_string(),
            original_tool: None,
            fallback_tool: None,
            fallback_reason: None,
            started_at: now,
            completed_at: now,
            events_count: 0,
            artifacts: Vec::new(),
            peak_memory_mb: None,
            fallback_chain: None,
            gate_timeout: false,
            manager_fields: Default::default(),
        };

        // Save and reload to test serialization
        save_result(project_root, session_id, &result_no_timeout).unwrap();
        let loaded_result = csa_session::load_result(project_root, session_id).unwrap().unwrap();
        assert_eq!(loaded_result.gate_timeout, false);

        // Test 2: gate_timeout can be set to true
        let result_with_timeout = SessionResult {
            gate_timeout: true,
            ..result_no_timeout.clone()
        };

        save_result(project_root, session_id, &result_with_timeout).unwrap();
        let loaded_timeout_result = csa_session::load_result(project_root, session_id).unwrap().unwrap();
        assert_eq!(loaded_timeout_result.gate_timeout, true);
    }

    #[test]
    fn test_session_retirement_after_gate_failure() {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path();

        // Create a session in Active state
        let mut session = create_session_fresh(project_root, None, None, None).unwrap();
        assert_eq!(session.phase, csa_session::SessionPhase::Active);

        // Simulate session retirement (what retire_session_after_gate_failure does)
        let phase_result = session.phase.transition(csa_session::PhaseEvent::Complete);
        assert!(phase_result.is_ok());
        session.phase = phase_result.unwrap();
        csa_session::save_session(project_root, &session).unwrap();

        // Verify session is now Retired
        let loaded_session = load_session(project_root, &session.meta_session_id).unwrap().unwrap();
        assert_eq!(loaded_session.phase, csa_session::SessionPhase::Retired);
    }
}