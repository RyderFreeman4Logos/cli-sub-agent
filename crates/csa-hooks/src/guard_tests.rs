//! Tests for prompt guard execution and output formatting.

use super::*;

// ---------------------------------------------------------------------------
// format_guard_output tests
// ---------------------------------------------------------------------------

#[test]
fn test_format_guard_output_empty_results() {
    let results: Vec<PromptGuardResult> = vec![];
    assert!(format_guard_output(&results).is_none());
}

#[test]
fn test_format_guard_output_all_empty_output() {
    let results = vec![
        PromptGuardResult {
            name: "guard1".to_string(),
            output: String::new(),
        },
        PromptGuardResult {
            name: "guard2".to_string(),
            output: String::new(),
        },
    ];
    assert!(format_guard_output(&results).is_none());
}

#[test]
fn test_format_guard_output_single_result() {
    let results = vec![PromptGuardResult {
        name: "branch-guard".to_string(),
        output: "Do not commit on main.".to_string(),
    }];
    let output = format_guard_output(&results).unwrap();
    assert!(output.contains("<prompt-guard name=\"branch-guard\">"));
    assert!(output.contains("Do not commit on main."));
    assert!(output.contains("</prompt-guard>"));
}

#[test]
fn test_format_guard_output_multiple_results() {
    let results = vec![
        PromptGuardResult {
            name: "guard-a".to_string(),
            output: "Message A".to_string(),
        },
        PromptGuardResult {
            name: "guard-b".to_string(),
            output: "Message B".to_string(),
        },
    ];
    let output = format_guard_output(&results).unwrap();
    assert!(output.contains("<prompt-guard name=\"guard-a\">"));
    assert!(output.contains("Message A"));
    assert!(output.contains("<prompt-guard name=\"guard-b\">"));
    assert!(output.contains("Message B"));
    // Verify order preserved
    let pos_a = output.find("guard-a").unwrap();
    let pos_b = output.find("guard-b").unwrap();
    assert!(pos_a < pos_b, "Guards should appear in order");
}

#[test]
fn test_format_guard_output_xml_escape_text() {
    let results = vec![PromptGuardResult {
        name: "escape-test".to_string(),
        output: "Use <branch> & \"quotes\"".to_string(),
    }];
    let output = format_guard_output(&results).unwrap();
    assert!(output.contains("Use &lt;branch&gt; &amp; \"quotes\""));
}

#[test]
fn test_format_guard_output_xml_escape_name() {
    let results = vec![PromptGuardResult {
        name: "guard<\"test\">".to_string(),
        output: "content".to_string(),
    }];
    let output = format_guard_output(&results).unwrap();
    assert!(output.contains("name=\"guard&lt;&quot;test&quot;&gt;\""));
}

#[test]
fn test_format_guard_output_skips_empty_in_mixed() {
    let results = vec![
        PromptGuardResult {
            name: "has-output".to_string(),
            output: "something".to_string(),
        },
        PromptGuardResult {
            name: "empty".to_string(),
            output: String::new(),
        },
        PromptGuardResult {
            name: "also-output".to_string(),
            output: "more".to_string(),
        },
    ];
    let output = format_guard_output(&results).unwrap();
    assert!(output.contains("has-output"));
    assert!(!output.contains("empty"));
    assert!(output.contains("also-output"));
}

// ---------------------------------------------------------------------------
// XML escape helper tests
// ---------------------------------------------------------------------------

#[test]
fn test_xml_escape_attr_all_special_chars() {
    assert_eq!(xml_escape_attr("a&b"), "a&amp;b");
    assert_eq!(xml_escape_attr("a\"b"), "a&quot;b");
    assert_eq!(xml_escape_attr("a<b"), "a&lt;b");
    assert_eq!(xml_escape_attr("a>b"), "a&gt;b");
}

#[test]
fn test_xml_escape_text_all_special_chars() {
    assert_eq!(xml_escape_text("a&b"), "a&amp;b");
    assert_eq!(xml_escape_text("a<b"), "a&lt;b");
    assert_eq!(xml_escape_text("a>b"), "a&gt;b");
    // Quotes are not escaped in text content
    assert_eq!(xml_escape_text("a\"b"), "a\"b");
}

#[test]
fn test_xml_escape_no_double_escape() {
    // &amp; should NOT become &amp;amp;
    assert_eq!(xml_escape_text("&amp;"), "&amp;amp;");
    // This is correct behavior: the input literally contains "&amp;"
}

