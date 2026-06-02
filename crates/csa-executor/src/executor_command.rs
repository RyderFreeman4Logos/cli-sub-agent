use std::ffi::OsString;

use super::*;

impl Executor {
    /// Inject environment variables from a generic (user/request/config) env map
    /// into a Command.
    ///
    /// Keys present in [`Self::STRIPPED_ENV_VARS`] are skipped so that
    /// generic-map values cannot re-introduce a freshly-stripped recursion
    /// guard, session-scoped var, startup-subtree contract key, or hook-bypass
    /// switch. A generic env map may NEVER set subtree contract keys, so caller
    /// request/config env cannot spoof a CSA subtree (#1750). CSA-owned session
    /// values and the authoritative subtree pin are applied separately, AFTER
    /// this merge, through typed channels.
    pub fn inject_env(cmd: &mut Command, env_vars: &HashMap<String, String>) {
        let mut env_vars = env_vars.clone();
        csa_core::env::scrub_subtree_contract_env_map(&mut env_vars);
        for (key, value) in &env_vars {
            if !Self::STRIPPED_ENV_VARS.contains(&key.as_str()) {
                cmd.env(key, value);
            }
        }
    }

    /// Build a configured Command ready for execution (without spawning).
    ///
    /// `subtree_pin` carries CSA's authoritative subtree model pin (#1741) and
    /// is applied LAST (after the generic `extra_env` merge, which strips the
    /// pin keys) so it is the sole, unforgeable writer of those keys.
    pub fn build_command(
        &self,
        prompt: &str,
        tool_state: Option<&ToolState>,
        session: &MetaSessionState,
        extra_env: Option<&HashMap<String, String>>,
        subtree_pin: Option<&csa_core::env::SubtreeModelPin>,
    ) -> (Command, Option<Vec<u8>>) {
        // Prepend CSA identity preamble for claude-code (#1397).
        let preamble_buf;
        let prompt = match self {
            Self::ClaudeCode { .. } => {
                preamble_buf =
                    format!("{}{prompt}", Self::csa_sub_agent_identity_preamble(session));
                preamble_buf.as_str()
            }
            _ => prompt,
        };
        let mut cmd = self.build_base_command(session);
        if matches!(self, Self::GeminiCli { .. } | Self::AntigravityCli { .. }) {
            Self::strip_gemini_inherited_env(&mut cmd);
        }
        if let Some(env) = extra_env {
            Self::inject_env(&mut cmd, env);
        }
        self.inject_csa_owned_env(&mut cmd, session);
        // #1741: apply CSA's trusted subtree pin LAST, after every generic env
        // merge (which stripped the pin keys). This is the only writer of the
        // pin keys, so user/request/config env can never spoof a pin.
        executor_env::apply_subtree_pin(&mut cmd, subtree_pin);
        executor_env::inject_git_guard_env(&mut cmd);
        let gemini_include_directories =
            gemini_include_directories(extra_env, prompt, Some(Path::new(&session.project_path)));
        let (prompt_transport, stdin_data) = self.select_prompt_transport(prompt);
        self.append_tool_args_with_transport(
            &mut cmd,
            prompt,
            tool_state,
            prompt_transport,
            &gemini_include_directories,
        );
        if matches!(self, Self::Codex { .. }) {
            sanitize_env_for_codex(&mut cmd);
            cmd = Self::sanitize_codex_command_args(cmd);
            if self.codex_tmux_mode_enabled() {
                cmd = codex_tmux::wrap_codex_command_for_tmux(cmd, session);
            }
        }
        (cmd, stdin_data)
    }
    /// Build command for execute_in() legacy path.
    ///
    /// `subtree_pin` carries CSA's authoritative subtree model pin (#1741) and
    /// is applied LAST (after the generic `extra_env` merge, which strips the
    /// pin keys) so it is the sole, unforgeable writer of those keys.
    pub(crate) fn build_execute_in_command(
        &self,
        prompt: &str,
        work_dir: &Path,
        extra_env: Option<&HashMap<String, String>>,
        subtree_pin: Option<&csa_core::env::SubtreeModelPin>,
    ) -> (Command, Option<Vec<u8>>) {
        let mut cmd = Command::new(self.executable_name());
        cmd.current_dir(work_dir);
        // Strip recursive-invocation guard vars (same as build_base_command).
        for var in Self::STRIPPED_ENV_VARS {
            cmd.env_remove(var);
        }
        csa_core::env::scrub_subtree_contract_env_tokio(&mut cmd);
        if matches!(self, Self::GeminiCli { .. } | Self::AntigravityCli { .. }) {
            Self::strip_gemini_inherited_env(&mut cmd);
        }
        if let Some(env) = extra_env {
            Self::inject_env(&mut cmd, env);
        }
        // #1741: apply CSA's trusted subtree pin LAST (after the generic merge,
        // which stripped the pin keys) — the only writer of the pin keys.
        executor_env::apply_subtree_pin(&mut cmd, subtree_pin);
        executor_env::inject_git_guard_env(&mut cmd);
        let gemini_include_directories =
            gemini_include_directories(extra_env, prompt, Some(work_dir));
        self.append_yolo_args(&mut cmd);
        self.append_model_args(&mut cmd);
        if matches!(self, Self::GeminiCli { .. } | Self::AntigravityCli { .. }) {
            append_gemini_include_directories_args(&mut cmd, &gemini_include_directories);
        }
        if matches!(self, Self::Codex { .. })
            && let Some(env) = extra_env
        {
            cmd.args(codex_notify_suppression_args(env));
        }
        let (prompt_transport, stdin_data) = self.select_prompt_transport(prompt);
        if matches!(prompt_transport, PromptTransport::Argv) {
            self.append_prompt_args(&mut cmd, prompt);
        } else {
            self.append_prompt_args_with_transport(&mut cmd, prompt, prompt_transport);
        }
        if matches!(self, Self::Codex { .. }) {
            sanitize_env_for_codex(&mut cmd);
            cmd = Self::sanitize_codex_command_args(cmd);
        }
        (cmd, stdin_data)
    }

    fn sanitize_codex_command_args(cmd: Command) -> Command {
        let program = cmd.as_std().get_program().to_os_string();
        let current_dir = cmd.as_std().get_current_dir().map(|dir| dir.to_path_buf());
        let envs = cmd
            .as_std()
            .get_envs()
            .map(|(key, value)| (key.to_os_string(), value.map(|v| v.to_os_string())))
            .collect::<Vec<_>>();
        let mut args = cmd
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        sanitize_args_for_codex(&mut args);

        let mut sanitized = Command::new(program);
        if let Some(dir) = current_dir {
            sanitized.current_dir(dir);
        }
        for (key, value) in envs {
            match value {
                Some(value) => {
                    sanitized.env(key, value);
                }
                None => {
                    sanitized.env_remove(key);
                }
            }
        }
        let os_args = args.into_iter().map(OsString::from).collect::<Vec<_>>();
        sanitized.args(os_args);
        sanitized
    }
}
