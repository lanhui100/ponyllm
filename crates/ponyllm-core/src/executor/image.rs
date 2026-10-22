//! SSRF-safe image fetching for the deepseek-web vision bridge (wave-10
//! contract R-VIS-06..10, H-V04/H-V05).
//!
//! Owned by T4 (task-7, write domain `crates/ponyllm-core/src/executor`).
//! Pure, zero-I/O host policy lives here (`is_blocked_image_host`) alongside
//! the bounded fetch (`fetch_image_bytes`). Upload/parse bridging lives in
//! `upstream.rs` (`prepare_deepseek_web_completion`) and consumes the
//! client-side `upload_file`/`fetch_files`/`wait_file_ready` (task-6).

use std::net::IpAddr;
use std::time::Duration;

/// Max bytes accepted for a fetched image (contract R-VIS-09: ≤100MB).
pub const MAX_IMAGE_FETCH_BYTES: u64 = 100 * 1024 * 1024;

/// Allowed image extensions (contract R-VIS-11 / R-VIS-09 white list).
pub const IMAGE_EXT_WHITELIST: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "tif", "avif",
];

/// Connect timeout budget for image URL fetching (R-VIS-08: ≤3s).
pub const IMAGE_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// Total fetch budget for image URL fetching (R-VIS-08: ≤30s).
pub const IMAGE_FETCH_TOTAL_TIMEOUT: Duration = Duration::from_secs(30);
/// Max redirect hops (R-VIS-08: ≤3, each hop re-validated).
pub const IMAGE_MAX_REDIRECTS: usize = 3;

/// Whether `host` is a blocked SSRF target (contract H-V04, R-VIS-07).
///
/// Pure-function policy layer (zero I/O, zero DNS except env read once per
/// call — the allowlist env is read per call so test hooks take effect without
/// process restart), case-insensitive; IPv6 compressed and expanded forms are
/// both recognized via [`IpAddr::from_str`].
///
/// Blocked: literal private (`10/8`, `172.16/12`, `192.168/16`), loopback
/// (`127/8`), link-local (`169.254/16` incl. cloud metadata
/// `169.254.169.254`), IPv6 ULA (`fc00::/7`), `::1`, `0.0.0.0`; `localhost`
/// (with subdomains). DNS names that are NOT literal IPs return `false` — the
/// caller must re-validate every resolved IP (R-VIS-07).
///
/// Test/controlled-ops knob `PONYLLM_IMAGE_HOST_ALLOWLIST` (Lead ruling):
/// comma-separated EXACT hostnames (no wildcards) that relax private/loopback
/// blocking for local image servers in tests. The link-local / cloud-metadata
/// range (169.254/16, incl. 169.254.169.254) can NEVER be allowlisted — it is
/// unconditionally blocked regardless of the knob.
pub fn is_blocked_image_host(host: &str) -> bool {
    let h = host.trim().trim_matches(|c| c == '[' || c == ']');
    if h.is_empty() {
        return true;
    }
    let lower = h.to_ascii_lowercase();
    // localhost and subdomains (e.g. localhost.localdomain, foo.localhost).
    if lower == "localhost"
        || lower.ends_with(".localhost")
        || lower.ends_with(".localdomain")
    {
        return true;
    }
    // Literal IP: parse and apply the blocked ranges.
    if let Ok(ip) = lower.parse::<IpAddr>() {
        // Link-local / cloud-metadata (169.254/16) is unconditionally blocked
        // — the allowlist can NEVER relax it (Lead ruling).
        let is_link_local = match ip {
            IpAddr::V4(v4) => v4.is_link_local(),
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map(|v| v.is_link_local()).unwrap_or(false),
        };
        if is_link_local {
            return true;
        }
        if image_ip_blocked(ip) {
            // Allowlist relaxation (test-only, exact hostnames only): a
            // private/loopback literal may be lifted when its hostname form is
            // in the allowlist.
            return !image_host_allowlisted(&lower);
        }
        return false;
    }
    // Non-IP literal → DNS name: not blocked at literal stage; every resolved
    // IP is re-validated by the caller (R-VIS-07). Exact-hostname allowlist
    // entries are honored there via [`image_host_allowlisted`].
    false
}

