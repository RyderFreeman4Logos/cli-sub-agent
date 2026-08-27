use std::ffi::OsString;
use std::path::{Path, PathBuf};

use csa_session::state::MetaSessionState;
use tokio::process::Command;

const WRAPPER_SCRIPT: &str = r#"
set -u
session_name=$1
work_dir=$2
shift 2
program=$1
shift

run_dir="${CSA_SESSION_DIR:?CSA_SESSION_DIR must be set for codex tmux_mode}/codex-tmux"
stdin_fifo="$run_dir/stdin.pipe"
stdout_fifo="$run_dir/stdout.pipe"
stderr_fifo="$run_dir/stderr.pipe"
status_file="$run_dir/status"

cleanup() {
  tmux kill-session -t "$session_name" >/dev/null 2>&1 || true
  rm -rf "$run_dir"
}
trap cleanup EXIT
trap 'exit 143' TERM
trap 'exit 130' INT

rm -rf "$run_dir"
mkdir -p "$run_dir"
mkfifo "$stdin_fifo" "$stdout_fifo" "$stderr_fifo"
tmux kill-session -t "$session_name" >/dev/null 2>&1 || true

inner='
set +e
stdin_fifo=$1
stdout_fifo=$2
stderr_fifo=$3
status_file=$4
program=$5
shift 5
program_stdout_fifo="${stdout_fifo}.program"
program_stderr_fifo="${stderr_fifo}.program"
rm -f "$program_stdout_fifo" "$program_stderr_fifo"
mkfifo "$program_stdout_fifo" "$program_stderr_fifo"
tee "$stdout_fifo" < "$program_stdout_fifo" &
stdout_tee_pid=$!
tee "$stderr_fifo" < "$program_stderr_fifo" >&2 &
stderr_tee_pid=$!
"$program" "$@" < "$stdin_fifo" > "$program_stdout_fifo" 2> "$program_stderr_fifo"
code=$?
wait "$stdout_tee_pid" "$stderr_tee_pid"
printf "%s\n" "$code" > "$status_file"
exit "$code"
'

gate_env=()
if [ "${CSA_NO_POST_EXEC_GATE+x}" = x ]; then
  gate_env=(env "CSA_NO_POST_EXEC_GATE=$CSA_NO_POST_EXEC_GATE")
else
  gate_env=(env -u CSA_NO_POST_EXEC_GATE)
fi

if ! tmux new-session -d -s "$session_name" -c "$work_dir" -- \
  "${gate_env[@]}" bash -c "$inner" bash "$stdin_fifo" "$stdout_fifo" "$stderr_fifo" "$status_file" "$program" "$@"; then
  echo "failed to start codex tmux session: $session_name" >&2
  exit 127
fi

cat "$stdout_fifo" &
stdout_pid=$!
cat "$stderr_fifo" >&2 &
stderr_pid=$!

cat > "$stdin_fifo" &
stdin_pid=$!

while tmux has-session -t "$session_name" >/dev/null 2>&1; do
  sleep 0.2
done

kill "$stdin_pid" 2>/dev/null || true
wait "$stdin_pid" 2>/dev/null || true
wait "$stdout_pid" "$stderr_pid" 2>/dev/null || true

if [ -f "$status_file" ]; then
  read -r code < "$status_file"
  exit "${code:-1}"
fi

exit 1
"#;

