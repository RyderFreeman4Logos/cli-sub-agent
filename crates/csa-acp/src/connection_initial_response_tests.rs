use std::{cell::Cell, rc::Rc, time::Duration};

use agent_client_protocol::{
    AgentSideConnection, ClientSideConnection, InitializeRequest, InitializeResponse,
    NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, SessionId, StopReason,
};
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
    task::LocalSet,
};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

#[derive(Clone)]
struct HangingTestAgent {
    next_session_id: Cell<u64>,
    prompt_delay: Duration,
}

impl HangingTestAgent {
    fn new(prompt_delay: Duration) -> Self {
        Self {
            next_session_id: Cell::new(0),
            prompt_delay,
        }
    }
}

#[async_trait::async_trait(?Send)]
impl agent_client_protocol::Agent for HangingTestAgent {
    async fn initialize(
        &self,
        args: InitializeRequest,
    ) -> agent_client_protocol::Result<InitializeResponse> {
        Ok(InitializeResponse::new(args.protocol_version))
    }

    async fn authenticate(
        &self,
        _args: agent_client_protocol::AuthenticateRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::AuthenticateResponse> {
        Ok(agent_client_protocol::AuthenticateResponse::default())
    }

    async fn new_session(
        &self,
        _args: NewSessionRequest,
    ) -> agent_client_protocol::Result<NewSessionResponse> {
        let session_id = self.next_session_id.get();
        self.next_session_id.set(session_id + 1);
        Ok(NewSessionResponse::new(SessionId::new(format!(
            "test-session-{session_id}"
        ))))
    }

    async fn prompt(
        &self,
        _args: PromptRequest,
    ) -> agent_client_protocol::Result<PromptResponse> {
        tokio::time::sleep(self.prompt_delay).await;
        Ok(PromptResponse::new(StopReason::EndTurn))
    }

    async fn cancel(
        &self,
        _args: agent_client_protocol::CancelNotification,
    ) -> agent_client_protocol::Result<()> {
        Ok(())
    }
}

fn spawn_test_child(shell_script: &str) -> Child {
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(shell_script)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    cmd.spawn().expect("spawn test child")
}

fn append_test_stderr_tail(stderr_buf: &mut String, chunk: &str) {
    stderr_buf.push_str(chunk);
    const MAX_STDERR_BYTES: usize = 1024 * 1024;
    if stderr_buf.len() > MAX_STDERR_BYTES {
        let trim_from = stderr_buf.len() - MAX_STDERR_BYTES;
        stderr_buf.drain(..trim_from);
    }
}

async fn build_test_connection(mut child: Child, prompt_delay: Duration) -> AcpConnection {
    let stdin = child.stdin.take().expect("child stdin");
    let stdout = child.stdout.take().expect("child stdout");
    let stderr = child.stderr.take().expect("child stderr");
    let local_set = LocalSet::new();
    let (connection, events, last_activity, stderr_buf) = local_set
        .run_until(async move {
            let events = Rc::new(RefCell::new(SessionEventStore::default()));
            let last_activity = Rc::new(RefCell::new(std::time::Instant::now()));
            let client = crate::client::AcpClient::new(events.clone(), last_activity.clone());
            let stderr_buf = Rc::new(RefCell::new(String::new()));
        let (server_reader, client_writer) = tokio::io::duplex(1024);
        let (client_reader, server_writer) = tokio::io::duplex(1024);

        let (conn, io_task) = ClientSideConnection::new(
            client,
            client_writer.compat_write(),
            client_reader.compat(),
            |fut| {
                tokio::task::spawn_local(fut);
            },
        );
        tokio::task::spawn_local(io_task);

        let agent = HangingTestAgent::new(prompt_delay);
        let (agent_conn, agent_io_task) = AgentSideConnection::new(
            agent,
            server_writer.compat_write(),
            server_reader.compat(),
            |fut| {
                tokio::task::spawn_local(fut);
            },
        );
        tokio::task::spawn_local(agent_io_task);
        drop(agent_conn);
        drop(stdin);
        drop(stdout);

        let stderr_buf_clone = stderr_buf.clone();
        let activity_clone = last_activity.clone();
        tokio::task::spawn_local(async move {
            let mut reader = stderr;
            let mut buf = vec![0_u8; 4096];
            loop {
                match reader.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        *activity_clone.borrow_mut() = std::time::Instant::now();
                        let text = String::from_utf8_lossy(&buf[..n]);
                        append_test_stderr_tail(&mut stderr_buf_clone.borrow_mut(), &text);
                    }
                    Err(_) => break,
                }
            }
        });

            (conn, events, last_activity, stderr_buf)
        })
        .await;

    AcpConnection::new_from_parts(
        local_set,
        connection,
        child,
        events,
        last_activity,
        stderr_buf,
        std::env::current_dir().expect("cwd"),
        AcpConnectionOptions::default(),
    )
}

