//! Admin-triggered egress guard (H2/SSRF).
//!
//! The gateway legitimately dials user-configured upstreams on the data
//! plane, but the *admin-triggered* probes (`upstream-models` list and key
//! dial-test) turn a stolen/leaked gateway token into a server-side request
//! forgery primitive: write an attacker-controlled `base_url`/`chat_url`,
//! then make the gateway fetch it. This module is the single choke point
//! for those two probes (NOT for the inference data plane, where local
//! dev servers like Ollama on 127.0.0.1 are legitimate).
//!
//! Policy:
//! - scheme must be `http` or `https` (rejects `file:`, `gopher:`, …);
//! - host must not be a loopback / link-local / private / unspecified
//!   literal IP (`127/8`, `10/8`, `172.16/12`, `192.168/16`, `169.254/16`,
//!   `::1`, `fe80::/10`, `fc00::/7`, `0.0.0.0`, `::`);
//! - hostnames are resolved (blocking `getaddrinfo` offloaded via
//!   `tokio::task::spawn_blocking`) and EVERY resolved address must pass
//!   the same IP policy — this closes the DNS-rebinding hole where a name
//!   resolves to a public IP at validation time and to 169.254.x at use
//!   time (TOCTOU is narrowed to the resolve→use window, logged, and the
//!   connection still times out fast on the probe path);
//! - `.svc`, `.svc.cluster.local`, `localhost`, and cloud metadata hosts
//!   (`169.254.169.254`, `metadata.google.internal`, …) are rejected by
//!   name as well, so they stay blocked even if DNS is unavailable.

use std::net::{IpAddr, ToSocketAddrs};

/// Cloud metadata endpoints that must never be reachable from admin probes,
/// even if their IPs were to change.
const METADATA_HOSTS: &[&str] = &[
    "metadata.google.internal",
    "metadata.google.com",
    "instance-data",
    "169.254.169.254",
];

/// Shared IPv4 octet policy (used for native V4 and for IPv4-mapped V6).
fn blocked_v4(octets: [u8; 4], allow_loopback: bool) -> bool {
    if octets[0] == 127 {
        return !allow_loopback;
    }
    octets[0] == 10 // private 10/8
        || (octets[0] == 172 && (16..=31).contains(&octets[1])) // 172.16/12
        || (octets[0] == 192 && octets[1] == 168) // 192.168/16
        || (octets[0] == 169 && octets[1] == 254) // link-local 169.254/16 (metadata)
        || octets == [0, 0, 0, 0] // 0.0.0.0
}

fn is_blocked_ip(ip: &IpAddr) -> bool {
    // PONYLLM_ALLOW_LOOPBACK_PROBE=1 lifts the *loopback-only* ban for
    // integration tests and local dev (mock upstreams on 127.0.0.1).
    // Private/link-local/metadata ranges stay blocked regardless.
    let allow_loopback =
        std::env::var("PONYLLM_ALLOW_LOOPBACK_PROBE").as_deref() == Ok("1");
    match ip {
        IpAddr::V4(v4) => blocked_v4(v4.octets(), allow_loopback),
        IpAddr::V6(v6) => {
            // Pure V6 loopback/unspecified FIRST: `to_ipv4()` maps ::1 to
            // 0.0.0.1, so judging embedded-V4 before the V6 predicates would
            // let http://[::1]/ slip through as "0.0.0.1, not blocked"
            // (H2 red-team B1).
            if v6.is_loopback() {
                return !allow_loopback;
            }
            if v6.is_unspecified() {
                return true;
            }
            // IPv4-mapped AND deprecated IPv4-compatible V6 forms
            // (::ffff:127.0.0.1, ::127.0.0.1, ::10.0.0.1, …) MUST be judged
            // by the embedded IPv4 rules: the V6 predicates below are all
            // false for such addresses, so without this branch
            // loopback/private/metadata slip straight through (H2 blue-team
            // P0; to_ipv4 covers mapped + compatible, mapped alone is not
            // enough).
            if let Some(mapped) = v6.to_ipv4() {
                return blocked_v4(mapped.octets(), allow_loopback);
            }
            v6.is_unique_local() // fc00::/7
                || v6.is_unicast_link_local() // fe80::/10
        }
    }
}

