use std::{
    cell::{Cell, RefCell},
    ops::Deref,
    rc::Rc,
    time::Duration,
};

#[cfg(target_os = "linux")]
use std::path::Path;

use agent_client_protocol::{
    AgentSideConnection, AvailableCommand, AvailableCommandsUpdate, Client as _,
    ClientSideConnection,
    ContentBlock, ContentChunk, InitializeRequest, InitializeResponse, NewSessionRequest,
    NewSessionResponse, PromptRequest, PromptResponse, SessionId, SessionNotification,
    SessionUpdate, StopReason, TextContent,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    process::{Child, Command},
    task::LocalSet,
};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

#[derive(Clone)]
struct HangingTestAgent {
    next_session_id: Cell<u64>,
    prompt_delay: Duration,
}

#[derive(Debug, Clone, Copy)]
enum PromptBehavior {
    Silent,
    ProtocolOnly,
    EligibleEventStream,
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
    cmd.arg("-c").arg(shell_script);
    configure_test_child(&mut cmd);
    cmd.spawn().expect("spawn test child")
}

fn configure_test_child(cmd: &mut Command) {
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // The test connection owns the child. Ensure an assertion panic cannot
        // leave its direct child behind while the nextest worker stays alive.
        .kill_on_drop(true);

    #[cfg(unix)]
    {
        // AcpConnection::kill targets -PID, so fixtures must be process-group
        // leaders for descendant cleanup to be meaningful.
        cmd.process_group(0);
    }
}

struct TestConnectionGuard {
    connection: AcpConnection,
    #[cfg(unix)]
    process_group_leader: Option<u32>,
}

impl TestConnectionGuard {
    fn new(connection: AcpConnection) -> Self {
        #[cfg(unix)]
        let process_group_leader = connection.child_pid();

        Self {
            connection,
            #[cfg(unix)]
            process_group_leader,
        }
    }
}

impl Deref for TestConnectionGuard {
    type Target = AcpConnection;

    fn deref(&self) -> &Self::Target {
        &self.connection
    }
}

impl Drop for TestConnectionGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        if self
            .process_group_leader
            .is_some_and(|pid| self.connection.child_pid() == Some(pid))
        {
            // SAFETY: the unreaped child still owns this process-group ID. This
            // is test-only panic cleanup; normal timeout cleanup remains in
            // AcpConnection::kill.
            unsafe {
                libc::kill(
                    -(self.process_group_leader.expect("stored process group") as i32),
                    libc::SIGKILL,
                );
            }
        }
    }
}

#[cfg(target_os = "linux")]
struct CpuBoundChildFixture {
    _temp_dir: tempfile::TempDir,
    control: Option<UnixStream>,
}

#[cfg(target_os = "linux")]
impl CpuBoundChildFixture {
    async fn spawn() -> (Self, Child) {
        let temp_dir = tempfile::tempdir().expect("create CPU-bound child fixture directory");
        let control_path = temp_dir.path().join("control.sock");
        let listener = UnixListener::bind(&control_path).expect("bind CPU-bound child control socket");

        let child = spawn_cpu_bound_test_child(&control_path);
        let control = wait_for_cpu_fixture_connection(&listener).await;

        (
            Self {
                _temp_dir: temp_dir,
                control: Some(control),
            },
            child,
        )
    }

    async fn begin_cpu_load(&mut self) {
        let mut control = self
            .control
            .take()
            .expect("CPU-bound fixture control connection");
        control
            .write_all(&[1])
            .await
            .expect("release CPU-bound child into its load loop");

        let mut started = [0];
        tokio::time::timeout(Duration::from_secs(5), control.read_exact(&mut started))
            .await
            .expect("CPU-bound child did not begin its load loop")
            .expect("read CPU-bound child load-loop acknowledgement");
        assert_eq!(started, [1], "unexpected CPU-bound child load-loop acknowledgement");
    }
}

#[cfg(target_os = "linux")]
fn spawn_cpu_bound_test_child(control_path: &Path) -> Child {
    const CPU_LOAD_FIXTURE: &str = r#"
import socket
import sys

control_path = sys.argv[1]
with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as control:
    control.connect(control_path)
    if control.recv(1) != b"\x01":
        raise RuntimeError("CPU-bound fixture was released without its control byte")
    for _ in range(1_000_000):
        pass
    control.sendall(b"\x01")
    while True:
        pass
"#;

    let mut cmd = Command::new("python3");
    cmd.arg("-c").arg(CPU_LOAD_FIXTURE).arg(control_path);
    configure_test_child(&mut cmd);
    cmd.spawn().expect("spawn CPU-bound test child")
}