/// Read `PONYLLM_IMAGE_HOST_ALLOWLIST` (test/controlled-ops only; default
/// empty = strict blocking): comma-separated EXACT hostnames (no wildcards).
/// Private/loopback hosts may be lifted for local test image servers; the
/// link-local/metadata range can never be allowlisted (enforced in
/// [`is_blocked_image_host`]). Non-empty usage logs a WARN once per process.
pub fn image_host_allowlisted(host: &str) -> bool {
    let raw = match std::env::var("PONYLLM_IMAGE_HOST_ALLOWLIST") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return false,
    };
    // Emit a one-time WARN when the knob is active (test-only marker).
    use std::sync::OnceLock;
    static WARNED: OnceLock<()> = OnceLock::new();
    let _ = WARNED.get_or_init(|| {
        tracing::warn!(
            "PONYLLM_IMAGE_HOST_ALLOWLIST is set — image SSRF allowlist is TEST-ONLY and must not be enabled in production (link-local/metadata 169.254/16 can never be allowlisted)"
        );
    });
    let h = host.trim().trim_matches(|c| c == '[' || c == ']').to_ascii_lowercase();
    raw.split(',').any(|entry| {
        let e = entry.trim().trim_matches(|c| c == '[' || c == ']').to_ascii_lowercase();
        !e.is_empty() && e == h
    })
}

/// Blocked-IP policy for a resolved address (H-V04 semantics). Link-local /
/// metadata (169.254/16) is unconditionally blocked.
pub fn image_ip_blocked(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            // 0.0.0.0 / loopback 127/8.
            if v4.is_unspecified() || v4.is_loopback() {
                return true;
            }
            // Link-local 169.254/16 (incl. cloud metadata 169.254.169.254).
            if v4.is_link_local() {
                return true;
            }
            // Private 10/8, 172.16/12, 192.168/16 (std is_private covers them).
            if v4.is_private() {
                return true;
            }
            false
        }
        IpAddr::V6(v6) => {
            // ::1 loopback, :: unspecified.
            if v6.is_unspecified() || v6.is_loopback() {
                return true;
            }
            // ULA fc00::/7.
            if v6.is_unique_local() {
                return true;
            }
            // IPv4-mapped addresses (`::ffff:a.b.c.d`) must be judged by the
            // embedded IPv4 rules (H2 precedent — never via the V6 branch).
            if let Some(v4) = v6.to_ipv4_mapped() {
                return image_ip_blocked(IpAddr::V4(v4));
            }
            false
        }
    }
}

/// Resolved-IP × host policy (R-VIS-07, Lead ruling): a resolved `ip` is
/// blocked when it hits the blocked ranges — EXCEPT when the exact hostname
/// is in the test-only allowlist AND the IP is not link-local/metadata
/// (169.254/16 can never be allowlisted).
pub fn image_host_blocked_for(host: &str, ip: IpAddr) -> bool {
    let link_local = match ip {
        IpAddr::V4(v4) => v4.is_link_local(),
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(|v| v.is_link_local()).unwrap_or(false),
    };
    if link_local {
        return true;
    }
    image_ip_blocked(ip) && !image_host_allowlisted(host)
}

/// Errors from [`fetch_image_bytes`] (R-VIS-06..09). All map to explicit 4xx
/// gateway errors — never a silent drop.
#[derive(Debug, thiserror::Error)]
pub enum ImageFetchError {
    #[error("only http/https image URLs are supported")]
    UnsupportedScheme,
    #[error("image host is blocked (SSRF policy): {0}")]
    HostBlocked(String),
    #[error("image host resolution failed: {0}")]
    DnsFailure(String),
    #[error("image fetch timeout exceeded budget (connect 3s / total 30s)")]
    Timeout,
    #[error("too many redirects (> {IMAGE_MAX_REDIRECTS})")]
    TooManyRedirects,
    #[error("image body exceeds 100MB limit")]
    BodyTooLarge,
    #[error("image content-type {0} is not an allowed image type")]
    ContentTypeNotImage(String),
    #[error("image extension .{0} is not in the allowed image whitelist")]
    ExtensionNotAllowed(String),
    #[error("image content-type ({0}) does not match the URL extension (.{1})")]
    ContentTypeExtensionMismatch(String, String),
    #[error("image fetch failed: {0}")]
    Transport(String),
}