/// Operator-managed allowlist for admin probes (e.g. an on-prem Ollama or
/// LAN model server): PONYLLM_PROBE_ALLOWLIST="ollama.lan,models.corp".
/// Entries match the exact host or any subdomain.
fn probe_allowlisted(host: &str) -> bool {
    if let Ok(list) = std::env::var("PONYLLM_PROBE_ALLOWLIST") {
        let lower = host.trim().trim_end_matches('.').to_ascii_lowercase();
        for entry in list.split(',') {
            let e = entry.trim().trim_end_matches('.').to_ascii_lowercase();
            if !e.is_empty() && (lower == e || lower.ends_with(&format!(".{}", e))) {
                return true;
            }
        }
    }
    false
}

fn is_blocked_name(host: &str) -> Option<&'static str> {
    // Operator-managed allowlist for admin probes (e.g. an on-prem Ollama or
    // LAN model server): PONYLLM_PROBE_ALLOWLIST="ollama.lan,models.corp".
    // Entries match the exact host or any subdomain. Loopback literals are
    // still governed by PONYLLM_ALLOW_LOOPBACK_PROBE, not by this list.
    if let Ok(list) = std::env::var("PONYLLM_PROBE_ALLOWLIST") {
        let lower = host.trim().trim_end_matches('.').to_ascii_lowercase();
        for entry in list.split(',') {
            let e = entry.trim().trim_end_matches('.').to_ascii_lowercase();
            if !e.is_empty() && (lower == e || lower.ends_with(&format!(".{}", e))) {
                return None;
            }
        }
    }
    let lower = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if lower == "localhost" {
        // Same test/dev escape hatch as loopback IPs (see is_blocked_ip).
        let allow_loopback =
            std::env::var("PONYLLM_ALLOW_LOOPBACK_PROBE").as_deref() == Ok("1");
        if allow_loopback {
            return None;
        }
        return Some("loopback hostname 'localhost' is not allowed for admin probes");
    }
    if lower == "svc" || lower.ends_with(".svc") || lower.ends_with(".svc.cluster.local") {
        return Some("kubernetes in-cluster names (*.svc) are not allowed for admin probes");
    }
    if METADATA_HOSTS.iter().any(|m| lower == *m || lower.ends_with(&format!(".{}", m))) {
        return Some("cloud metadata endpoints are not allowed for admin probes");
    }
    None
}