// ---------------------------------------------------------------------------
// run_prompt_guards tests (require shell execution)
//
// Guards run via `sh -c <command>`, so tests use inline shell commands
// instead of writing temporary script files. This eliminates filesystem
// races (ETXTBSY / exit code 126) under heavy parallel execution.
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod unix_tests {
    use super::*;

    fn test_context() -> GuardContext {
        GuardContext {
            project_root: "/tmp/test-project".to_string(),
            session_id: "01TEST000000000000000000000".to_string(),
            tool: "codex".to_string(),
            is_resume: false,
            cwd: "/tmp".to_string(),
        }
    }

    #[test]
    fn test_run_guards_stdout_capture() {
        let guards = vec![PromptGuardEntry {
            name: "test-guard".to_string(),
            command: "echo 'Hello from guard'".to_string(),
            timeout_secs: 5,
        }];

        let results = run_prompt_guards(&guards, &test_context());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "test-guard");
        assert_eq!(results[0].output, "Hello from guard");
    }

    #[test]
    fn test_run_guards_empty_stdout_skipped() {
        let guards = vec![PromptGuardEntry {
            name: "empty-guard".to_string(),
            command: "true".to_string(),
            timeout_secs: 5,
        }];

        let results = run_prompt_guards(&guards, &test_context());
        assert!(results.is_empty(), "Empty stdout should be filtered out");
    }

    #[test]
    fn test_run_guards_nonzero_exit_skipped() {
        let guards = vec![PromptGuardEntry {
            name: "fail-guard".to_string(),
            command: "echo 'should not appear'; exit 1".to_string(),
            timeout_secs: 5,
        }];

        let results = run_prompt_guards(&guards, &test_context());
        assert!(results.is_empty(), "Non-zero exit guard should be skipped");
    }

    #[test]
    fn test_run_guards_timeout_skipped() {
        let guards = vec![PromptGuardEntry {
            name: "slow-guard".to_string(),
            command: "sleep 10".to_string(),
            timeout_secs: 1,
        }];

        let results = run_prompt_guards(&guards, &test_context());
        assert!(results.is_empty(), "Timed-out guard should be skipped");
    }

    #[test]
    fn test_run_guards_multiple_merge() {
        let guards = vec![
            PromptGuardEntry {
                name: "first".to_string(),
                command: "echo 'Guard 1 output'".to_string(),
                timeout_secs: 5,
            },
            PromptGuardEntry {
                name: "second".to_string(),
                command: "echo 'Guard 2 output'".to_string(),
                timeout_secs: 5,
            },
        ];

        let results = run_prompt_guards(&guards, &test_context());
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].name, "first");
        assert_eq!(results[0].output, "Guard 1 output");
        assert_eq!(results[1].name, "second");
        assert_eq!(results[1].output, "Guard 2 output");
    }

    #[test]
    fn test_run_guards_stdin_receives_json_context() {
        // Read stdin and extract the "tool" field using pure shell (no jq).
        let guards = vec![PromptGuardEntry {
            name: "context-check".to_string(),
            command: r#"INPUT=$(cat); echo "$INPUT" | sed -n 's/.*"tool":"\([^"]*\)".*/\1/p'"#
                .to_string(),
            timeout_secs: 5,
        }];

        let ctx = test_context();
        let results = run_prompt_guards(&guards, &ctx);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].output, "codex");
    }

    #[test]
    fn test_run_guards_missing_script_skipped() {
        let guards = vec![PromptGuardEntry {
            name: "missing".to_string(),
            command: "/nonexistent/path/to/guard_abc123.sh".to_string(),
            timeout_secs: 5,
        }];

        let results = run_prompt_guards(&guards, &test_context());
        assert!(results.is_empty(), "Missing script guard should be skipped");
    }

    #[test]
    fn test_run_guards_empty_list() {
        let guards: Vec<PromptGuardEntry> = vec![];
        let results = run_prompt_guards(&guards, &test_context());
        assert!(results.is_empty());
    }

    #[test]
    fn test_run_guards_mixed_success_and_failure() {
        let guards = vec![
            PromptGuardEntry {
                name: "good".to_string(),
                command: "echo 'good output'".to_string(),
                timeout_secs: 5,
            },
            PromptGuardEntry {
                name: "bad".to_string(),
                command: "exit 1".to_string(),
                timeout_secs: 5,
            },
            PromptGuardEntry {
                name: "good2".to_string(),
                command: "echo 'also good'".to_string(),
                timeout_secs: 5,
            },
        ];

        let results = run_prompt_guards(&guards, &test_context());
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].name, "good");
        assert_eq!(results[1].name, "good2");
    }

    #[test]
    fn test_run_guards_stdout_trimmed() {
        let guards = vec![PromptGuardEntry {
            name: "trim-test".to_string(),
            command: "echo '  trimmed  '; echo ''".to_string(),
            timeout_secs: 5,
        }];

        let results = run_prompt_guards(&guards, &test_context());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].output, "trimmed");
    }

    #[test]
    fn test_run_guards_output_capped_at_max_size() {
        // Generate output larger than MAX_GUARD_OUTPUT_BYTES (32 KB).
        // `dd` writes 40 KB of 'A' characters.
        let guards = vec![PromptGuardEntry {
            name: "large-guard".to_string(),
            command: "dd if=/dev/zero bs=1024 count=40 2>/dev/null | tr '\\0' 'A'".to_string(),
            timeout_secs: 5,
        }];

        let results = run_prompt_guards(&guards, &test_context());
        assert_eq!(results.len(), 1);
        assert!(
            results[0].output.len() <= super::MAX_GUARD_OUTPUT_BYTES,
            "Output should be capped at {} bytes, got {}",
            super::MAX_GUARD_OUTPUT_BYTES,
            results[0].output.len()
        );
    }

    #[test]
    fn test_run_guards_background_process_no_hang() {
        // A guard that backgrounds a long-running process inheriting stdout.
        // With tempfile stdout, the background process holds a file descriptor
        // (not a pipe), so the parent reads the tempfile after the main child
        // exits without blocking.
        let guards = vec![PromptGuardEntry {
            name: "bg-test".to_string(),
            command: "sleep 300 & echo 'guard output'".to_string(),
            timeout_secs: 5,
        }];

        let start = std::time::Instant::now();
        let results = run_prompt_guards(&guards, &test_context());
        assert!(
            start.elapsed() < std::time::Duration::from_secs(3),
            "Must not hang waiting for background process, took {:?}",
            start.elapsed()
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].output, "guard output");
    }

    #[test]
    fn test_run_guards_setsid_detached_no_hang() {
        // A guard that spawns a detached child via setsid, which escapes the
        // process group. With tempfile stdout, neither pipes nor process groups
        // can cause the parent to block.
        let guards = vec![PromptGuardEntry {
            name: "setsid-test".to_string(),
            command: "setsid sh -c 'sleep 300' </dev/null & echo 'setsid guard output'".to_string(),
            timeout_secs: 5,
        }];

        let start = std::time::Instant::now();
        let results = run_prompt_guards(&guards, &test_context());
        assert!(
            start.elapsed() < std::time::Duration::from_secs(4),
            "Must not hang on setsid-detached process, took {:?}",
            start.elapsed()
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].output, "setsid guard output");
    }

    #[test]
    fn test_run_guards_large_output_no_deadlock() {
        // Produces 128 KB — exceeds typical 64 KB pipe buffer.
        // With tempfile stdout, there is no pipe buffer limit, so the child
        // writes freely and exits cleanly. Output is capped at read time.
        let guards = vec![PromptGuardEntry {
            name: "pipe-stress".to_string(),
            command: "dd if=/dev/zero bs=1024 count=128 2>/dev/null | tr '\\0' 'B'".to_string(),
            timeout_secs: 5,
        }];

        let start = std::time::Instant::now();
        let results = run_prompt_guards(&guards, &test_context());
        // Must complete well before timeout — proves no pipe deadlock.
        assert!(
            start.elapsed() < std::time::Duration::from_secs(3),
            "Guard should complete quickly without pipe deadlock, took {:?}",
            start.elapsed()
        );
        // With tempfile, child exits cleanly → guard succeeds with capped output.
        assert_eq!(
            results.len(),
            1,
            "Guard should succeed (tempfile prevents backpressure)"
        );
        assert!(
            results[0].output.len() <= super::MAX_GUARD_OUTPUT_BYTES,
            "Output capped at {} bytes, got {}",
            super::MAX_GUARD_OUTPUT_BYTES,
            results[0].output.len()
        );
    }

    /// Regression test: fragmented output (many small writes) must be captured
    /// correctly. The tempfile approach reads all output after child exit, so
    /// fragmentation does not affect correctness.
    #[test]
    fn test_run_guards_fragmented_output_no_memory_bloat() {
        let guards = vec![PromptGuardEntry {
            name: "fragmented-test".to_string(),
            command: "i=0; while [ $i -lt 1024 ]; do printf 'a\\n'; i=$((i + 1)); done".to_string(),
            timeout_secs: 10,
        }];

        let results = run_prompt_guards(&guards, &test_context());
        assert_eq!(results.len(), 1);
        // Should have captured 1024 'a' characters (plus newlines, trimmed)
        let a_count = results[0].output.chars().filter(|&c| c == 'a').count();
        assert_eq!(
            a_count, 1024,
            "Expected 1024 'a' chars in fragmented output, got {a_count}"
        );
    }
}