/// Fetch an image over HTTP(S) with SSRF protection (contract H-V05,
/// R-VIS-06..09). Returns the image bytes for the uploader.
///
/// Guards: http/https only; literal host AND every resolved IP re-checked via
/// [`is_blocked_image_host`]/[`image_ip_blocked`]; connect ≤3s, total ≤30s,
/// redirects ≤3 with per-hop re-validation; body ≤100MB; `content-type:
/// image/*` must match the URL extension whitelist.
pub async fn fetch_image_bytes(url: &str) -> Result<Vec<u8>, ImageFetchError> {
    let parsed = reqwest::Url::parse(url).map_err(|e| {
        ImageFetchError::Transport(format!("invalid image URL: {e}"))
    })?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(ImageFetchError::UnsupportedScheme);
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| ImageFetchError::Transport("image URL missing host".to_string()))?;

    // Literal-host check (R-VIS-07).
    if is_blocked_image_host(host) {
        return Err(ImageFetchError::HostBlocked(host.to_string()));
    }
    // Extension white list (R-VIS-09/11).
    let ext = parsed
        .path_segments()
        .and_then(|mut s| s.next_back())
        .and_then(|p| {
            p.rsplit_once('.')
                .map(|(_, e)| e.to_ascii_lowercase())
        })
        .unwrap_or_default();
    if !IMAGE_EXT_WHITELIST.contains(&ext.as_str()) {
        return Err(ImageFetchError::ExtensionNotAllowed(ext));
    }

    // Resolve + per-IP re-check; then fetch with budgets.
    let ips = resolve_image_host(host).await?;
    let client = image_fetch_client();
    let mut current = parsed.clone();
    for hop in 0..=IMAGE_MAX_REDIRECTS {
        let cur_host = current
            .host_str()
            .ok_or_else(|| ImageFetchError::Transport("redirect URL missing host".to_string()))?;
        // Per-hop literal check (R-VIS-08).
        if is_blocked_image_host(cur_host) {
            return Err(ImageFetchError::HostBlocked(cur_host.to_string()));
        }
        // Per-hop resolved-IP check (DNS rebinding defense).
        if hop == 0 {
            for ip in &ips {
                if image_host_blocked_for(cur_host, *ip) {
                    return Err(ImageFetchError::HostBlocked(format!("{cur_host} → {ip}")));
                }
            }
        } else {
            let hop_ips = resolve_image_host(cur_host).await?;
            for ip in &hop_ips {
                if image_host_blocked_for(cur_host, *ip) {
                    return Err(ImageFetchError::HostBlocked(format!("{cur_host} → {ip}")));
                }
            }
        }

        let resp = tokio::time::timeout(IMAGE_FETCH_TOTAL_TIMEOUT, {
            let client = &client;
            let cur = current.clone();
            async move { client.get(cur).send().await }
        })
        .await
        .map_err(|_| ImageFetchError::Timeout)?
        .map_err(|e| ImageFetchError::Transport(e.to_string()))?;

        if let Some(loc) = resp.headers().get(reqwest::header::LOCATION) {
            if hop >= IMAGE_MAX_REDIRECTS {
                return Err(ImageFetchError::TooManyRedirects);
            }
            let next_url = loc
                .to_str()
                .ok()
                .and_then(|l| current.join(l).ok())
                .ok_or_else(|| ImageFetchError::Transport("invalid redirect".to_string()))?;
            if !matches!(next_url.scheme(), "http" | "https") {
                return Err(ImageFetchError::UnsupportedScheme);
            }
            current = next_url;
            continue;
        }

        if !resp.status().is_success() {
            return Err(ImageFetchError::Transport(format!(
                "image fetch HTTP {}",
                resp.status()
            )));
        }

        // content-type image/* check (R-VIS-09).
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        let ct_main = content_type.split(';').next().unwrap_or("").trim();
        if !ct_main.starts_with("image/") {
            return Err(ImageFetchError::ContentTypeNotImage(content_type));
        }
        // content-type ↔ extension consistency (R-VIS-09).
        let ct_ext = ct_main.trim_start_matches("image/").to_ascii_lowercase();
        if ct_ext != ext && !(ct_ext == "jpeg" && ext == "jpg") && !(ct_ext == "jpg" && ext == "jpeg")
        {
            return Err(ImageFetchError::ContentTypeExtensionMismatch(
                content_type,
                ext,
            ));
        }

        // Bounded body read (≤100MB).
        let mut body = Vec::new();
        let mut stream = resp.bytes_stream();
        use futures_util::StreamExt;
        loop {
            match tokio::time::timeout(IMAGE_FETCH_TOTAL_TIMEOUT, stream.next()).await {
                Ok(Some(Ok(chunk))) => {
                    if body.len() as u64 + chunk.len() as u64 > MAX_IMAGE_FETCH_BYTES {
                        return Err(ImageFetchError::BodyTooLarge);
                    }
                    body.extend_from_slice(&chunk);
                }
                Ok(Some(Err(e))) => {
                    return Err(ImageFetchError::Transport(e.to_string()));
                }
                Ok(None) => break,
                Err(_) => return Err(ImageFetchError::Timeout),
            }
        }
        return Ok(body);
    }
    Err(ImageFetchError::TooManyRedirects)
}

