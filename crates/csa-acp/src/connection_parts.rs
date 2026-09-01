use super::*;

pub(crate) struct AcpConnectionParts {
    pub(crate) local_set: LocalSet,
    pub(crate) connection: ClientSideConnection,
    pub(crate) child: Child,
    pub(crate) events: SharedEvents,
    pub(crate) last_activity: SharedActivity,
    pub(crate) last_meaningful_activity: SharedActivity,
    pub(crate) tool_output_compactor: SharedToolOutputCompactor,
    pub(crate) stderr_buf: Rc<RefCell<String>>,
    pub(crate) stderr_closed: Rc<std::cell::Cell<bool>>,
    pub(crate) default_working_dir: PathBuf,
    pub(crate) options: AcpConnectionOptions,
}
