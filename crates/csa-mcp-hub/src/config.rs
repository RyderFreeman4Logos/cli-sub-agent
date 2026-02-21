use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use csa_config::{GlobalConfig, McpServerConfig};

const DEFAULT_HTTP_BIND: &str = "127.0.0.1";
const DEFAULT_HTTP_PORT: u16 = 0;
const DEFAULT_MAX_CONNECTIONS: usize = 32;
const DEFAULT_MAX_REQUESTS_PER_SEC: u32 = 100;
const DEFAULT_MAX_REQUEST_BODY_BYTES: usize = 10 * 1024 * 1024;
const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Clone)]
pub(crate) struct HubConfig {
    pub(crate) project_root: PathBuf,
    pub(crate) socket_path: PathBuf,
    pub(crate) pid_path: PathBuf,
    pub(crate) mcp_servers: Vec<McpServerConfig>,
    pub(crate) mcp_whitelist: Vec<String>,
    pub(crate) mcp_blacklist: Vec<String>,
    pub(crate) http_bind: String,
    pub(crate) http_port: u16,
    pub(crate) max_connections: usize,
    pub(crate) max_requests_per_sec: u32,
    pub(crate) max_request_body_bytes: usize,
    pub(crate) request_timeout_secs: u64,
}

impl HubConfig {
    pub(crate) fn load(
        socket_override: Option<PathBuf>,
        http_bind_override: Option<String>,
        http_port_override: Option<u16>,
    ) -> Result<Self> {
        let global = GlobalConfig::load()?;
        let cwd = std::env::current_dir().context("failed to resolve current working directory")?;
        let project_root = discover_project_root(&cwd);
        let (mcp_whitelist, mcp_blacklist) = load_project_mcp_visibility(&project_root)?;
        Ok(Self::from_parts(
            &global,
            project_root,
            socket_override,
            http_bind_override,
            http_port_override,
            mcp_whitelist,
            mcp_blacklist,
        ))
    }

    #[cfg(test)]
    pub(crate) fn from_global_config(
        global: &GlobalConfig,
        socket_override: Option<PathBuf>,
        http_bind_override: Option<String>,
        http_port_override: Option<u16>,
    ) -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let project_root = discover_project_root(&cwd);
        Self::from_parts(
            global,
            project_root,
            socket_override,
            http_bind_override,
            http_port_override,
            Vec::new(),
            Vec::new(),
        )
    }

    fn from_parts(
        global: &GlobalConfig,
        project_root: PathBuf,
        socket_override: Option<PathBuf>,
        http_bind_override: Option<String>,
        http_port_override: Option<u16>,
        mcp_whitelist: Vec<String>,
        mcp_blacklist: Vec<String>,
    ) -> Self {
        let socket_path = socket_override
            .or_else(|| global.mcp_proxy_socket.clone().map(PathBuf::from))
            .unwrap_or_else(default_socket_path);
        let pid_path = pid_path_for_socket(&socket_path);
        Self {
            project_root,
            socket_path,
            pid_path,
            mcp_servers: global.mcp_servers().to_vec(),
            mcp_whitelist,
            mcp_blacklist,
            http_bind: http_bind_override.unwrap_or_else(|| DEFAULT_HTTP_BIND.to_string()),
            http_port: http_port_override.unwrap_or(DEFAULT_HTTP_PORT),
            max_connections: DEFAULT_MAX_CONNECTIONS,
            max_requests_per_sec: DEFAULT_MAX_REQUESTS_PER_SEC,
            max_request_body_bytes: DEFAULT_MAX_REQUEST_BODY_BYTES,
            request_timeout_secs: DEFAULT_REQUEST_TIMEOUT_SECS,
        }
    }

    pub(crate) fn request_timeout(&self) -> Duration {
        Duration::from_secs(self.request_timeout_secs)
    }
}

fn discover_project_root(start: &Path) -> PathBuf {
    for candidate in start.ancestors() {
        if is_project_root_marker(candidate) {
            return candidate.to_path_buf();
        }
    }
    start.to_path_buf()
}

fn is_project_root_marker(candidate: &Path) -> bool {
    candidate.join(".csa").join("config.toml").is_file()
        || candidate.join(".csa").is_dir()
        || candidate.join(".git").is_dir()
        || candidate.join("Cargo.toml").is_file()
}