#[cfg(target_os = "linux")]
async fn wait_for_cpu_fixture_connection(listener: &UnixListener) -> UnixStream {
    tokio::time::timeout(Duration::from_secs(5), listener.accept())
        .await
        .expect("CPU-bound child did not reach its control handshake")
        .expect("accept CPU-bound child control connection")
        .0
}

fn append_test_stderr_tail(stderr_buf: &mut String, chunk: &str) {
    stderr_buf.push_str(chunk);
    const MAX_STDERR_BYTES: usize = 1024 * 1024;
    if stderr_buf.len() > MAX_STDERR_BYTES {
        let trim_from = stderr_buf.len() - MAX_STDERR_BYTES;
        stderr_buf.drain(..trim_from);
    }
}

async fn build_test_connection(
    mut child: Child,
    prompt_delay: Duration,
    prompt_behavior: PromptBehavior,
) -> TestConnectionGuard {
    let stdin = child.stdin.take().expect("child stdin");
    let stdout = child.stdout.take().expect("child stdout");
    let stderr = child.stderr.take().expect("child stderr");
    let local_set = LocalSet::new();
    let (connection, events, last_activity, last_meaningful_activity, stderr_buf) = local_set
        .run_until(async move {
            let events = Rc::new(RefCell::new(SessionEventStore::default()));
            let last_activity = Rc::new(RefCell::new(std::time::Instant::now()));
            let last_meaningful_activity = Rc::new(RefCell::new(std::time::Instant::now()));
            let client = crate::client::AcpClient::new(
                events.clone(),
                last_activity.clone(),
                last_meaningful_activity.clone(),
            );
            let notification_client = client.clone();
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

            if !matches!(prompt_behavior, PromptBehavior::Silent) {
                tokio::task::spawn_local(async move {
                    let sleep_step = Duration::from_millis(40);
                    let deadline = tokio::time::Instant::now() + prompt_delay;
                    while tokio::time::Instant::now() < deadline {
                        let update = match prompt_behavior {
                            PromptBehavior::Silent => unreachable!(),
                            PromptBehavior::ProtocolOnly => {
                                SessionUpdate::AvailableCommandsUpdate(AvailableCommandsUpdate::new(
                                    vec![AvailableCommand::new("/help", "Get help")],
                                ))
                            }
                            PromptBehavior::EligibleEventStream => {
                                SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                    ContentBlock::Text(TextContent::new("still working")),
                                ))
                            }
                        };
                        notification_client
                            .session_notification(SessionNotification::new("test-session-0", update))
                            .await
                            .expect("inject test session notification");
                        tokio::time::sleep(sleep_step).await;
                    }
                });
            }

            let stderr_buf_clone = stderr_buf.clone();
            let activity_clone = last_activity.clone();
            let meaningful_activity_clone = last_meaningful_activity.clone();
            tokio::task::spawn_local(async move {
                let mut reader = stderr;
                let mut buf = vec![0_u8; 4096];
                loop {
                    match reader.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => {
                            let now = std::time::Instant::now();
                            *activity_clone.borrow_mut() = now;
                            *meaningful_activity_clone.borrow_mut() = now;
                            let text = String::from_utf8_lossy(&buf[..n]);
                            append_test_stderr_tail(&mut stderr_buf_clone.borrow_mut(), &text);
                        }
                        Err(_) => break,
                    }
                }
            });

            (conn, events, last_activity, last_meaningful_activity, stderr_buf)
        })
        .await;

    TestConnectionGuard::new(AcpConnection::new_from_parts(
        local_set,
        connection,
        child,
        events,
        last_activity,
        last_meaningful_activity,
        Rc::new(RefCell::new(None)),
        stderr_buf,
        std::env::current_dir().expect("cwd"),
        AcpConnectionOptions {
            termination_grace_period: Duration::ZERO,
            ..AcpConnectionOptions::default()
        },
    ))
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
async fn initial_response_timeout_stays_alive_for_stderr_only() {
    let connection = build_test_connection(
        // Use a single long-lived process for stderr heartbeats. A shell loop with
        // `sleep 0.05` forks repeatedly and can trip EAGAIN under `nextest` load,
        // causing the child to exit before `new_session()`.
        spawn_test_child(
            "python3 -c 'import sys,time\nwhile True:\n sys.stderr.write(\"starting codex auth\\\\n\")\n sys.stderr.flush()\n time.sleep(0.05)'",
        ),
        Duration::from_secs(5),
        PromptBehavior::Silent,
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
        Some(Duration::from_millis(500)),
        PromptIoOptions::default(),
    );
    let outcome = tokio::time::timeout(Duration::from_millis(1200), prompt).await;

    assert!(
        outcome.is_err(),
        "stderr activity before the first eligible event must keep the initial-response watchdog alive"
    );
    connection.kill().await.expect("kill test child");
}