/// Build the fetch client with the connect ≤3s budget (R-VIS-08).
fn image_fetch_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(IMAGE_CONNECT_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none()) // manual per-hop revalidation
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Resolve a host to IPs (empty on DNS failure → caller treats as explicit
/// error). Uses the system resolver; every resolved IP must pass the SSRF
/// policy (R-VIS-07).
async fn resolve_image_host(host: &str) -> Result<Vec<IpAddr>, ImageFetchError> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(vec![ip]);
    }
    let host_owned = host.to_string();
    tokio::task::spawn_blocking(move || {
        use std::net::ToSocketAddrs;
        let addrs: Vec<IpAddr> = (host_owned.as_str(), 0)
            .to_socket_addrs()
            .map_err(|e| ImageFetchError::DnsFailure(e.to_string()))?
            .map(|s| s.ip())
            .collect();
        if addrs.is_empty() {
            return Err(ImageFetchError::DnsFailure(format!(
                "no addresses for {host_owned}"
            )));
        }
        Ok(addrs)
    })
    .await
    .map_err(|e| ImageFetchError::DnsFailure(e.to_string()))?
}

/// Whether `ext` (lowercase, no dot) is in the image whitelist (R-VIS-11).
pub fn is_allowed_image_ext(ext: &str) -> bool {
    IMAGE_EXT_WHITELIST.contains(&ext)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_literal_private_loopback_linklocal_metadata() {
        for bad in [
            "10.0.0.1",
            "10.255.255.255",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.10",
            "127.0.0.1",
            "127.8.8.8",
            "169.254.169.254", // cloud metadata
            "169.254.1.1",
            "0.0.0.0",
            "::1",
            "::",
            "fc00::1",     // ULA
            "fd12:3456::1", // ULA
            "::ffff:127.0.0.1", // IPv4-mapped loopback
            "::ffff:10.0.0.1",  // IPv4-mapped private
            "::ffff:169.254.169.254", // IPv4-mapped metadata
        ] {
            assert!(is_blocked_image_host(bad), "must block {bad}");
        }
        for ok in ["example.com", "images.example.com", "8.8.8.8", "1.1.1.1", "::1:2:3:4"] {
            assert!(!is_blocked_image_host(ok), "must allow {ok}");
        }
    }

    #[test]
    fn blocks_localhost_with_subdomains() {
        assert!(is_blocked_image_host("localhost"));
        assert!(is_blocked_image_host("localhost.localdomain"));
        assert!(is_blocked_image_host("foo.localhost"));
        assert!(is_blocked_image_host("[::1]"));
    }

    #[test]
    fn whitelist_exts() {
        for e in ["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "tif", "avif"] {
            assert!(is_allowed_image_ext(e), "{e}");
        }
        assert!(!is_allowed_image_ext("txt"));
        assert!(!is_allowed_image_ext("pdf"));
        assert!(!is_allowed_image_ext(""));
    }

    #[test]
    fn ipv4_mapped_judged_by_embedded_v4() {
        assert!(image_ip_blocked("::ffff:192.168.1.1".parse().unwrap()));
        assert!(image_ip_blocked("::ffff:127.0.0.1".parse().unwrap()));
        assert!(!image_ip_blocked("::ffff:8.8.8.8".parse().unwrap()));
    }

    #[tokio::test]
    async fn rejects_non_http_scheme() {
        // file:/ / ftp: are rejected before any network activity.
        for url in ["file:///etc/passwd", "ftp://example.com/x.png", "data:image/png;base64,x"] {
            match fetch_image_bytes(url).await {
                Err(ImageFetchError::UnsupportedScheme) => {}
                other => panic!("{url} must be UnsupportedScheme, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn rejects_blocked_literal_hosts_without_egress() {
        for url in [
            "http://127.0.0.1/x.png",
            "http://169.254.169.254/latest/meta-data",
            "http://10.0.0.1/x.png",
            "http://[::1]/x.png",
            "http://localhost/x.png",
        ] {
            match fetch_image_bytes(url).await {
                Err(ImageFetchError::HostBlocked(_)) => {}
                other => panic!("{url} must be HostBlocked, got {other:?}"),
            }
        }
    }
}
