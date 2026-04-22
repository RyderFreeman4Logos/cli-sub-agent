#[tokio::test]
async fn test_gemini_acp_falls_back_to_degraded_mcp_when_preflight_detects_unhealthy() {
    let temp = tempfile::tempdir().expect("tempdir");
    let runtime_home = temp.path().join("runtime-home");
    write_gemini_settings(&runtime_home, "missing-mcp-command");

    let steps = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let observed_settings = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let settings_path = runtime_home.join(".gemini/settings.json");

    let outcome = AcpTransport::execute_gemini_acp_with_degraded_mcp_retry(
        &runtime_home,
        Some(std::ffi::OsString::from("/prepared/bin")),
        true,
        {
            let steps = std::sync::Arc::clone(&steps);
            let observed_settings = std::sync::Arc::clone(&observed_settings);
            let settings_path = settings_path.clone();
            move || {
                let steps = std::sync::Arc::clone(&steps);
                let observed_settings = std::sync::Arc::clone(&observed_settings);
                let settings_path = settings_path.clone();
                async move {
                    steps.lock().expect("steps lock").push("spawn".to_string());
                    *observed_settings.lock().expect("settings lock") = Some(
                        std::fs::read_to_string(&settings_path)
                            .expect("spawn should see runtime settings"),
                    );
                    Ok::<_, anyhow::Error>("spawned".to_string())
                }
            }
        },
        {
            let steps = std::sync::Arc::clone(&steps);
            move |_, _| {
                steps.lock().expect("steps lock").push("diagnose".to_string());
                McpInitDiagnostic {
                    unhealthy_servers: vec!["broken-mcp".to_string()],
                    ..Default::default()
                }
            }
        },
        {
            let steps = std::sync::Arc::clone(&steps);
            move |path, diagnostic, disable_all| {
                steps.lock().expect("steps lock").push(format!(
                    "disable:{disable_all}:{}",
                    diagnostic.unhealthy_servers.join(",")
                ));
                disable_mcp_servers_in_runtime(path, diagnostic, disable_all)
            }
        },
        |_| None,
    )
    .await
    .expect("preflight unhealthy MCP should degrade before first spawn");

    assert_eq!(
        steps.lock().expect("steps lock").clone(),
        vec![
            "diagnose".to_string(),
            "disable:false:broken-mcp".to_string(),
            "spawn".to_string(),
        ]
    );
    assert!(
        observed_settings
            .lock()
            .expect("settings lock")
            .as_deref()
            .is_some_and(|settings| settings.contains(r#""mcpServers": {}"#)),
        "spawn should observe a degraded runtime settings file"
    );
    assert!(
        outcome
            .warning_summary
            .as_deref()
            .is_some_and(|summary| summary.contains("broken-mcp")),
        "warning summary should name the unhealthy server: {:?}",
        outcome.warning_summary
    );
}

#[tokio::test]
async fn test_gemini_acp_retries_with_degraded_mcp_on_generic_init_crash() {
    async fn run_case(second_spawn_succeeds: bool) -> Result<GeminiAcpMcpRetryOutcome<String>> {
        let temp = tempfile::tempdir().expect("tempdir");
        let runtime_home = temp.path().join("runtime-home");
        write_gemini_settings(&runtime_home, "missing-mcp-command");

        let diagnose_calls = std::sync::Arc::new(std::sync::Mutex::new(0usize));
        let disable_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let spawn_calls = std::sync::Arc::new(std::sync::Mutex::new(0usize));
        let settings_after_retry = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
        let settings_path = runtime_home.join(".gemini/settings.json");

        AcpTransport::execute_gemini_acp_with_degraded_mcp_retry(
            &runtime_home,
            Some(std::ffi::OsString::from("/prepared/bin")),
            true,
            {
                let spawn_calls = std::sync::Arc::clone(&spawn_calls);
                let settings_after_retry = std::sync::Arc::clone(&settings_after_retry);
                let settings_path = settings_path.clone();
                move || {
                    let spawn_calls = std::sync::Arc::clone(&spawn_calls);
                    let settings_after_retry = std::sync::Arc::clone(&settings_after_retry);
                    let settings_path = settings_path.clone();
                    async move {
                        let mut call_count = spawn_calls.lock().expect("spawn calls lock");
                        *call_count += 1;
                        if *call_count == 1 {
                            return Err(anyhow::anyhow!("first generic init crash"));
                        }

                        *settings_after_retry.lock().expect("settings lock") = Some(
                            std::fs::read_to_string(&settings_path)
                                .expect("retry spawn should read runtime settings"),
                        );
                        if second_spawn_succeeds {
                            Ok::<_, anyhow::Error>("recovered".to_string())
                        } else {
                            Err(anyhow::anyhow!("second generic init crash"))
                        }
                    }
                }
            },
            {
                let diagnose_calls = std::sync::Arc::clone(&diagnose_calls);
                move |_, _| {
                    let mut call_count = diagnose_calls.lock().expect("diagnose calls lock");
                    *call_count += 1;
                    if *call_count == 1 {
                        McpInitDiagnostic::default()
                    } else {
                        McpInitDiagnostic {
                            unhealthy_servers: vec!["broken-mcp".to_string()],
                            ..Default::default()
                        }
                    }
                }
            },
            {
                let disable_calls = std::sync::Arc::clone(&disable_calls);
                move |path, diagnostic, disable_all| {
                    disable_calls.lock().expect("disable calls lock").push(format!(
                        "disable:{disable_all}:{}",
                        diagnostic.unhealthy_servers.join(",")
                    ));
                    disable_mcp_servers_in_runtime(path, diagnostic, disable_all)
                }
            },
            |_| {
                Some(GeminiAcpInitFailureClassification {
                    code: "gemini_acp_init_handshake_timeout",
                    missing_env_vars: Vec::new(),
                })
            },
        )
        .await
        .inspect(|_| {
            assert_eq!(
                *spawn_calls.lock().expect("spawn calls lock"),
                2,
                "generic init crash should trigger exactly one degraded retry"
            );
            assert_eq!(
                disable_calls.lock().expect("disable calls lock").as_slice(),
                ["disable:false:broken-mcp"],
                "degraded retry should disable the diagnosed unhealthy server"
            );
            assert!(
                settings_after_retry
                    .lock()
                    .expect("settings lock")
                    .as_deref()
                    .is_some_and(|settings| settings.contains(r#""mcpServers": {}"#)),
                "retry spawn should observe the degraded runtime settings"
            );
        })
    }

    let success = run_case(true)
        .await
        .expect("generic init crash should recover after degraded retry");
    assert!(
        success
            .warning_summary
            .as_deref()
            .is_some_and(|summary| summary.contains("broken-mcp")),
        "warning summary should name the retried unhealthy server: {:?}",
        success.warning_summary
    );

    let error = run_case(false)
        .await
        .expect_err("second degraded retry failure should surface unhealthy server hint");
    let rendered = format!("{error:#}");
    assert!(
        rendered.contains("broken-mcp"),
        "second-round failure should include unhealthy server hint: {rendered}"
    );
}

#[tokio::test]
async fn test_gemini_acp_no_retry_when_first_spawn_succeeds() {
    let temp = tempfile::tempdir().expect("tempdir");
    let runtime_home = temp.path().join("runtime-home");
    write_gemini_settings(&runtime_home, "missing-mcp-command");

    let diagnose_calls = std::sync::Arc::new(std::sync::Mutex::new(0usize));
    let disable_calls = std::sync::Arc::new(std::sync::Mutex::new(0usize));
    let spawn_calls = std::sync::Arc::new(std::sync::Mutex::new(0usize));

    let outcome = AcpTransport::execute_gemini_acp_with_degraded_mcp_retry(
        &runtime_home,
        Some(std::ffi::OsString::from("/prepared/bin")),
        true,
        {
            let spawn_calls = std::sync::Arc::clone(&spawn_calls);
            move || {
                let spawn_calls = std::sync::Arc::clone(&spawn_calls);
                async move {
                    *spawn_calls.lock().expect("spawn calls lock") += 1;
                    Ok::<_, anyhow::Error>("ok".to_string())
                }
            }
        },
        {
            let diagnose_calls = std::sync::Arc::clone(&diagnose_calls);
            move |_, _| {
                *diagnose_calls.lock().expect("diagnose calls lock") += 1;
                McpInitDiagnostic::default()
            }
        },
        {
            let disable_calls = std::sync::Arc::clone(&disable_calls);
            move |_, _, _| {
                *disable_calls.lock().expect("disable calls lock") += 1;
                Ok(())
            }
        },
        |_| {
            Some(GeminiAcpInitFailureClassification {
                code: "gemini_acp_init_handshake_timeout",
                missing_env_vars: Vec::new(),
            })
        },
    )
    .await
    .expect("first spawn success should return directly");

    assert_eq!(outcome.value, "ok");
    assert!(outcome.warning_summary.is_none());
    assert_eq!(*diagnose_calls.lock().expect("diagnose calls lock"), 1);
    assert_eq!(*disable_calls.lock().expect("disable calls lock"), 0);
    assert_eq!(*spawn_calls.lock().expect("spawn calls lock"), 1);
}
