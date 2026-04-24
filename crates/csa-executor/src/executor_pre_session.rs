use std::borrow::Cow;

use csa_session::state::MetaSessionState;

use super::{ExecuteOptions, Executor};

impl Executor {
    pub(crate) fn apply_pre_session_hook<'a>(
        &self,
        prompt: &'a str,
        session: &MetaSessionState,
        options: &ExecuteOptions,
    ) -> Cow<'a, str> {
        let Some(config) = options.pre_session_hook.as_ref() else {
            return Cow::Borrowed(prompt);
        };
        let working_dir = std::env::current_dir()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| session.project_path.clone());
        let context = csa_hooks::PreSessionHookContext {
            session_id: &session.meta_session_id,
            transport: self.tool_name(),
            project_root: &session.project_path,
            working_dir: &working_dir,
            user_prompt: prompt,
        };

        csa_hooks::run_pre_session_hook(config, &context).map_or(Cow::Borrowed(prompt), Cow::Owned)
    }
}
