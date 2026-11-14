//! Outbound callback URL validation to prevent Server-Side Request Forgery (SSRF).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};

/// Validates an outbound `callback_url` before opening any socket.
///
/// Requires `https://` in production. Allows `http://` and loopback addresses only when
/// `allow_insecure_local` is explicitly enabled for local development or testing.
/// Rejects RFC1918 private, link-local (`169.254.0.0/16`, `fe80::/10`), unspecified,
/// multicast, and CGNAT addresses.
///
/// # Errors
///
/// Returns `Err("invalid_callback_url")` if the URL is malformed, uses a disallowed
/// scheme, or targets a restricted host or IP range.
pub fn validate_callback_url(url: &str, allow_insecure_local: bool) -> Result<(), &'static str> {
    let trimmed = url.trim();
    let (rest, default_port) = if let Some(r) = trimmed.strip_prefix("https://") {
        (r, 443u16)
    } else if allow_insecure_local && let Some(r) = trimmed.strip_prefix("http://") {
        (r, 80u16)
    } else {
        return Err("invalid_callback_url");
    };

    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return Err("invalid_callback_url");
    }

    let (host, port) = parse_authority(authority, default_port)?;
    let host_lower = host.to_ascii_lowercase();
    if is_blocked_hostname(&host_lower, allow_insecure_local) {
        return Err("invalid_callback_url");
    }

    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_blocked_ip(ip, allow_insecure_local) {
            return Err("invalid_callback_url");
        }
        return Ok(());
    }

    // Best-effort DNS resolution check: if the hostname resolves locally, block private/loopback targets.
    if let Ok(addrs) = (host, port).to_socket_addrs() {
        for addr in addrs {
            if is_blocked_ip(addr.ip(), allow_insecure_local) {
                return Err("invalid_callback_url");
            }
        }
    }

    Ok(())
}

/// Parses `host[:port]` or `[ipv6][:port]` into `(host, port)`.
fn parse_authority(authority: &str, default_port: u16) -> Result<(&str, u16), &'static str> {
    if let Some(after_open) = authority.strip_prefix('[') {
        let close_idx = after_open.find(']').ok_or("invalid_callback_url")?;
        let host = &after_open[..close_idx];
        if host.is_empty() {
            return Err("invalid_callback_url");
        }
        let remainder = &after_open[close_idx + 1..];
        let port = if remainder.is_empty() {
            default_port
        } else if let Some(port_str) = remainder.strip_prefix(':') {
            parse_nonzero_port(port_str)?
        } else {
            return Err("invalid_callback_url");
        };
        return Ok((host, port));
    }

    match authority.rsplit_once(':') {
        Some((host, port_str)) => {
            if host.is_empty() || host.contains(':') {
                return Err("invalid_callback_url");
            }
            Ok((host, parse_nonzero_port(port_str)?))
        }
        None => Ok((authority, default_port)),
    }
}

/// Parses a non-zero TCP port number.
fn parse_nonzero_port(raw: &str) -> Result<u16, &'static str> {
    let port = raw.parse::<u16>().map_err(|_| "invalid_callback_url")?;
    if port == 0 {
        return Err("invalid_callback_url");
    }
    Ok(port)
}

/// Checks whether a hostname is a restricted local or internal name.
fn is_blocked_hostname(host_lower: &str, allow_insecure_local: bool) -> bool {
    if (host_lower == "localhost" || host_lower.ends_with(".localhost")) && !allow_insecure_local {
        return true;
    }
    host_lower.ends_with(".local") || host_lower.ends_with(".internal")
}

/// Checks whether an IP address belongs to a restricted network range.
fn is_blocked_ip(ip: IpAddr, allow_insecure_local: bool) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_ipv4(v4, allow_insecure_local),
        IpAddr::V6(v6) => is_blocked_ipv6(v6, allow_insecure_local),
    }
}

/// Checks whether an IPv4 address is loopback, private, link-local, unspecified, multicast, or CGNAT.
fn is_blocked_ipv4(ip: Ipv4Addr, allow_insecure_local: bool) -> bool {
    if ip.is_loopback() {
        return !allow_insecure_local;
    }
    let octets = ip.octets();
    let is_cgnat = octets[0] == 100 && (octets[1] & 0b1100_0000) == 64;
    ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_multicast()
        || ip.is_broadcast()
        || is_cgnat
}

/// Checks whether an IPv6 address is loopback, unspecified, multicast, link-local, unique-local, or IPv4-mapped restricted.
fn is_blocked_ipv6(ip: Ipv6Addr, allow_insecure_local: bool) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_blocked_ipv4(v4, allow_insecure_local);
    }
    if ip.is_loopback() {
        return !allow_insecure_local;
    }
    let first_seg = ip.segments()[0];
    let is_unique_local = (first_seg & 0xfe00) == 0xfc00;
    let is_link_local = (first_seg & 0xffc0) == 0xfe80;
    ip.is_unspecified() || ip.is_multicast() || is_unique_local || is_link_local
}
