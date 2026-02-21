#[cfg(not(unix))]
compile_error!("mcp-hub requires Unix domain sockets; Windows is not supported");

use std::path::Path;

use anyhow::{Context, Result, bail};
use tokio::net::{UnixListener, UnixStream};

pub(crate) async fn bind_listener(socket_path: &Path) -> Result<UnixListener> {
    if let Some(parent) = socket_path.parent() {
        let parent_existed = parent.exists();
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create socket parent: {}", parent.display()))?;
        if !parent_existed {
            set_permissions(parent, 0o700).await?;
        }
    }

    if socket_path.exists() {
        tokio::fs::remove_file(socket_path)
            .await
            .with_context(|| format!("failed to remove stale socket: {}", socket_path.display()))?;
    }

    let listener = UnixListener::bind(socket_path)
        .with_context(|| format!("failed to bind unix socket: {}", socket_path.display()))?;
    set_permissions(socket_path, 0o600).await?;
    Ok(listener)
}

pub(crate) async fn connect(socket_path: &Path) -> Result<UnixStream> {
    UnixStream::connect(socket_path)
        .await
        .with_context(|| format!("failed to connect unix socket: {}", socket_path.display()))
}

pub(crate) async fn cleanup_socket_file(socket_path: &Path) -> Result<()> {
    if socket_path.exists() {
        tokio::fs::remove_file(socket_path)
            .await
            .with_context(|| format!("failed to cleanup socket: {}", socket_path.display()))?;
    }
    Ok(())
}

#[cfg(unix)]
async fn set_permissions(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .await
        .with_context(|| format!("failed to chmod {:o}: {}", mode, path.display()))
}

#[cfg(target_os = "linux")]
pub(crate) fn bind_systemd_activated_listener() -> Result<Option<UnixListener>> {
    let listen_fds = std::env::var("LISTEN_FDS")
        .ok()
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(0);
    if listen_fds <= 0 {
        return Ok(None);
    }

    let listen_pid = std::env::var("LISTEN_PID")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0);
    if listen_pid != std::process::id() {
        return Ok(None);
    }

    if listen_fds != 1 {
        bail!("expected exactly one LISTEN_FD for mcp-hub, got {listen_fds}");
    }

    const SD_LISTEN_FDS_START: i32 = 3;
    let fd = SD_LISTEN_FDS_START;

    // SAFETY: reading and updating fd flags via fcntl on inherited systemd socket fd.
    let current_flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if current_flags >= 0 {
        // SAFETY: setting O_NONBLOCK bit on inherited socket fd.
        let _ = unsafe { libc::fcntl(fd, libc::F_SETFL, current_flags | libc::O_NONBLOCK) };
    }

    // SAFETY: fd ownership is transferred exactly once from systemd to std listener.
    let std_listener = unsafe {
        use std::os::fd::FromRawFd;
        std::os::unix::net::UnixListener::from_raw_fd(fd)
    };
    std_listener
        .set_nonblocking(true)
        .context("failed to set nonblocking on systemd socket fd")?;

    let listener = UnixListener::from_std(std_listener)
        .context("failed to construct tokio UnixListener from systemd socket")?;
    Ok(Some(listener))
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn bind_systemd_activated_listener() -> Result<Option<UnixListener>> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    #[tokio::test]
    async fn bind_and_connect_round_trip() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let socket_path = dir.path().join("mcp-hub.sock");
        let listener = super::bind_listener(&socket_path).await?;

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept client");
            let (read_half, mut write_half) = stream.into_split();
            let mut reader = BufReader::new(read_half);
            let mut line = String::new();
            reader
                .read_line(&mut line)
                .await
                .expect("read request line");
            write_half
                .write_all(b"{\"ok\":true}\n")
                .await
                .expect("write response");
        });

        let mut client = super::connect(&socket_path).await?;
        client.write_all(b"ping\n").await?;

        let mut response = String::new();
        let mut client_reader = BufReader::new(client);
        client_reader.read_line(&mut response).await?;

        server.await?;
        assert_eq!(response.trim(), "{\"ok\":true}");

        super::cleanup_socket_file(&socket_path).await?;
        assert!(!socket_path.exists());

        Ok(())
    }

    #[tokio::test]
    async fn bind_listener_sets_restrictive_permissions() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir()?;
        let socket_path = dir.path().join("private").join("mcp-hub.sock");
        let _listener = super::bind_listener(&socket_path).await?;

        let socket_mode = std::fs::metadata(&socket_path)?.permissions().mode() & 0o777;
        let parent = socket_path.parent().expect("socket parent");
        let parent_mode = std::fs::metadata(parent)?.permissions().mode() & 0o777;

        assert_eq!(socket_mode, 0o600);
        assert_eq!(parent_mode, 0o700);
        Ok(())
    }

    #[tokio::test]
    async fn bind_listener_does_not_chmod_existing_parent_directory() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir()?;
        let shared_parent = dir.path().join("shared");
        std::fs::create_dir(&shared_parent)?;
        std::fs::set_permissions(&shared_parent, std::fs::Permissions::from_mode(0o755))?;

        let socket_path = shared_parent.join("mcp-hub.sock");
        let _listener = super::bind_listener(&socket_path).await?;

        let parent_mode = std::fs::metadata(&shared_parent)?.permissions().mode() & 0o777;
        assert_eq!(parent_mode, 0o755);
        Ok(())
    }
}