#[test]
fn initial_response_event_filter_accepts_agent_plan_and_tool_events() {
    for event in [
        SessionEvent::AgentMessage("msg".to_string()),
        SessionEvent::AgentThought("thought".to_string()),
        SessionEvent::PlanUpdate("plan".to_string()),
        SessionEvent::ToolCallStarted {
            id: "tool-1".to_string(),
            title: "Run".to_string(),
            kind: "execute".to_string(),
        },
        SessionEvent::ToolCallCompleted {
            id: "tool-1".to_string(),
            status: "completed".to_string(),
        },
    ] {
        assert!(
            crate::client::event_counts_as_initial_response(&event),
            "event should count as initial-response progress: {event:?}"
        );
    }
}

#[test]
fn initial_response_event_filter_ignores_other_events() {
    assert!(
        !crate::client::event_counts_as_initial_response(&SessionEvent::Other(
            "protocol overhead".to_string()
        )),
        "protocol-only events must not satisfy the initial-response watchdog"
    );
}

#[test]
fn stream_new_agent_messages_reports_initial_response_progress_only_for_eligible_events() {
    let events = shared_events(vec![SessionEvent::Other("overhead".to_string())]);
    let mut index = 0;
    let mut spool: Option<SpoolRotator> = None;
    let mut metadata = StreamingMetadata::default();

    assert!(
        !stream_new_agent_messages(
            &events,
            &mut index,
            false,
            &mut spool,
            &mut metadata,
            &mut String::new(),
            &mut String::new(),
        ),
        "Other-only batches must not count as initial-response progress"
    );

    events
        .borrow_mut()
        .push(SessionEvent::AgentMessage("hello".to_string()));
    assert!(
        stream_new_agent_messages(
            &events,
            &mut index,
            false,
            &mut spool,
            &mut metadata,
            &mut String::new(),
            &mut String::new(),
        ),
        "AgentMessage must satisfy initial-response progress"
    );
}

#[tokio::test]
async fn initial_response_timeout_respects_stderr_liveness_before_first_eligible_event() {
    let connection = build_test_connection(
        spawn_test_child("while :; do printf 'starting codex auth\\n' >&2; sleep 0.05; done"),
        Duration::from_secs(5),
    )
    .await;

    connection.initialize().await.expect("initialize");
    let cwd = std::env::current_dir().expect("cwd");
    let session_id = connection
        .new_session(None, Some(cwd.as_path()), None)
        .await
        .expect("new session");

    let prompt = connection.prompt_with_io(
        &session_id,
        "ping",
        Duration::from_secs(5),
        Some(Duration::from_millis(150)),
        PromptIoOptions::default(),
    );
    let outcome = tokio::time::timeout(Duration::from_millis(350), prompt).await;

    assert!(
        outcome.is_err(),
        "stderr activity before the first eligible event must keep the initial-response watchdog alive"
    );
    connection.kill().await.expect("kill test child");
}

#[tokio::test]
async fn initial_response_timeout_fires_when_stderr_also_silent() {
    let connection = build_test_connection(spawn_test_child("sleep 5"), Duration::from_secs(5)).await;

    connection.initialize().await.expect("initialize");
    let cwd = std::env::current_dir().expect("cwd");
    let session_id = connection
        .new_session(None, Some(cwd.as_path()), None)
        .await
        .expect("new session");

    let result = connection
        .prompt_with_io(
            &session_id,
            "ping",
            Duration::from_secs(5),
            Some(Duration::from_millis(150)),
            PromptIoOptions::default(),
        )
        .await
        .expect("prompt result");

    assert!(result.timed_out, "silent child must trip the watchdog");
    assert_eq!(
        result.exit_reason.as_deref(),
        Some("initial_response_timeout")
    );
}
