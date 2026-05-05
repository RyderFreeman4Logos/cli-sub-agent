use std::time::Instant;
use std::{cell::RefCell, collections::VecDeque, rc::Rc};

use agent_client_protocol::{
    Client, ContentBlock, ContentChunk, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SelectedPermissionOutcome, SessionNotification, SessionUpdate,
};

/// Maximum bytes retained in the tail text buffer.
///
/// Agent message and thought text beyond this limit is discarded from memory
/// after being written to the output spool on disk.  1 MiB is sufficient for
/// summary extraction and token-usage parsing, which only inspect the tail.
///
/// Canonical values shared with `csa-process::output_helpers` (same 1 MiB / 2 MiB).
const TAIL_BUFFER_MAX_BYTES: usize = 1024 * 1024;

/// High-water mark for tail buffer trimming (2× the target size).
///
/// We allow the buffer to grow to this size before trimming it back to
/// [`TAIL_BUFFER_MAX_BYTES`].  This amortises the O(N) cost of
/// `String::drain` so that trimming occurs once per MiB of new text
/// rather than once per chunk, avoiding O(N²) behaviour.
const TAIL_BUFFER_HIGH_WATER: usize = TAIL_BUFFER_MAX_BYTES * 2;

/// Maximum number of ACP session events retained in memory.
///
/// Set high enough to absorb bursts from parallel test output (cargo
/// nextest can emit thousands of lines per second) without overrunning
/// the 200ms polling interval in `stream_new_agent_messages`.  At ~200
/// bytes per event, 10K events ≈ 2 MiB — negligible vs the old unbounded
/// accumulation that reached 6+ GiB.
pub(crate) const MAX_RETAINED_EVENTS: usize = 10_000;

/// Maximum number of execute command titles retained for post-run policy checks.
const MAX_EXTRACTED_COMMANDS: usize = 100;

/// Incrementally accumulated metadata from streamed ACP events.
///
/// Built up by [`super::connection::stream_new_agent_messages`] as events flow
/// through, avoiding the need to keep the full event vector in memory.
#[derive(Debug, Clone, Default)]
pub struct StreamingMetadata {
    /// Total number of events seen across the entire prompt turn, including dropped events.
    pub total_events_count: usize,
    /// Whether any `ToolCallStarted` event was observed.
    pub has_tool_calls: bool,
    /// Whether any execute `ToolCallStarted` event was observed.
    pub has_execute_tool_calls: bool,
    /// Whether a `--no-verify` or `-n` git commit command was observed.
    /// Tracked separately from `extracted_commands` because the command ring
    /// buffer may evict old entries, but this safety flag must never be lost.
    pub has_no_verify_commit: bool,
    /// Whether any `PlanUpdate` event was observed.
    pub has_plan_updates: bool,
    /// Tail of execute command titles observed during the prompt turn.
    pub extracted_commands: Vec<String>,
    /// Tail buffer of agent message/thought text (bounded by [`TAIL_BUFFER_MAX_BYTES`]).
    pub tail_text: String,
    /// Tail buffer of agent message text only (bounded by [`TAIL_BUFFER_MAX_BYTES`]).
    pub message_text: String,
    /// Tail buffer of agent thought text only (bounded by [`TAIL_BUFFER_MAX_BYTES`]).
    pub thought_text: String,
    /// Whether the output used thought text as fallback (no message text was produced).
    pub has_thought_fallback: bool,
    /// Total bytes written to the output spool file.
    pub(crate) spool_bytes_written: u64,
}

impl StreamingMetadata {
    pub(crate) fn sync_from_store(&mut self, store: &SessionEventStore) {
        self.total_events_count = store.total_events_count();
        self.has_tool_calls = store.has_tool_calls();
        self.has_execute_tool_calls = store.has_execute_tool_calls();
        self.has_no_verify_commit = store.has_no_verify_commit();
        self.has_plan_updates = store.has_plan_updates();
        self.extracted_commands = store.extracted_commands();
    }