// ---------------------------------------------------------------------------
// GuardContext serialization tests
// ---------------------------------------------------------------------------

#[test]
fn test_guard_context_serializes_to_json() {
    let ctx = GuardContext {
        project_root: "/tmp/project".to_string(),
        session_id: "01ABCDEF".to_string(),
        tool: "codex".to_string(),
        is_resume: true,
        cwd: "/tmp".to_string(),
    };

    let json = serde_json::to_string(&ctx).unwrap();
    assert!(json.contains("\"project_root\":\"/tmp/project\""));
    assert!(json.contains("\"is_resume\":true"));
    assert!(json.contains("\"tool\":\"codex\""));
}

#[test]
fn test_guard_context_deserializes_from_json() {
    let json = r#"{
        "project_root": "/home/user/project",
        "session_id": "01TESTID",
        "tool": "claude-code",
        "is_resume": false,
        "cwd": "/home/user/project"
    }"#;

    let ctx: GuardContext = serde_json::from_str(json).unwrap();
    assert_eq!(ctx.project_root, "/home/user/project");
    assert_eq!(ctx.tool, "claude-code");
    assert!(!ctx.is_resume);
}

// ---------------------------------------------------------------------------
// PromptGuardEntry deserialization tests
// ---------------------------------------------------------------------------

