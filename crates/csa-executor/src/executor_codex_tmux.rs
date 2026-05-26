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
"$program" "$@" < "$stdin_fifo" > >(tee "$stdout_fifo") 2> >(tee "$stderr_fifo" >&2)
code=$?
printf "%s\n" "$code" > "$status_file"
exit "$code"
'

if ! tmux new-session -d -s "$session_name" -c "$work_dir" -- \
  bash -c "$inner" bash "$stdin_fifo" "$stdout_fifo" "$stderr_fifo" "$status_file" "$program" "$@"; then
  echo "failed to start codex tmux session: $session_name" >&2
  exit 127
fi

cat "$stdout_fifo" &
stdout_pid=$!
cat "$stderr_fifo" >&2 &
stderr_pid=$!

cat > "$stdin_fifo" || true

while tmux has-session -t "$session_name" >/dev/null 2>&1; do
  sleep 0.2
done

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
        MetaSessionState {
            meta_session_id: "01KTESTCODEXTMUXMODE000000".to_string(),
            project_path: project_path.to_string_lossy().into_owned(),
            created_at: chrono::Utc::now(),
            last_accessed: chrono::Utc::now(),
            ..Default::default()
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
}
