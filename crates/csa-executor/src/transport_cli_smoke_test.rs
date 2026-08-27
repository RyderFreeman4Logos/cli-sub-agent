// ---- Optional integration test (gated; CI-friendly) ----

/// Optional: actually spawn `claude` and verify the CLI transport produces
/// a non-error result.  Skipped when the binary isn't installed (so this
/// test never causes false-red on CI without claude).
#[ignore = "requires claude CLI installed; run manually with `cargo test -p csa-executor -- --ignored claude_cli_smoke`"]
#[tokio::test]
async fn claude_cli_smoke() {
    if which::which("claude").is_err() {
        eprintln!("claude binary not on PATH; skipping smoke");
        return;
    }
    let executor = make_executor();
    let transport = ClaudeCodeCliTransport::new(executor);
    let tmp = tempfile::tempdir().expect("tempdir");
    let result = transport
        .execute_in(
            "say 'hello from cli transport'",
            tmp.path(),
            None,
            None,
            false,
            StreamMode::BufferOnly,
            30,
            ResolvedTimeout::of(60),
        )
        .await;
    // We don't assert on content (depends on user auth and network); we
    // only assert the call did not bubble an unrelated error.
    assert!(
        result.is_ok(),
        "smoke: transport.execute_in failed: {result:?}"
    );
}