fn load_project_mcp_visibility(project_root: &Path) -> Result<(Vec<String>, Vec<String>)> {
    let path = project_root.join(".csa").join("config.toml");
    if !path.exists() {
        return Ok((Vec::new(), Vec::new()));
    }

    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read project config: {}", path.display()))?;
    let value: toml::Value = toml::from_str(&raw)
        .with_context(|| format!("failed to parse project config: {}", path.display()))?;

    Ok((
        read_string_list(&value, "mcp_whitelist"),
        read_string_list(&value, "mcp_blacklist"),
    ))
}

fn read_string_list(value: &toml::Value, key: &str) -> Vec<String> {
    let Some(items) = value.get(key).and_then(toml::Value::as_array) else {
        return Vec::new();
    };

    items
        .iter()
        .filter_map(toml::Value::as_str)
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect()
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
    use super::{
        DEFAULT_HTTP_BIND, DEFAULT_HTTP_PORT, HubConfig, discover_project_root,
        load_project_mcp_visibility, pid_path_for_socket, socket_path_from_runtime_dir,
    };

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

    #[test]
    fn config_uses_default_http_binding() {
        let cfg =
            HubConfig::from_global_config(&csa_config::GlobalConfig::default(), None, None, None);
        assert_eq!(cfg.http_bind, DEFAULT_HTTP_BIND);
        assert_eq!(cfg.http_port, DEFAULT_HTTP_PORT);
    }

    #[test]
    fn config_allows_http_override() {
        let cfg = HubConfig::from_global_config(
            &csa_config::GlobalConfig::default(),
            None,
            Some("127.0.0.2".to_string()),
            Some(61234),
        );
        assert_eq!(cfg.http_bind, "127.0.0.2");
        assert_eq!(cfg.http_port, 61234);
    }

    #[test]
    fn discover_project_root_walks_ancestor_chain() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project_root = tmp.path().join("project");
        let nested = project_root.join("a").join("b");
        std::fs::create_dir_all(project_root.join(".csa")).expect("create .csa");
        std::fs::write(project_root.join(".csa/config.toml"), "").expect("write config");
        std::fs::create_dir_all(&nested).expect("create nested");

        let resolved = discover_project_root(&nested);
        assert_eq!(resolved, project_root);
    }

    #[test]
    fn discover_project_root_accepts_csa_directory_marker() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project_root = tmp.path().join("project");
        let nested = project_root.join("nested");
        std::fs::create_dir_all(project_root.join(".csa")).expect("create .csa");
        std::fs::create_dir_all(&nested).expect("create nested");

        let resolved = discover_project_root(&nested);
        assert_eq!(resolved, project_root);
    }

    #[test]
    fn discover_project_root_accepts_git_directory_marker() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project_root = tmp.path().join("project");
        let nested = project_root.join("nested");
        std::fs::create_dir_all(project_root.join(".git")).expect("create .git");
        std::fs::create_dir_all(&nested).expect("create nested");

        let resolved = discover_project_root(&nested);
        assert_eq!(resolved, project_root);
    }

    #[test]
    fn discover_project_root_accepts_cargo_toml_marker() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project_root = tmp.path().join("project");
        let nested = project_root.join("nested");
        std::fs::create_dir_all(&nested).expect("create nested");
        std::fs::write(
            project_root.join("Cargo.toml"),
            "[package]\nname = \"x\"\nversion = \"0.1.0\"\n",
        )
        .expect("write Cargo.toml");

        let resolved = discover_project_root(&nested);
        assert_eq!(resolved, project_root);
    }

    #[test]
    fn load_project_mcp_visibility_reads_filters() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project_root = tmp.path().join("project");
        std::fs::create_dir_all(project_root.join(".csa")).expect("create .csa");
        std::fs::write(
            project_root.join(".csa/config.toml"),
            r#"
mcp_whitelist = ["repomix", "deepwiki"]
mcp_blacklist = ["memory"]
"#,
        )
        .expect("write config");

        let (whitelist, blacklist) =
            load_project_mcp_visibility(&project_root).expect("load visibility");
        assert_eq!(whitelist, vec!["repomix", "deepwiki"]);
        assert_eq!(blacklist, vec!["memory"]);
    }
}