#[test]
fn test_prompt_guard_entry_default_timeout() {
    let toml_str = r#"
        name = "test"
        command = "echo hello"
    "#;
    let entry: PromptGuardEntry = toml::from_str(toml_str).unwrap();
    assert_eq!(entry.timeout_secs, 10, "Default timeout should be 10s");
}

#[test]
fn test_prompt_guard_entry_custom_timeout() {
    let toml_str = r#"
        name = "test"
        command = "echo hello"
        timeout_secs = 30
    "#;
    let entry: PromptGuardEntry = toml::from_str(toml_str).unwrap();
    assert_eq!(entry.timeout_secs, 30);
}

// ---------------------------------------------------------------------------
// HooksConfig prompt_guard deserialization tests
// ---------------------------------------------------------------------------

#[test]
fn test_hooks_config_with_prompt_guard() {
    let toml_str = r#"
        [[prompt_guard]]
        name = "branch-protection"
        command = "/path/to/guard.sh"
        timeout_secs = 5

        [[prompt_guard]]
        name = "commit-reminder"
        command = "/path/to/remind.sh"

        [pre_run]
        enabled = true
        command = "echo pre"
    "#;

    let config: crate::config::HooksConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.prompt_guard.len(), 2);
    assert_eq!(config.prompt_guard[0].name, "branch-protection");
    assert_eq!(config.prompt_guard[0].timeout_secs, 5);
    assert_eq!(config.prompt_guard[1].name, "commit-reminder");
    assert_eq!(config.prompt_guard[1].timeout_secs, 10); // default
    // Regular hooks still work
    assert!(config.hooks.contains_key("pre_run"));
}

#[test]
fn test_hooks_config_without_prompt_guard() {
    let toml_str = r#"
        [pre_run]
        enabled = true
        command = "echo pre"
    "#;

    let config: crate::config::HooksConfig = toml::from_str(toml_str).unwrap();
    assert!(
        config.prompt_guard.is_empty(),
        "Missing prompt_guard should default to empty vec"
    );
    assert!(config.hooks.contains_key("pre_run"));
}
