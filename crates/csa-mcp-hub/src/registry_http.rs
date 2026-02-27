//! HTTP URL safety validation for MCP transport.
//!
//! Performs scheme whitelisting, HTTPS enforcement, and pre-flight SSRF
//! protection (DNS resolution check against private/reserved IP ranges).

use anyhow::Result;

/// Validate that a URL is safe for outbound HTTP transport.
///
/// Checks performed:
/// - Scheme must be `http` or `https` (rejects `file://`, `data://`, `gopher://`, etc.)
/// - `http://` is rejected unless `allow_insecure` is explicitly set
pub(crate) fn validate_http_url(url: &str, allow_insecure: bool, server_name: &str) -> Result<()> {
    let scheme_end = url.find("://").ok_or_else(|| {
        anyhow::anyhow!(
            "MCP server '{server_name}': URL '{url}' has no scheme (expected https:// or http://)"
        )
    })?;
    let scheme = &url[..scheme_end].to_ascii_lowercase();

    match scheme.as_str() {
        "https" => Ok(()),
        "http" if allow_insecure => {
            tracing::warn!(
                server = %server_name,
                url = %url,
                "using insecure HTTP transport (allow_insecure = true)"
            );
            Ok(())
        }
        "http" => anyhow::bail!(
            "MCP server '{server_name}': HTTP transport requires HTTPS. \
             Set allow_insecure = true to allow plain HTTP."
        ),
        other => anyhow::bail!(
            "MCP server '{server_name}': unsupported URL scheme '{other}://'. \
             Only https:// (and http:// with allow_insecure) are supported."
        ),
    }
}

/// Pre-flight DNS resolution to catch obvious SSRF misconfig.
///
/// Resolves the URL's host and rejects connections to private/reserved IPs.
/// This is best-effort (TOCTOU with DNS rebinding), but catches the common case
/// of accidentally pointing HTTP transport at localhost or internal services.
pub(crate) fn preflight_ssrf_check(url: &str, server_name: &str) -> Result<()> {
    use std::net::ToSocketAddrs;

    let (host, port) = parse_host_port(url).unwrap_or_default();
    if host.is_empty() {
        return Ok(()); // unparseable host -- let the transport report the error
    }

    let socket_addr = format!("{host}:{port}");
    let addrs = match socket_addr.to_socket_addrs() {
        Ok(addrs) => addrs,
        Err(_) => return Ok(()), // DNS failure -- transport will report
    };

    for addr in addrs {
        let ip = addr.ip();
        if is_ssrf_dangerous_ip(ip) {
            anyhow::bail!(
                "MCP server '{server_name}': resolved IP {ip} is a private/reserved address \
                 (SSRF protection). Use stdio transport for local servers."
            );
        }
    }
    Ok(())
}

/// Extract host and port from an HTTP(S) URL using basic string parsing.
/// Returns `("host", port)` or `None` if unparseable.
pub(crate) fn parse_host_port(url: &str) -> Option<(String, u16)> {
    let after_scheme = url.split("://").nth(1)?;
    let authority = after_scheme.split('/').next()?;
    // Strip userinfo if present
    let host_port = authority.rsplit('@').next()?;

    // Handle IPv6 [::1]:port
    if let Some(bracket_end) = host_port.find(']') {
        let host = &host_port[..=bracket_end];
        let port = host_port[bracket_end + 1..]
            .strip_prefix(':')
            .and_then(|p| p.parse().ok())
            .unwrap_or(if url.starts_with("https") { 443 } else { 80 });
        Some((host.to_string(), port))
    } else if let Some((h, p)) = host_port.rsplit_once(':') {
        let port = p
            .parse()
            .unwrap_or(if url.starts_with("https") { 443 } else { 80 });
        Some((h.to_string(), port))
    } else {
        let port = if url.starts_with("https") { 443 } else { 80 };
        Some((host_port.to_string(), port))
    }
}

/// Check if an IP address belongs to a private, loopback, link-local, or
/// cloud metadata range that should not be targeted by outbound HTTP.
pub(crate) fn is_ssrf_dangerous_ip(ip: std::net::IpAddr) -> bool {
    use std::net::{Ipv4Addr, Ipv6Addr};

    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_loopback()                              // 127.0.0.0/8
            || v4.is_private()                             // 10/8, 172.16/12, 192.168/16
            || v4.is_link_local()                          // 169.254.0.0/16
            || v4 == Ipv4Addr::UNSPECIFIED                 // 0.0.0.0
            || v4.octets()[0] == 169                       // cloud metadata 169.254.169.254
                && v4.octets()[1] == 254
                && v4.octets()[2] == 169
                && v4.octets()[3] == 254
        }
        std::net::IpAddr::V6(v6) => {
            v6.is_loopback()                               // ::1
            || v6 == Ipv6Addr::UNSPECIFIED                 // ::
            || is_ipv4_mapped_dangerous(v6)
        }
    }
}

/// Check IPv4-mapped IPv6 addresses (::ffff:a.b.c.d) against the IPv4 SSRF list.
fn is_ipv4_mapped_dangerous(v6: std::net::Ipv6Addr) -> bool {
    let segments = v6.segments();
    // ::ffff:0:0/96 (IPv4-mapped)
    if segments[0..5] == [0, 0, 0, 0, 0] && segments[5] == 0xffff {
        let mapped = std::net::Ipv4Addr::new(
            (segments[6] >> 8) as u8,
            segments[6] as u8,
            (segments[7] >> 8) as u8,
            segments[7] as u8,
        );
        return is_ssrf_dangerous_ip(std::net::IpAddr::V4(mapped));
    }
    false
}