    /// Append agent message text to both the message-specific and combined tail buffers.
    pub(crate) fn append_message_text(&mut self, text: &str) {
        self.tail_text.push_str(text);
        trim_tail_buffer(&mut self.tail_text);
        self.message_text.push_str(text);
        trim_tail_buffer(&mut self.message_text);
    }

    /// Append agent thought text to both the thought-specific and combined tail buffers.
    pub(crate) fn append_thought_text(&mut self, text: &str) {
        self.tail_text.push_str(text);
        trim_tail_buffer(&mut self.tail_text);
        self.thought_text.push_str(text);
        trim_tail_buffer(&mut self.thought_text);
    }
}

/// Trim a tail buffer back to [`TAIL_BUFFER_MAX_BYTES`] when it exceeds
/// [`TAIL_BUFFER_HIGH_WATER`], respecting UTF-8 char boundaries.
pub(crate) fn trim_tail_buffer(buf: &mut String) {
    if buf.len() > TAIL_BUFFER_HIGH_WATER {
        let excess = buf.len() - TAIL_BUFFER_MAX_BYTES;
        let mut trim_at = excess;
        while trim_at < buf.len() && !buf.is_char_boundary(trim_at) {
            trim_at += 1;
        }
        buf.drain(..trim_at);
    }
}

/// Streaming session events collected from ACP notifications.
#[derive(Debug, Clone, serde::Serialize)]
pub enum SessionEvent {
    AgentMessage(String),
    AgentThought(String),
    ToolCallStarted {
        id: String,
        title: String,
        kind: String,
    },
    ToolCallCompleted {
        id: String,
        status: String,
    },
    PlanUpdate(String),
    Other(String),
}

pub(crate) fn event_counts_as_initial_response(event: &SessionEvent) -> bool {
    matches!(
        event,
        SessionEvent::AgentMessage(_)
            | SessionEvent::AgentThought(_)
            | SessionEvent::PlanUpdate(_)
            | SessionEvent::ToolCallStarted { .. }
            | SessionEvent::ToolCallCompleted { .. }
    )
}

/// Bounded in-memory ACP event retention with incremental metadata extraction.
#[derive(Debug, Clone, Default)]
pub(crate) struct SessionEventStore {
    events: VecDeque<SessionEvent>,
    total_events_count: usize,
    has_tool_calls: bool,
    has_execute_tool_calls: bool,
    has_no_verify_commit: bool,
    has_plan_updates: bool,
    extracted_commands: VecDeque<String>,
}