pub(crate) fn wrap_codex_command_for_tmux(cmd: Command, session: &MetaSessionState) -> Command {
    let program = cmd.as_std().get_program().to_os_string();
    let args = cmd
        .as_std()
        .get_args()
        .map(OsString::from)
        .collect::<Vec<_>>();
    let current_dir = cmd
        .as_std()
        .get_current_dir()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(&session.project_path));
    let envs = cmd
        .as_std()
        .get_envs()
        .map(|(key, value)| (key.to_os_string(), value.map(|v| v.to_os_string())))
        .collect::<Vec<_>>();
    let session_name = format!("csa-{}", session.meta_session_id);

    let mut wrapped = Command::new("bash");
    wrapped
        .arg("-c")
        .arg(WRAPPER_SCRIPT)
        .arg("csa-codex-tmux")
        .arg(session_name)
        .arg(current_dir)
        .arg(program)
        .args(args);

    for (key, value) in envs {
        match value {
            Some(value) => {
                wrapped.env(key, value);
            }
            None => {
                wrapped.env_remove(key);
            }
        }
    }

    wrapped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(project_path: &Path) -> MetaSessionState {
        session_named(project_path, "01KTESTCODEXTMUXMODE000000")
    }

    fn session_named(project_path: &Path, meta_session_id: &str) -> MetaSessionState {
        MetaSessionState {
            meta_session_id: meta_session_id.to_string(),
            project_path: project_path.to_string_lossy().into_owned(),
            created_at: chrono::Utc::now(),
            last_accessed: chrono::Utc::now(),
            ..Default::default()
        }
    }

    fn dump_gate_script(path: &Path, ready_token: Option<&str>) -> String {
        let ready_signal = ready_token
            .map(|token| format!("; tmux wait-for -S '{token}'"))
            .unwrap_or_default();
        format!(
            r#"if [ "${{CSA_NO_POST_EXEC_GATE+x}}" = x ]; then printf 'set:%s\n' "$CSA_NO_POST_EXEC_GATE"; else printf 'unset\n'; fi > '{}'{ready_signal}"#,
            path.display()
        )
    }

    async fn wait_for_tmux_event(tmux_tmpdir: &Path, token: &str) {
        let mut wait = Command::new("tmux");
        wait.env("TMUX_TMPDIR", tmux_tmpdir)
            .env_remove("TMUX")
            .args(["wait-for", token]);
        let status = tokio::time::timeout(std::time::Duration::from_secs(10), wait.status())
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for tmux event {token}"))
            .unwrap_or_else(|err| panic!("wait for tmux event {token}: {err}"));
        assert!(status.success(), "tmux wait-for {token} failed: {status}");
    }

    struct TmuxServerCleanup {
        tmux_tmpdir: PathBuf,
    }

    impl Drop for TmuxServerCleanup {
        fn drop(&mut self) {
            let mut kill = std::process::Command::new("tmux");
            kill.env("TMUX_TMPDIR", &self.tmux_tmpdir)
                .env_remove("TMUX")
                .arg("kill-server");
            let _ = kill.status();
        }
    }

    fn read_gate_dump(path: &Path) -> String {
        std::fs::read_to_string(path)
            .unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
            .trim()
            .to_string()
    }

    struct RestoreTmuxGlobalGate {
        tmux_tmpdir: PathBuf,
        previous: Option<String>,
    }

    impl Drop for RestoreTmuxGlobalGate {
        fn drop(&mut self) {
            let mut cmd = std::process::Command::new("tmux");
            cmd.env("TMUX_TMPDIR", &self.tmux_tmpdir).env_remove("TMUX");
            cmd.args(["set-environment", "-g"]);
            match &self.previous {
                Some(value) => {
                    cmd.args([csa_core::env::CSA_NO_POST_EXEC_GATE_ENV_KEY, value]);
                }
                None => {
                    cmd.args(["-u", csa_core::env::CSA_NO_POST_EXEC_GATE_ENV_KEY]);
                }
            }
            let _ = cmd.status();
            let mut kill = std::process::Command::new("tmux");
            kill.env("TMUX_TMPDIR", &self.tmux_tmpdir)
                .env_remove("TMUX")
                .arg("kill-server");
            let _ = kill.status();
        }
    }

    fn poison_tmux_global_gate(tmux_tmpdir: &Path) -> RestoreTmuxGlobalGate {
        let show = std::process::Command::new("tmux")
            .env("TMUX_TMPDIR", tmux_tmpdir)
            .env_remove("TMUX")
            .args([
                "show-environment",
                "-g",
                csa_core::env::CSA_NO_POST_EXEC_GATE_ENV_KEY,
            ])
            .output()
            .expect("tmux show-environment -g");
        let previous = if show.status.success() {
            let text = String::from_utf8_lossy(&show.stdout);
            text.strip_prefix(&format!(
                "{}=",
                csa_core::env::CSA_NO_POST_EXEC_GATE_ENV_KEY
            ))
            .map(|value| value.trim().to_string())
        } else {
            None
        };
        let status = std::process::Command::new("tmux")
            .env("TMUX_TMPDIR", tmux_tmpdir)
            .env_remove("TMUX")
            .args([
                "set-environment",
                "-g",
                csa_core::env::CSA_NO_POST_EXEC_GATE_ENV_KEY,
                "1",
            ])
            .status()
            .expect("tmux set-environment -g poison");
        assert!(status.success(), "failed to poison tmux global env");
        RestoreTmuxGlobalGate {
            tmux_tmpdir: tmux_tmpdir.to_path_buf(),
            previous,
        }
    }

    #[test]
    fn wrapper_uses_csa_session_ulid_for_tmux_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut cmd = Command::new("codex");
        cmd.arg("exec").arg("--json").arg("hello");
        cmd.env("CSA_SESSION_DIR", dir.path());

        let wrapped = wrap_codex_command_for_tmux(cmd, &session(dir.path()));
        let args = wrapped
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert!(
            args.iter()
                .any(|arg| arg == "csa-01KTESTCODEXTMUXMODE000000")
        );
        assert!(args.iter().any(|arg| arg == "codex"));
        assert!(args.iter().any(|arg| arg == "exec"));
    }

    struct TmuxGateTestLock {
        path: PathBuf,
    }

    impl Drop for TmuxGateTestLock {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir(&self.path);
        }
    }

    fn lock_tmux_gate_test() -> TmuxGateTestLock {
        let path = std::env::temp_dir().join("csa-tmux-no-post-exec-gate-test.lockdir");
        for _ in 0..200 {
            if std::fs::create_dir(&path).is_ok() {
                return TmuxGateTestLock { path };
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("timed out waiting for tmux gate test lock");
    }

    fn unique_session_id(tag: &str) -> String {
        format!(
            "01T{:08}{:010}{tag}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_millis()
                % 10_000_000_000u128
        )
    }

    async fn run_wrapped_dump(
        session_id: &str,
        no_post_exec_gate: bool,
        dump: &Path,
        session_dir: &Path,
        tmux_tmpdir: &Path,
    ) -> String {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(dump_gate_script(dump, None));
        cmd.env("CSA_SESSION_DIR", session_dir);
        cmd.env("TMUX_TMPDIR", tmux_tmpdir).env_remove("TMUX");
        crate::executor::executor_env::apply_no_post_exec_gate(&mut cmd, no_post_exec_gate);
        let mut wrapped = wrap_codex_command_for_tmux(cmd, &session_named(session_dir, session_id));
        wrapped.stdin(std::process::Stdio::null());
        wrapped.stdout(std::process::Stdio::null());
        wrapped.stderr(std::process::Stdio::null());
        let status = tokio::time::timeout(std::time::Duration::from_secs(15), wrapped.status())
            .await
            .unwrap_or_else(|_| panic!("codex tmux wrap timed out for {session_id}"))
            .expect("spawn wrapped dump");
        assert!(
            status.success(),
            "wrapped dump failed for {session_id}: {status}"
        );
        read_gate_dump(dump)
    }

    async fn run_direct_tmux_dump(
        session_name: &str,
        no_post_exec_gate: bool,
        dump: &Path,
        work_dir: &Path,
        tmux_tmpdir: &Path,
    ) -> String {
        let ready_token = format!("csa-tmux-inner-ready-{session_name}");
        let work_dir_str = work_dir.to_str().expect("utf8 work_dir");
        let dump_args = vec![
            "sh".to_string(),
            "-c".to_string(),
            dump_gate_script(dump, Some(&ready_token)),
        ];
        let inner = crate::executor::executor_env::tmux_inner_child_with_gate(
            no_post_exec_gate,
            &dump_args,
        );
        let mut cmd = crate::executor::executor_env::tmux_new_session_command(
            session_name,
            work_dir_str,
            None,
            no_post_exec_gate,
            &inner,
        );
        cmd.env("TMUX_TMPDIR", tmux_tmpdir).env_remove("TMUX");
        let status = cmd.status().await.expect("direct tmux new-session");
        assert!(status.success(), "direct tmux new-session failed: {status}");
        wait_for_tmux_event(tmux_tmpdir, &ready_token).await;
        let dump_value = read_gate_dump(dump);
        let _ = std::process::Command::new("tmux")
            .env("TMUX_TMPDIR", tmux_tmpdir)
            .env_remove("TMUX")
            .args(["kill-session", "-t", session_name])
            .status();
        dump_value
    }

    fn global_gate_value(tmux_tmpdir: &Path) -> Option<String> {
        let show = std::process::Command::new("tmux")
            .env("TMUX_TMPDIR", tmux_tmpdir)
            .env_remove("TMUX")
            .args([
                "show-environment",
                "-g",
                csa_core::env::CSA_NO_POST_EXEC_GATE_ENV_KEY,
            ])
            .output()
            .expect("tmux show-environment -g");
        if !show.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&show.stdout);
        text.strip_prefix(&format!(
            "{}=",
            csa_core::env::CSA_NO_POST_EXEC_GATE_ENV_KEY
        ))
        .map(|value| value.trim().to_string())
    }

    #[tokio::test]
    async fn tmux_inner_child_isolates_no_post_exec_gate_from_global_server_env() {
        which::which("tmux").expect("tmux must be on PATH");
        let _lock = lock_tmux_gate_test();
        let tmux_tmpdir = tempfile::tempdir().expect("tmux tempdir");
        let dir = tempfile::tempdir().expect("tempdir");
        let _cleanup = TmuxServerCleanup {
            tmux_tmpdir: tmux_tmpdir.path().to_path_buf(),
        };
        let fixture_ready = "csa-tmux-gate-fixture-ready";
        let started = std::process::Command::new("tmux")
            .env("TMUX_TMPDIR", tmux_tmpdir.path())
            .env_remove("TMUX")
            .args(["new-session", "-d", "-s", "csa-tmux-gate-fixture", "-c"])
            .arg(dir.path())
            .args(["--", "sh", "-c"])
            .arg(format!("tmux wait-for -S '{fixture_ready}'; exec sh"))
            .status()
            .expect("tmux new-session");
        assert!(started.success(), "tmux new-session failed: {started}");
        wait_for_tmux_event(tmux_tmpdir.path(), fixture_ready).await;
        let _restore = poison_tmux_global_gate(tmux_tmpdir.path());
        assert_eq!(global_gate_value(tmux_tmpdir.path()).as_deref(), Some("1"));

        let work_dir = dir.path();

        let enabled_direct = work_dir.join("enabled-direct");
        let disabled_legacy = work_dir.join("disabled-legacy");
        let disabled_direct = work_dir.join("disabled-direct");
        let enabled_legacy = work_dir.join("enabled-legacy");
        let disabled_legacy_fallback = work_dir.join("disabled-legacy-fallback");

        assert_eq!(
            run_direct_tmux_dump(
                &format!("csa-{}", unique_session_id("ed")),
                true,
                &enabled_direct,
                work_dir,
                tmux_tmpdir.path(),
            )
            .await,
            "set:1",
            "enabled direct tmux inner child must have the gate flag"
        );
        assert_eq!(
            global_gate_value(tmux_tmpdir.path()).as_deref(),
            Some("1"),
            "direct tmux must not restore or rewrite the pre-existing global server env"
        );

        assert_eq!(
            run_wrapped_dump(
                &unique_session_id("dl"),
                false,
                &disabled_legacy,
                work_dir,
                tmux_tmpdir.path(),
            )
            .await,
            "unset",
            "disabled legacy Codex tmux inner child must not inherit a poisoned global flag"
        );

        assert_eq!(
            run_direct_tmux_dump(
                &format!("csa-{}", unique_session_id("dd")),
                false,
                &disabled_direct,
                work_dir,
                tmux_tmpdir.path(),
            )
            .await,
            "unset",
            "disabled direct tmux inner child must clear a poisoned global flag"
        );

        assert_eq!(
            run_wrapped_dump(
                &unique_session_id("el"),
                true,
                &enabled_legacy,
                work_dir,
                tmux_tmpdir.path(),
            )
            .await,
            "set:1",
            "enabled legacy Codex tmux inner child must bind the gate flag"
        );

        assert_eq!(
            run_wrapped_dump(
                &unique_session_id("fb"),
                false,
                &disabled_legacy_fallback,
                work_dir,
                tmux_tmpdir.path(),
            )
            .await,
            "unset",
            "best-effort fallback uses the same wrap; inner child must stay isolated"
        );
        assert_eq!(
            global_gate_value(tmux_tmpdir.path()).as_deref(),
            Some("1"),
            "legacy Codex tmux must not leave CSA_NO_POST_EXEC_GATE in the global server env"
        );
    }
}
