use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::Result;
use csa_config::{GlobalConfig, McpServerConfig};

#[derive(Debug, Clone)]
pub(crate) struct HubConfig {
    pub(crate) socket_path: PathBuf,
    pub(crate) pid_path: PathBuf,
    pub(crate) mcp_servers: Vec<McpServerConfig>,
}

impl HubConfig {
    pub(crate) fn load(socket_override: Option<PathBuf>) -> Result<Self> {
        let global = GlobalConfig::load()?;
        Ok(Self::from_global_config(&global, socket_override))
    }

    pub(crate) fn from_global_config(
        global: &GlobalConfig,
        socket_override: Option<PathBuf>,
    ) -> Self {
        let socket_path = socket_override
            .or_else(|| global.mcp_proxy_socket.clone().map(PathBuf::from))
            .unwrap_or_else(default_socket_path);
        let pid_path = pid_path_for_socket(&socket_path);
        Self {
            socket_path,
            pid_path,
            mcp_servers: global.mcp_servers().to_vec(),
        }
    }
}

pub(crate) fn default_socket_path() -> PathBuf {
    socket_path_from_runtime_dir(
        std::env::var("XDG_RUNTIME_DIR").ok().as_deref(),
        effective_uid(),
    )
}

pub(crate) fn pid_path_for_socket(socket_path: &Path) -> PathBuf {
    let mut buf: OsString = socket_path.as_os_str().to_owned();
    buf.push(".pid");
    PathBuf::from(buf)
}

fn effective_uid() -> u32 {
    #[cfg(unix)]
    {
        // SAFETY: `geteuid` has no preconditions and returns caller effective UID.
        unsafe { libc::geteuid() }
    }

    #[cfg(not(unix))]
    {
        0
    }
}

fn socket_path_from_runtime_dir(runtime_dir: Option<&str>, uid: u32) -> PathBuf {
    if let Some(runtime_dir) = runtime_dir {
        return PathBuf::from(runtime_dir).join("csa").join("mcp-hub.sock");
    }

    PathBuf::from("/tmp")
        .join(format!("csa-{uid}"))
        .join("mcp-hub.sock")
}

#[cfg(test)]
mod tests {
    use super::{pid_path_for_socket, socket_path_from_runtime_dir};

    #[test]
    fn default_socket_path_prefers_xdg_runtime_dir() {
        let path = socket_path_from_runtime_dir(Some("/tmp/xdg-test"), 1000);

        assert_eq!(path, std::path::Path::new("/tmp/xdg-test/csa/mcp-hub.sock"));
    }

    #[test]
    fn default_socket_path_falls_back_to_tmp_with_uid() {
        let path = socket_path_from_runtime_dir(None, 1001);

        let path_string = path.to_string_lossy();
        assert!(
            path_string.contains("/tmp/csa-"),
            "expected /tmp fallback path, got {path_string}"
        );
        assert!(path_string.ends_with("/mcp-hub.sock"));
    }

    #[test]
    fn pid_path_appends_pid_suffix() {
        let socket = std::path::Path::new("/tmp/csa-1000/mcp-hub.sock");
        let pid = pid_path_for_socket(socket);
        assert_eq!(pid, std::path::Path::new("/tmp/csa-1000/mcp-hub.sock.pid"));
    }
}