/// Parse `raw` as an absolute http(s) URL and return its host part.
///
/// NOTE: `Url::host_str()` keeps IPv6 brackets (`[::1]`), which would break
/// `parse::<IpAddr>` below and silently downgrade literal IPs to the
/// hostname path. We strip exactly one surrounding bracket pair instead of
/// switching to `Url::host()` so DNS names keep their verbatim form.
fn parse_host(raw: &str) -> Result<String, String> {
    let url = reqwest::Url::parse(raw.trim())
        .map_err(|e| format!("invalid probe URL '{}': {}", raw.trim(), e))?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(format!(
            "probe URL scheme must be http or https, got '{}'",
            url.scheme()
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| format!("probe URL '{}' has no host", raw.trim()))?;
    let stripped = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    Ok(stripped.to_string())
}

/// Proxy-URL policy for admin writes (H2 red-team).
///
/// A proxy only *steers* outbound traffic (it is never fetched as content),
/// so loopback proxies (local pproxy `127.0.0.1:8899`, `localhost:7890`)
/// are legitimate and allowed. Everything else follows the probe policy:
/// http/https(+socks for proxy use) scheme, no private/link-local/metadata
/// targets. An attacker writing `proxy=http://169.254.169.254:80` or an
/// external sniffer proxy is refused here instead of silently adopted.
pub fn check_proxy_url_fast(raw: &str) -> Result<(), String> {
    let trimmed = raw.trim();
    let scheme_end = trimmed.find("://").ok_or_else(|| {
        format!(
            "invalid proxy URL '{}': must be scheme://host:port",
            trimmed
        )
    })?;
    let scheme = trimmed[..scheme_end].to_ascii_lowercase();
    if scheme != "http" && scheme != "https" && scheme != "socks5" && scheme != "socks5h" {
        return Err(format!(
            "proxy URL scheme must be http/https/socks5, got '{}'",
            scheme
        ));
    }
    // Reuse the host/IP policy with loopback permitted (local egress proxy
    // is the documented deployment shape).
    let after_scheme = &trimmed[scheme_end + 3..];
    let host_port = after_scheme.split('/').next().unwrap_or("");
    // Strip optional userinfo.
    let host_port = host_port.rsplit('@').next().unwrap_or("");
    // Strip optional port (careful with IPv6 literals).
    let host = if let Some(stripped) = host_port
        .strip_prefix('[')
        .and_then(|h| h.split(']').next())
    {
        stripped
    } else {
        host_port.split(':').next().unwrap_or("")
    };
    if host.is_empty() {
        return Err(format!("proxy URL '{}' has no host", trimmed));
    }
    if let Some(reason) = is_blocked_name(host) {
        // localhost is allowed for proxies (local dev proxy); the name
        // check would otherwise reject it.
        let lower = host.trim().trim_end_matches('.').to_ascii_lowercase();
        if lower != "localhost" {
            return Err(reason.to_string());
        }
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        match &ip {
            // Loopback proxy explicitly allowed (documented shape).
            IpAddr::V4(v4) if v4.octets()[0] == 127 => return Ok(()),
            IpAddr::V6(v6) if v6.is_loopback() => return Ok(()),
            _ => {}
        }
        if is_blocked_ip(&ip) {
            return Err(format!(
                "proxy target '{}' resolves to a blocked address ({})",
                host, ip
            ));
        }
        return Ok(());
    }
    Ok(())
}

/// Synchronous (no-DNS-for-literals) policy check. Used by the create/update
/// provider write path so deployments fail fast on obvious mistakes without
/// paying a DNS lookup inside the write lock.
pub fn check_probe_url_fast(raw: &str) -> Result<(), String> {
    let host = parse_host(raw)?;
    if let Some(reason) = is_blocked_name(&host) {
        return Err(reason.to_string());
    }
    // Literal IPs are decided without DNS.
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_blocked_ip(&ip) {
            return Err(format!(
                "probe target '{}' resolves to a blocked address ({})",
                host, ip
            ));
        }
        return Ok(());
    }
    // Hostnames pass the fast check; the async check below resolves them.
    // (DNS-rebinding between check and use is narrowed by re-resolving at
    // probe time — see check_probe_url.)
    Ok(())
}

/// Probe-only HTTP client: short timeouts, NO redirect following, NO proxy
/// inheritance. Redirects are refused because an attacker-controlled public
/// URL could otherwise 302 to a metadata/private target AFTER the
/// check_probe_url gate passed (H2 red-team). Callers surface redirect
/// attempts as errors instead of following them.
pub fn probe_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .connect_timeout(std::time::Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .unwrap_or_default()
}