impl SessionEventStore {
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn push(&mut self, event: SessionEvent) {
        self.total_events_count += 1;
        self.update_metadata(&event);
        self.events.push_back(event);
        if self.events.len() > MAX_RETAINED_EVENTS {
            let _ = self.events.pop_front();
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.events.len()
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn events(&self) -> Vec<SessionEvent> {
        self.events.iter().cloned().collect()
    }

    pub(crate) fn retained_events(&self) -> &VecDeque<SessionEvent> {
        &self.events
    }

    pub(crate) fn retained_start_index(&self) -> usize {
        self.total_events_count.saturating_sub(self.events.len())
    }

    pub(crate) fn total_events_count(&self) -> usize {
        self.total_events_count
    }

    pub(crate) fn has_tool_calls(&self) -> bool {
        self.has_tool_calls
    }

    pub(crate) fn has_execute_tool_calls(&self) -> bool {
        self.has_execute_tool_calls
    }

    pub(crate) fn has_no_verify_commit(&self) -> bool {
        self.has_no_verify_commit
    }

    pub(crate) fn has_plan_updates(&self) -> bool {
        self.has_plan_updates
    }

    pub(crate) fn extracted_commands(&self) -> Vec<String> {
        self.extracted_commands.iter().cloned().collect()
    }

    pub(crate) fn take_events(&mut self) -> Vec<SessionEvent> {
        let retained = self.events.drain(..).collect();
        self.clear();
        retained
    }

    fn update_metadata(&mut self, event: &SessionEvent) {
        match event {
            SessionEvent::ToolCallStarted { title, kind, .. } => {
                self.has_tool_calls = true;
                if kind.eq_ignore_ascii_case("execute") {
                    self.has_execute_tool_calls = true;
                    self.push_extracted_command(title);
                }
            }
            SessionEvent::PlanUpdate(_) => {
                self.has_plan_updates = true;
            }
            SessionEvent::AgentMessage(_)
            | SessionEvent::AgentThought(_)
            | SessionEvent::ToolCallCompleted { .. }
            | SessionEvent::Other(_) => {}
        }
    }

    fn push_extracted_command(&mut self, title: &str) {
        let command = title.trim();
        if command.is_empty() {
            return;
        }
        // Sticky flag: once a --no-verify commit is seen, it can never be
        // evicted from the ring buffer and lost.
        if !self.has_no_verify_commit && command_looks_like_no_verify_commit(command) {
            self.has_no_verify_commit = true;
        }
        if self.extracted_commands.len() == MAX_EXTRACTED_COMMANDS {
            let _ = self.extracted_commands.pop_front();
        }
        self.extracted_commands.push_back(command.to_string());
    }
}

/// Quick heuristic: does a tool-call title look like `git commit --no-verify`
/// or `git commit -n`?  This is intentionally simpler than the full shell
/// parser in `run_cmd_shell.rs` because it only needs to catch the common
/// pattern within ACP execute-tool-call titles (which are short, single
/// commands).  The authoritative check still runs in
/// `apply_no_verify_commit_policy`; this flag merely ensures the event is
/// never silently evicted from the bounded ring buffer.
fn command_looks_like_no_verify_commit(cmd: &str) -> bool {
    let tokens = tokenize_shell_tokens(cmd);
    if let Some(shell_script_tokens) = extract_shell_c_payload_tokens(&tokens)
        && shell_script_contains_no_verify_commit(shell_script_tokens)
    {
        return true;
    }
    let Some((_, git_commit_subcommand_idx)) = locate_git_commit_command(&tokens) else {
        return false;
    };
    commit_args_include_no_verify(&tokens[git_commit_subcommand_idx + 1..])
}

fn tokenize_shell_tokens(segment: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = segment.chars().peekable();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        if escaped {
            if ch != '\n' {
                current.push(ch);
            }
            escaped = false;
            continue;
        }

        if ch == '\\' {
            escaped = true;
            continue;
        }

        if in_single_quote {
            if ch == '\'' {
                in_single_quote = false;
            } else {
                current.push(ch);
            }
            continue;
        }

        if in_double_quote {
            if ch == '"' {
                in_double_quote = false;
            } else {
                current.push(ch);
            }
            continue;
        }

        match ch {
            '\'' => in_single_quote = true,
            '"' => in_double_quote = true,
            '\n' => {
                push_shell_token(&mut tokens, &mut current);
                tokens.push(";".to_string());
            }
            ';' => {
                push_shell_token(&mut tokens, &mut current);
                tokens.push(";".to_string());
            }
            '&' => {
                push_shell_token(&mut tokens, &mut current);
                if chars.peek().is_some_and(|next| *next == '&') {
                    let _ = chars.next();
                    tokens.push("&&".to_string());
                } else {
                    tokens.push("&".to_string());
                }
            }
            '|' => {
                push_shell_token(&mut tokens, &mut current);
                if chars.peek().is_some_and(|next| *next == '|') {
                    let _ = chars.next();
                    tokens.push("||".to_string());
                } else {
                    tokens.push("|".to_string());
                }
            }
            c if c.is_whitespace() => push_shell_token(&mut tokens, &mut current),
            _ => current.push(ch),
        }
    }

    if escaped {
        current.push('\\');
    }
    push_shell_token(&mut tokens, &mut current);
    tokens
}

fn push_shell_token(tokens: &mut Vec<String>, current: &mut String) {
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        tokens.push(trimmed.to_string());
    }
    current.clear();
}

fn extract_shell_c_payload_tokens(tokens: &[String]) -> Option<&[String]> {
    if tokens.len() < 3 || !is_shell_token(tokens[0].as_str()) {
        return None;
    }
    let shell_flag = tokens[1].as_str();
    if !shell_flag.starts_with('-') || !shell_flag.contains('c') {
        return None;
    }
    Some(&tokens[2..])
}