#[tokio::test]
async fn initial_response_timeout_fires_when_stderr_also_silent() {
    let connection = build_test_connection(
        spawn_test_child("sleep 5"),
        Duration::from_secs(5),
        PromptBehavior::Silent,
    )
    .await;

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

#[cfg(target_os = "linux")]
#[tokio::test]
async fn idle_timeout_stays_alive_while_child_process_tree_consumes_cpu() {
    let connection = build_test_connection(
        spawn_test_child("while :; do :; done"),
        Duration::from_millis(450),
        PromptBehavior::Silent,
    )
    .await;

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
            Duration::from_millis(120),
            None,
            PromptIoOptions::default(),
        )
        .await
        .expect("prompt should complete while CPU progress keeps idle timeout alive");

    assert!(!result.timed_out, "busy child process tree must not be killed");
    assert_eq!(result.exit_reason.as_deref(), Some("end_turn"));
    connection.kill().await.expect("kill test child");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn initial_response_timeout_fires_while_child_process_tree_consumes_cpu() {
    let (mut cpu_fixture, child) = CpuBoundChildFixture::spawn().await;
    let connection = build_test_connection(
        child,
        Duration::from_secs(5),
        PromptBehavior::Silent,
    )
    .await;

    connection.initialize().await.expect("initialize");
    let cwd = std::env::current_dir().expect("cwd");
    let session_id = connection
        .new_session(None, Some(cwd.as_path()), None)
        .await
        .expect("new session");

    cpu_fixture.begin_cpu_load().await;

    let result = tokio::time::timeout(
        Duration::from_millis(700),
        connection.prompt_with_io(
            &session_id,
            "ping",
            Duration::from_secs(5),
            Some(Duration::from_millis(150)),
            PromptIoOptions::default(),
        ),
    )
    .await
    .expect("CPU progress must not extend the initial-response watchdog")
    .expect("prompt result");

    assert!(
        result.timed_out,
        "CPU-only child process activity must trip the initial-response watchdog"
    );
    assert_eq!(
        result.exit_reason.as_deref(),
        Some("initial_response_timeout")
    );
}

#[tokio::test]
async fn idle_timeout_still_fires_for_alive_child_with_no_cpu_progress() {
    let connection = build_test_connection(
        spawn_test_child("sleep 5"),
        Duration::from_secs(5),
        PromptBehavior::Silent,
    )
    .await;

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
            Duration::from_millis(150),
            None,
            PromptIoOptions::default(),
        )
        .await
        .expect("prompt returns timeout result");

    assert!(result.timed_out, "sleeping child must remain killable");
    assert_eq!(result.exit_reason.as_deref(), Some("idle_timeout"));
}

#[tokio::test]
async fn initial_response_timeout_fires_when_only_protocol_notifications() {
    let connection = build_test_connection(
        spawn_test_child("sleep 5"),
        Duration::from_secs(5),
        PromptBehavior::ProtocolOnly,
    )
    .await;

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

    assert!(result.timed_out, "protocol-only chatter must trip the watchdog");
    assert_eq!(
        result.exit_reason.as_deref(),
        Some("initial_response_timeout")
    );
}

#[tokio::test]
async fn initial_response_timeout_stays_alive_for_eligible_event_stream() {
    let connection = build_test_connection(
        spawn_test_child("sleep 5"),
        Duration::from_millis(220),
        PromptBehavior::EligibleEventStream,
    )
    .await;

    connection.initialize().await.expect("initialize");
    let cwd = std::env::current_dir().expect("cwd");
    let session_id = connection
        .new_session(None, Some(cwd.as_path()), None)
        .await
        .expect("new session");

    let result = tokio::time::timeout(
        Duration::from_millis(700),
        connection.prompt_with_io(
            &session_id,
            "ping",
            Duration::from_millis(300),
            Some(Duration::from_millis(150)),
            PromptIoOptions::default(),
        ),
    )
    .await
    .expect("eligible events should keep prompt alive until completion")
    .expect("prompt result");

    assert!(!result.timed_out, "eligible events must prevent timeout");
    assert_eq!(result.exit_reason.as_deref(), Some("end_turn"));
    assert!(
        result
            .events
            .iter()
            .any(crate::client::event_counts_as_initial_response),
        "eligible event stream should record at least one initial-response event"
    );
}