/// Full async policy check: fast policy + resolve every address and require
/// ALL of them to pass. Must be called on the admin probe path immediately
/// before dialing (not only at provider write time).
pub async fn check_probe_url(raw: &str) -> Result<(), String> {
    check_probe_url_fast(raw)?;
    let host = parse_host(raw)?;
    // Operator allowlist (see is_blocked_name) also skips the IP re-check:
    // an allowlisted LAN name necessarily resolves to a LAN address.
    if host.parse::<IpAddr>().is_err() && probe_allowlisted(&host) {
        return Ok(());
    }
    // Literal IPs were already decided above.
    if host.parse::<IpAddr>().is_ok() {
        return Ok(());
    }
    let host_for_lookup = host.clone();
    // Bound the blocking getaddrinfo: an attacker-controlled hostname with a
    // hung authoritative server must not pin a spawn_blocking thread
    // indefinitely. Timeout is fail-closed (probe refused).
    let lookup = tokio::task::spawn_blocking(move || {
        (host_for_lookup.as_str(), 0)
            .to_socket_addrs()
            .map(|it| it.map(|s| s.ip()).collect::<Vec<IpAddr>>())
            .map_err(|e| format!("DNS resolution failed for '{}': {}", host_for_lookup, e))
    });
    let addrs: Vec<IpAddr> = match tokio::time::timeout(std::time::Duration::from_secs(5), lookup).await
    {
        Ok(Ok(Ok(addrs))) => addrs,
        Ok(Ok(Err(e))) => return Err(e),
        Ok(Err(e)) => return Err(format!("DNS lookup task failed: {}", e)),
        Err(_) => {
            return Err(format!(
                "DNS resolution timed out for '{}' (blocked fail-closed)",
                host
            ))
        }
    };
    if addrs.is_empty() {
        return Err(format!("DNS resolution returned no addresses for '{}'", host));
    }
    for ip in &addrs {
        if is_blocked_ip(ip) {
            return Err(format!(
                "probe target '{}' resolves to a blocked address ({})",
                host, ip
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_rejects_literal_private_and_metadata() {
        // NOTE: never assert on 127.0.0.1/localhost here — the
        // PONYLLM_ALLOW_LOOPBACK_PROBE escape hatch is process-global and
        // integration-test binaries enable it. Loopback blocking is covered
        // by the async tests below (separate binary, hatch unset) and by
        // always-blocked ranges asserted here.
        for bad in [
            "http://10.0.0.5/",
            "http://172.16.9.9:8080/",
            "http://192.168.1.10/v1",
            "http://169.254.169.254/latest/meta-data/",
            "http://api.svc:8080/",
            "http://x.svc.cluster.local/",
            "http://0.0.0.0:8080/",
            "file:///etc/passwd",
            "gopher://example.com/",
        ] {
            assert!(check_probe_url_fast(bad).is_err(), "should block {}", bad);
        }
    }

    #[test]
    fn fast_allows_public_https() {
        for good in [
            "https://api.openai.com/v1",
            "https://api.deepseek.com/",
            "http://example.com/models",
        ] {
            assert!(check_probe_url_fast(good).is_ok(), "should allow {}", good);
        }
    }

    #[test]
    fn fast_rejects_ipv4_mapped_ipv6_bypass() {
        // H2 blue-team P0: ::ffff:<v4> must be judged by the embedded IPv4
        // rules, never by the V6 branch (whose predicates are all false for
        // mapped addresses).
        for bad in [
            "http://[::ffff:127.0.0.1]/",
            "http://[::ffff:10.0.0.1]/v1",
            "http://[::ffff:192.168.1.1]/",
            "http://[::ffff:169.254.169.254]/latest/meta-data/",
            // H2 red-team B1: pure-V6 loopback/unspecified must be judged
            // BEFORE the to_ipv4() fallback (which maps ::1 to 0.0.0.1).
            "http://[::1]/",
            "http://[::1]:8080/v1",
            "http://[::]/",
        ] {
            assert!(check_probe_url_fast(bad).is_err(), "should block {}", bad);
        }
    }

    #[test]
    fn proxy_policy_allows_loopback_but_blocks_metadata() {
        // H2 red-team: proxy steers Bearer-bearing traffic, so hostile
        // proxies are refused, but the documented local pproxy shape stays.
        for good in [
            "http://127.0.0.1:8899",
            "http://localhost:7890",
            "http://proxy.lan:8080",
            "socks5://127.0.0.1:1080",
        ] {
            assert!(check_proxy_url_fast(good).is_ok(), "should allow {}", good);
        }
        for bad in [
            "http://169.254.169.254:80",
            "http://10.0.0.5:8080",
            "http://192.168.1.1:3128",
            "http://[::ffff:169.254.169.254]:80",
            "gopher://127.0.0.1:70",
            "no-scheme-here",
        ] {
            assert!(check_proxy_url_fast(bad).is_err(), "should block {}", bad);
        }
    }

    #[tokio::test]
    async fn async_blocks_localhost_resolution() {
        // localhost typically resolves to 127.0.0.1/::1; even if the test
        // sandbox has no DNS, the name block fires first.
        assert!(check_probe_url("http://localhost:11434/v1").await.is_err());
    }

    #[tokio::test]
    async fn async_blocks_literal_loopback() {
        assert!(check_probe_url("http://127.0.0.1:9/").await.is_err());
    }

    #[tokio::test]
    async fn probe_client_refuses_redirect_to_metadata() {
        // H2 red-team B2: a public URL that passes the gate must not 302 to
        // a metadata target afterwards. The probe client never follows.
        use axum::routing::get;
        let app = axum::Router::new().route(
            "/redir",
            get(|| async {
                (
                    axum::http::StatusCode::FOUND,
                    [(axum::http::header::LOCATION, "http://169.254.169.254/latest/meta-data/")],
                    "redirect",
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let url = format!("http://{}/redir", addr);
        let resp = probe_http_client().get(&url).send().await.unwrap();
        // Policy::none => the 302 is surfaced, never followed.
        assert_eq!(resp.status(), axum::http::StatusCode::FOUND);
    }
}