fn shell_script_contains_no_verify_commit(tokens: &[String]) -> bool {
    let mut script_tokens = Vec::new();
    for token in tokens {
        script_tokens.extend(tokenize_shell_tokens(token));
    }

    let Some((_, git_commit_subcommand_idx)) = locate_git_commit_command(&script_tokens) else {
        return false;
    };
    commit_args_include_no_verify(&script_tokens[git_commit_subcommand_idx + 1..])
}

fn locate_git_commit_command(tokens: &[String]) -> Option<(usize, usize)> {
    let mut idx = 0usize;
    while idx < tokens.len() {
        let token = tokens[idx].as_str();
        if is_git_token(token) {
            let mut scan = idx + 1;
            while scan < tokens.len() {
                let current = tokens[scan].as_str();
                if current == "commit" {
                    return Some((idx, scan));
                }
                if current == "--" {
                    break;
                }
                if current.starts_with('-') {
                    scan += 1;
                    if git_global_option_consumes_value(current) && !current.contains('=') {
                        scan = consume_option_value(tokens, scan);
                    }
                    continue;
                }
                break;
            }
        }
        idx += 1;
    }
    None
}

fn commit_args_include_no_verify(args: &[String]) -> bool {
    let mut idx = 0usize;
    while idx < args.len() {
        let token = args[idx].as_str();
        if token == "--" || is_command_separator_token(token) {
            break;
        }
        if token.eq_ignore_ascii_case("--no-verify") {
            return true;
        }
        if token.starts_with("--") {
            idx += 1;
            if commit_long_option_consumes_value(token) && !token.contains('=') {
                idx = consume_option_value(args, idx);
            }
            continue;
        }
        if token.starts_with('-') && token.len() > 1 {
            let mut chars = token[1..].chars().peekable();
            let mut consumes_value = false;
            while let Some(flag) = chars.next() {
                if flag == 'n' {
                    return true;
                }
                if commit_short_option_consumes_value(flag) {
                    consumes_value = chars.peek().is_none();
                    break;
                }
            }
            idx += 1;
            if consumes_value {
                idx = consume_option_value(args, idx);
            }
            continue;
        }
        idx += 1;
    }
    false
}

fn is_command_separator_token(token: &str) -> bool {
    matches!(token, ";" | "&&" | "||" | "|" | "&")
        || token.ends_with(';')
        || token.ends_with("&&")
        || token.ends_with("||")
        || token.ends_with('|')
        || token.ends_with('&')
}

fn consume_option_value(args: &[String], mut idx: usize) -> usize {
    if idx < args.len() {
        idx += 1;
    }
    idx
}

fn commit_short_option_consumes_value(flag: char) -> bool {
    matches!(flag, 'm' | 'F' | 'c' | 'C' | 't')
}

fn commit_long_option_consumes_value(token: &str) -> bool {
    matches!(
        token,
        "--message"
            | "--file"
            | "--template"
            | "--reuse-message"
            | "--reedit-message"
            | "--fixup"
            | "--squash"
            | "--author"
            | "--date"
            | "--trailer"
            | "--pathspec-from-file"
            | "--cleanup"
    )
}

fn is_git_token(token: &str) -> bool {
    token.eq_ignore_ascii_case("git") || token.ends_with("/git")
}

fn is_shell_token(token: &str) -> bool {
    matches!(
        token.rsplit('/').next(),
        Some("bash" | "sh" | "zsh" | "fish")
    )
}

fn git_global_option_consumes_value(token: &str) -> bool {
    matches!(
        token,
        "-c" | "-C" | "--exec-path" | "--git-dir" | "--work-tree" | "--namespace" | "--config-env"
    )
}

pub(crate) type SharedEvents = Rc<RefCell<SessionEventStore>>;
pub(crate) type SharedActivity = Rc<RefCell<Instant>>;

#[derive(Debug, Clone)]
pub(crate) struct AcpClient {
    events: SharedEvents,
    last_activity: SharedActivity,
    last_meaningful_activity: SharedActivity,
}

