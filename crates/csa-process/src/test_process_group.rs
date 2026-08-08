use std::os::unix::process::CommandExt;

/// Own a test fixture and clean up every process in its private group.
pub(crate) struct ProcessGroupFixture {
    child: Option<std::process::Child>,
}

impl ProcessGroupFixture {
    pub(crate) fn spawn(mut command: std::process::Command) -> Self {
        // SAFETY: test fixture only; gives the fixture a private process group.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Self {
            child: Some(command.spawn().expect("spawn process-group fixture")),
        }
    }

    pub(crate) fn id(&self) -> u32 {
        self.child
            .as_ref()
            .expect("fixture leader must remain unreaped")
            .id()
    }

    fn terminate_and_reap(&mut self) {
        let pid = self.id() as libc::pid_t;
        // SAFETY: the unreaped setsid leader owns this fixture's process group.
        let _ = unsafe { libc::kill(-pid, libc::SIGTERM) };
        std::thread::sleep(std::time::Duration::from_millis(100));
        // SAFETY: the leader remains unreaped, so its PID cannot have been reused.
        let _ = unsafe { libc::kill(-pid, libc::SIGKILL) };
        let _ = self
            .child
            .take()
            .expect("fixture leader must remain unreaped")
            .wait();
    }
}

impl Drop for ProcessGroupFixture {
    fn drop(&mut self) {
        if self.child.is_some() {
            self.terminate_and_reap();
        }
    }
}