impl AcpClient {
    pub(crate) fn new(
        events: SharedEvents,
        last_activity: SharedActivity,
        last_meaningful_activity: SharedActivity,
    ) -> Self {
        Self {
            events,
            last_activity,
            last_meaningful_activity,
        }
    }

    fn chunk_to_text(chunk: &ContentChunk) -> String {
        match &chunk.content {
            ContentBlock::Text(text) => text.text.clone(),
            other => serde_json::to_string(other).unwrap_or_else(|_| "<non-text-content>".into()),
        }
    }

    /// Convert an ACP `SessionUpdate` into an optional `SessionEvent`.
    ///
    /// Returns `None` for protocol-level overhead events that carry no
    /// meaningful content (e.g. `AvailableCommandsUpdate`, mode/config
    /// updates).  These are logged at trace level but excluded from
    /// collected output to prevent multi-KB JSON blobs from polluting
    /// stdout and summary extraction.
    fn update_to_event(update: SessionUpdate) -> Option<SessionEvent> {
        match update {
            SessionUpdate::AgentMessageChunk(chunk) => {
                Some(SessionEvent::AgentMessage(Self::chunk_to_text(&chunk)))
            }
            SessionUpdate::AgentThoughtChunk(chunk) => {
                Some(SessionEvent::AgentThought(Self::chunk_to_text(&chunk)))
            }
            SessionUpdate::ToolCall(tool_call) => Some(SessionEvent::ToolCallStarted {
                id: tool_call.tool_call_id.0.to_string(),
                title: tool_call.title,
                kind: format!("{:?}", tool_call.kind),
            }),
            SessionUpdate::ToolCallUpdate(tool_call_update) => {
                let id = tool_call_update.tool_call_id.0.to_string();
                if let Some(status) = tool_call_update.fields.status {
                    Some(SessionEvent::ToolCallCompleted {
                        id,
                        status: format!("{status:?}"),
                    })
                } else {
                    Some(SessionEvent::Other(
                        serde_json::to_string(&tool_call_update)
                            .unwrap_or_else(|_| "tool_call_update".into()),
                    ))
                }
            }
            SessionUpdate::Plan(plan) => {
                let serialized = serde_json::to_string(&plan)
                    .unwrap_or_else(|_| "<plan-serialize-failed>".into());
                Some(SessionEvent::PlanUpdate(serialized))
            }
            // Protocol overhead: these carry large JSON payloads (slash
            // command lists, config toggles, mode switches) that are not
            // meaningful agent output.  Suppress from events to keep
            // stdout clean and summary extraction accurate.
            SessionUpdate::AvailableCommandsUpdate(_)
            | SessionUpdate::ConfigOptionUpdate(_)
            | SessionUpdate::CurrentModeUpdate(_)
            | SessionUpdate::UserMessageChunk(_) => {
                tracing::trace!("suppressed protocol-level SessionUpdate (not content)");
                None
            }
            // Catch-all for future ACP protocol variants (enum is
            // non-exhaustive).  Emit as Other for visibility.
            other => Some(SessionEvent::Other(
                serde_json::to_string(&other).unwrap_or_else(|_| "<unknown-update>".into()),
            )),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl Client for AcpClient {
    async fn request_permission(
        &self,
        args: RequestPermissionRequest,
    ) -> agent_client_protocol::Result<RequestPermissionResponse> {
        let outcome = args
            .options
            .first()
            .map(|first| {
                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                    first.option_id.clone(),
                ))
            })
            .unwrap_or(RequestPermissionOutcome::Cancelled);

        Ok(RequestPermissionResponse::new(outcome))
    }

    async fn session_notification(
        &self,
        args: SessionNotification,
    ) -> agent_client_protocol::Result<()> {
        let now = Instant::now();
        // Idle-timeout remains broad: any ACP session notification counts
        // as transport liveness, even when suppressed from collected output.
        *self.last_activity.borrow_mut() = now;
        if let Some(event) = Self::update_to_event(args.update) {
            if event_counts_as_initial_response(&event) {
                *self.last_meaningful_activity.borrow_mut() = now;
            }
            self.events.borrow_mut().push(event);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
