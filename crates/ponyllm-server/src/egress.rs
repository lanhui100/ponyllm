//! Egress guard module (H2/SSRF + VULN-07/F6 data plane).
//!
//! Two policy families live here:
//! - **Admin-triggered probes** (`upstream-models` list and key dial-test):
//!   a stolen/leaked gateway token must not become a server-side request
//!   forgery primitive, so `check_probe_url(_fast)` refuses internal
//!   targets before any probe dial.
//! - **Data-plane upstreams** (inference): `check_data_plane_url` re-validates
//!   a routed upstream URL at dial time (DNS-rebinding defense) for DIRECT
//!   dials, while `check_data_plane_url_proxied` skips local DNS for targets
//!   dialed through an explicit forward proxy (the trusted proxy owns
//!   resolution + egress; see ADR 2026-10-06-egress-guard-proxied-dns-skip).
//!   Local dev servers like Ollama on 127.0.0.1 stay legitimate on the data
//!   plane.
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
//!   name as well, so they stay blocked even if DNS is unavailable; on the
//!   data plane an `*.svc` name is only unlocked by an allowlist entry that
//!   is itself a `.svc`-suffixed name, and cloud metadata is never unlockable;
//! - the operator allowlist (`PONYLLM_PROBE_ALLOWLIST`) exempts exact hosts
//!   AND literal IPs (LAN model servers / on-prem proxies) BEFORE the
//!   name/IP policy runs (B7).

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
        || (octets[0] == 100 && (64..=127).contains(&octets[1])) // 100.64/10 CGNAT (RFC 6598)
        || (octets[0] == 198 && (18..=19).contains(&octets[1])) // 198.18/15 benchmarking (RFC 2544)
        || octets == [0, 0, 0, 0] // 0.0.0.0
}

/// Shared per-IP policy with an explicit loopback flag. Used by
/// [`is_blocked_ip`] (admin probes, loopback lifted only via
/// `PONYLLM_ALLOW_LOOPBACK_PROBE`) and by [`check_data_plane_url`] (data
/// plane, where loopback is a documented legitimate shape — local Ollama).
fn is_blocked_ip_with(ip: &IpAddr, allow_loopback: bool) -> bool {
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

fn is_blocked_ip(ip: &IpAddr) -> bool {
    // PONYLLM_ALLOW_LOOPBACK_PROBE=1 lifts the *loopback-only* ban for
    // integration tests and local dev (mock upstreams on 127.0.0.1).
    // Private/link-local/metadata ranges stay blocked regardless.
    let allow_loopback = std::env::var("PONYLLM_ALLOW_LOOPBACK_PROBE").as_deref() == Ok("1");
    is_blocked_ip_with(ip, allow_loopback)
}

/// Operator-managed allowlist: `PONYLLM_PROBE_ALLOWLIST="ollama.lan,models.corp"`.
/// Entries match the exact host or any subdomain; literal IPs (e.g.
/// `10.0.0.5`) also match exactly and are exempted from the name/IP policy
/// (B7) — the intended escape hatch for LAN model servers / on-prem
/// proxies.
/// Also includes well-known cloud AI providers / endpoints (e.g. Sensetime/SenseNova, DeepSeek)
/// to prevent local recursive DNS timeouts from breaking upstream inference.
const BUILTIN_MODEL_ALLOWLIST: &[&str] = &[
    "sensenova.cn",
    "deepseek.com",
    "openai.com",
    "anthropic.com",
    "moonshot.cn",
    "baichuan-ai.com",
    "zhipuai.cn",
    "bigmodel.cn",
    "minimax.chat",
    "stepfun.com",
    "aliyun.com",
];

fn probe_allowlisted(host: &str) -> bool {
    let lower = host.trim().trim_end_matches('.').to_ascii_lowercase();
    for entry in BUILTIN_MODEL_ALLOWLIST {
        if lower == *entry || lower.ends_with(&format!(".{}", entry)) {
            return true;
        }
    }
    if let Ok(list) = std::env::var("PONYLLM_PROBE_ALLOWLIST") {
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
        let allow_loopback = std::env::var("PONYLLM_ALLOW_LOOPBACK_PROBE").as_deref() == Ok("1");
        if allow_loopback {
            return None;
        }
        return Some("loopback hostname 'localhost' is not allowed for admin probes");
    }
    if lower == "svc" || lower.ends_with(".svc") || lower.ends_with(".svc.cluster.local") {
        return Some("kubernetes in-cluster names (*.svc) are not allowed for admin probes");
    }
    if METADATA_HOSTS
        .iter()
        .any(|m| lower == *m || lower.ends_with(&format!(".{}", m)))
    {
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
pub(crate) fn parse_host(raw: &str) -> Result<String, String> {
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
    // B7: the operator allowlist exempts exact proxy hosts AND literal IPs
    // (on-prem/LAN proxy), consistent with the probe path.
    if probe_allowlisted(host) {
        return Ok(());
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

/// Egress-pool entry policy (contract C3): `direct` / `none` / empty are
/// legal (gateway node's own exit); any other entry must pass the proxy URL
/// policy above. Delegates to `ponyllm_config::validate_egress_entry` — the
/// single source shared with the config-side C3 acceptance tests — so the
/// config model and the admin write path can never drift apart.
pub fn check_egress_pool_entry(raw: &str) -> Result<(), String> {
    ponyllm_config::validate_egress_entry(raw)
}

/// Redact credentials from an egress entry for admin VIEWS only (review
/// VIEW-CREDENTIAL-ECHO): `direct` and credential-free entries are returned
/// verbatim (byte-identical to the configured string — the quota/providers
/// views key off the raw value); an entry carrying userinfo is rebuilt as
/// `scheme://host:port` with the userinfo dropped. The runtime executor keeps
/// the raw URL (pproxy auth must reach the dialer); only display is sanitized.
pub fn sanitize_egress_entry_for_view(entry: &str) -> String {
    let trimmed = entry.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("direct")
        || trimmed.eq_ignore_ascii_case("none")
    {
        return trimmed.to_string();
    }
    match reqwest::Url::parse(trimmed) {
        Ok(url) => {
            if url.username().is_empty() && url.password().is_none() {
                // No credentials: keep the configured verbatim string so view
                // assertions and operator muscle-memory match the config.
                trimmed.to_string()
            } else {
                let host = url.host_str().unwrap_or("");
                match url.port() {
                    Some(port) => format!("{}://{}:{}", url.scheme(), host, port),
                    None => format!("{}://{}", url.scheme(), host),
                }
            }
        }
        Err(_) => trimmed.to_string(),
    }
}

/// Synchronous (no-DNS-for-literals) policy check. Used by the create/update
/// provider write path so deployments fail fast on obvious mistakes without
/// paying a DNS lookup inside the write lock.
pub fn check_probe_url_fast(raw: &str) -> Result<(), String> {
    let host = parse_host(raw)?;
    // B7: the operator allowlist exempts the target BEFORE any name/IP
    // policy — exact hosts AND literal IPs (e.g. a LAN model server at
    // `10.0.0.5`) are permitted explicitly by the operator.
    if probe_allowlisted(&host) {
        return Ok(());
    }
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
    // Operator allowlist (see probe_allowlisted) also skips the IP re-check:
    // an allowlisted LAN name/literal necessarily points at a target the
    // operator explicitly permitted (B7).
    if probe_allowlisted(&host) {
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
    let addrs: Vec<IpAddr> =
        match tokio::time::timeout(std::time::Duration::from_secs(5), lookup).await {
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
        return Err(format!(
            "DNS resolution returned no addresses for '{}'",
            host
        ));
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

/// Allowlist matching primitive shared by the `*.svc` exemption below.
///
/// Mirrors the semantics of [`probe_allowlisted`] (`host == entry ||
/// host.ends_with("." + entry)`, i.e. an entry covers itself and its
/// subdomains) without the per-entry `format!` allocation. `lower` must
/// already be trimmed / trailing-dot-stripped / lowercased; `entry` is
/// normalized here.
fn matches_allowlist_entry(lower: &str, entry: &str) -> bool {
    let e = entry.trim().trim_end_matches('.').to_ascii_lowercase();
    if e.is_empty() {
        return false;
    }
    lower == e
        || lower
            .strip_suffix(e.as_str())
            .is_some_and(|head| head.ends_with('.'))
}

/// First entry of `entries` that matches `lower` (already normalized),
/// returned in normalized (lowercased, trailing-dot-free) form.
fn first_matching_allowlist_entry(lower: &str, entries: &[&str]) -> Option<String> {
    entries
        .iter()
        .find(|entry| matches_allowlist_entry(lower, entry))
        .map(|entry| entry.trim().trim_end_matches('.').to_ascii_lowercase())
}

/// Whether an allowlist entry is specific enough to unlock an in-cluster
/// `*.svc` data-plane upstream.
///
/// The entry must itself be a MULTI-LABEL in-cluster service name, i.e. it
/// ends with `.svc` or `.svc.cluster.local`. This suffix requirement is a
/// deliberate anti-footgun guard: a bare TLD-ish entry (`svc`,
/// `svc.cluster.local`, `cluster.local`) is a plausible operator typo, and
/// because entries also match subdomains such a typo would unlock EVERY
/// `*.svc` host in the cluster. Requiring the `.svc` suffix confines an
/// over-broad operator mistake to the one service tree it was written for.
fn allowlist_entry_is_svc_name(entry: &str) -> bool {
    entry.ends_with(".svc") || entry.ends_with(".svc.cluster.local")
}

/// Whether the operator allowlist explicitly unlocks `host` as an in-cluster
/// data-plane upstream (the only sanctioned exemption from the `*.svc` block).
///
/// All three conditions must hold, and they are expressed fail-closed:
/// 1. [`probe_allowlisted`] hits `host` — the very same sources and matching
///    semantics the rest of the data-plane policy uses, so the exemption can
///    never be wider than "this host is on the allowlist". Checking the
///    aggregate predicate (rather than only this helper) keeps the exemption
///    closed if `probe_allowlisted` ever gains a source this helper does not
///    know about.
/// 2. The entry that actually matched is itself a `.svc` / `.svc.cluster.local`
///    suffixed name — see [`allowlist_entry_is_svc_name`].
/// 3. `host` is not a cloud-metadata host. That is guaranteed by ORDER, not
///    by this function: [`data_plane_blocked_name`] evaluates the metadata
///    blocklist before it ever considers the `*.svc` branch, so metadata hosts
///    return a refusal no matter what the allowlist says.
fn svc_allowlist_unlocks(host: &str) -> bool {
    if !probe_allowlisted(host) {
        return false;
    }
    let lower = host.trim().trim_end_matches('.').to_ascii_lowercase();
    let env_list = std::env::var("PONYLLM_PROBE_ALLOWLIST").unwrap_or_default();
    let env_entries: Vec<&str> = env_list.split(',').collect();
    let matched = first_matching_allowlist_entry(&lower, BUILTIN_MODEL_ALLOWLIST)
        .or_else(|| first_matching_allowlist_entry(&lower, &env_entries));
    // No entry matched even though probe_allowlisted() says otherwise (divergent
    // sources): fail closed.
    match matched {
        Some(entry) => allowlist_entry_is_svc_name(&entry),
        None => false,
    }
}

/// Name-based rejections for the data plane (VULN-07/F6): k8s in-cluster
/// names and cloud-metadata hosts stay blocked by name even if DNS is
/// unavailable. Loopback hostnames are deliberately ALLOWED here — the
/// documented data-plane shape includes local model servers (Ollama on
/// 127.0.0.1); `PONYLLM_PROBE_ALLOWLIST` still lifts LAN model-server names.
///
/// 为什么这里要与 admin 写入闸门对齐 / why align with the admin write gate:
/// `is_blocked_name` (admin writes) consults `PONYLLM_PROBE_ALLOWLIST` BEFORE
/// its `*.svc` block, so an operator can legitimately save an in-cluster
/// upstream (the shipped `pproxy-host.ponyllm.svc` reverse-route shape). The
/// data plane used to block the very same name unconditionally, which meant a
/// config the admin API accepted was refused at dial time with HTTP 503 —
/// "能写进去、不能用" 的策略不一致。The only sanctioned way to re-use an
/// in-cluster name as a data-plane upstream is now an EXPLICIT, service-scoped
/// allowlist entry (see [`svc_allowlist_unlocks`]); everything else keeps the
/// VULN-07/F6 block.
///
/// 云元数据仍然硬拒 / cloud metadata stays hard-blocked: the metadata check
/// runs FIRST and returns before any allowlist reasoning, so no allowlist entry
/// (including a `.svc`-suffixed one) can ever unlock `169.254.169.254` /
/// `metadata.google.internal`. 允许的豁免只针对 in-cluster 名称，封禁元数据端点不是本次变更的目标，也不是它的副作用.
fn data_plane_blocked_name(host: &str) -> Option<&'static str> {
    let lower = host.trim().trim_end_matches('.').to_ascii_lowercase();
    // (1) Cloud metadata FIRST — never allowlist-exemptible, by construction.
    if METADATA_HOSTS
        .iter()
        .any(|m| lower == *m || lower.ends_with(&format!(".{}", m)))
    {
        return Some("cloud metadata endpoints are not allowed for data-plane upstreams");
    }
    // (2) k8s in-cluster names — blocked unless explicitly unlocked above.
    if lower == "svc" || lower.ends_with(".svc") || lower.ends_with(".svc.cluster.local") {
        if svc_allowlist_unlocks(host) {
            return None;
        }
        return Some(
            "kubernetes in-cluster names (*.svc) are not allowed for data-plane upstreams",
        );
    }
    None
}

/// Data-plane upstream refusal with a stability classification so the verdict
/// cache can treat transient DNS failures differently from deterministic
/// policy rejections (see `AppState::data_plane_egress_guard`).
#[derive(Debug, Clone)]
pub struct DataPlaneRefusal {
    pub reason: String,
    /// `true` = environment-jitter class (DNS timeout / resolution error /
    /// empty result / join failure): short-lived, must NOT land in a long
    /// negative cache. `false` = deterministic policy rejection (blocklisted
    /// name, private literal IP, resolved private IP).
    pub transient: bool,
}

impl From<DataPlaneRefusal> for String {
    fn from(r: DataPlaneRefusal) -> String {
        r.reason
    }
}

/// Injectable DNS lookup outcome for `check_data_plane_url_with_resolver`
/// (deterministic unit tests without real wall-clock DNS).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DnsLookupError {
    /// Resolution exceeded the fail-closed bound.
    Timeout,
    /// Resolver returned an error.
    Failure(String),
}

fn data_plane_refusal(reason: impl Into<String>) -> DataPlaneRefusal {
    DataPlaneRefusal {
        reason: reason.into(),
        transient: false,
    }
}

/// Shared data-plane fast policy (VULN-07/F6): scheme + in-cluster/metadata
/// name blocklist + operator allowlist + literal-IP blocklist. NO DNS — used
/// by both the direct check (before resolution) and the proxied fast path.
/// Loopback stays ALLOWED (documented data-plane shape — local Ollama);
/// `PONYLLM_PROBE_ALLOWLIST` lifts LAN model-server names (B7).
pub(crate) fn check_data_plane_policy_fast(raw: &str) -> Result<(), DataPlaneRefusal> {
    let host = parse_host(raw).map_err(data_plane_refusal)?;
    if let Some(reason) = data_plane_blocked_name(&host) {
        return Err(data_plane_refusal(reason));
    }
    if probe_allowlisted(&host) {
        return Ok(());
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_blocked_ip_with(&ip, true) {
            return Err(data_plane_refusal(format!(
                "data-plane upstream '{}' resolves to a blocked address ({})",
                host, ip
            )));
        }
        return Ok(());
    }
    Ok(())
}

/// Whether an effective proxy URL is eligible for the data-plane proxied fast
/// path (skip local DNS). MUST mirror the actual client construction so the
/// guard never thinks "proxied" while the client dials direct:
/// - scheme must be `http`/`https` (workspace reqwest has NO socks feature,
///   so `Proxy::all("socks5://…")` fails and the client silently falls back
///   to a direct dial — skipping DNS there would remove SSRF protection);
/// - `reqwest::Proxy::all` must parse it (same source as the client build);
/// - the target host must not be no_proxy-exempt (`localhost`/`127.x` dial
///   direct regardless of the proxy);
/// - the proxy URL itself must pass `check_proxy_url_fast` (defense in depth
///   if a config write poisoned it after the write-time check).
pub fn proxy_fast_path_eligible(proxy_url: &str, target_url: &str) -> bool {
    let trimmed = proxy_url.trim();
    if trimmed.is_empty() {
        return false;
    }
    let scheme_end = match trimmed.find("://") {
        Some(e) => e,
        None => return false,
    };
    let scheme = trimmed[..scheme_end].to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return false;
    }
    if reqwest::Proxy::all(trimmed).is_err() {
        return false;
    }
    if check_proxy_url_fast(trimmed).is_err() {
        return false;
    }
    match parse_host(target_url) {
        Ok(host) => {
            let lower = host.trim().trim_end_matches('.').to_ascii_lowercase();
            // Align with the client's static no_proxy list ("localhost,127.0.0.1",
            // matched by hyper-util as domain + *.localhost subdomains): any of
            // these dial DIRECT even with an explicit proxy, so the guard must
            // NOT skip DNS for them (a *.localhost target would otherwise pass
            // the fast policy while the client bypasses the proxy).
            if lower == "localhost"
                || lower == "127.0.0.1"
                || lower.starts_with("127.")
                || lower.ends_with(".localhost")
            {
                return false;
            }
            true
        }
        Err(_) => false,
    }
}

/// Data-plane (inference) upstream policy check (VULN-07/F6) with an
/// injectable resolver seam.
///
/// Fast policy first (scheme / name / literal-IP / allowlist, no DNS), then —
/// ONLY for non-allowlisted hostnames — the resolver is consulted and EVERY
/// resolved address must pass the shared IP policy (same block table as
/// `check_probe_url`, with `allow_loopback=true` for the data plane). The
/// resolution runs on `spawn_blocking` bounded 5s fail-closed so a hung
/// authoritative server cannot pin an async worker; refusal is classified
/// `transient` when DNS itself failed (timeout / error / empty / join), and
/// deterministic when a private address was resolved.
pub async fn check_data_plane_url_with_resolver<F>(
    raw: &str,
    resolver: F,
) -> Result<(), DataPlaneRefusal>
where
    F: Fn(&str) -> Result<Vec<IpAddr>, DnsLookupError> + Send + Sync + 'static,
{
    check_data_plane_policy_fast(raw)?;
    let host = parse_host(raw).map_err(data_plane_refusal)?;
    // Literals and allowlisted names were decided by the fast policy; a
    // hostname here still needs resolution.
    if probe_allowlisted(&host) || host.parse::<IpAddr>().is_ok() {
        return Ok(());
    }
    let resolver = std::sync::Arc::new(resolver);
    // Perform DNS lookup with timeout, and retry once on transient timeout or failure
    // to absorb transient network jitter before declaring fail-closed.
    let mut last_refusal: Option<DataPlaneRefusal> = None;
    let mut addrs: Vec<IpAddr> = Vec::new();

    for attempt in 0..2 {
        let host_for_lookup = host.clone();
        let resolver_clone = resolver.clone();
        let lookup = tokio::task::spawn_blocking(move || resolver_clone(&host_for_lookup));

        match tokio::time::timeout(std::time::Duration::from_secs(5), lookup).await {
            Ok(Ok(Ok(resolved))) if !resolved.is_empty() => {
                addrs = resolved;
                last_refusal = None;
                break;
            }
            Ok(Ok(Ok(_empty))) => {
                last_refusal = Some(DataPlaneRefusal {
                    reason: format!("DNS resolution returned no addresses for '{}'", host),
                    transient: true,
                });
            }
            Ok(Ok(Err(e))) => {
                last_refusal = Some(match e {
                    DnsLookupError::Timeout => DataPlaneRefusal {
                        reason: format!(
                            "DNS resolution timed out for '{}' (blocked fail-closed)",
                            host
                        ),
                        transient: true,
                    },
                    DnsLookupError::Failure(msg) => DataPlaneRefusal {
                        reason: format!("DNS resolution failed for '{}': {}", host, msg),
                        transient: true,
                    },
                });
            }
            Ok(Err(e)) => {
                last_refusal = Some(DataPlaneRefusal {
                    reason: format!("DNS lookup task failed: {}", e),
                    transient: true,
                });
            }
            Err(_) => {
                last_refusal = Some(DataPlaneRefusal {
                    reason: format!(
                        "DNS resolution timed out for '{}' (blocked fail-closed)",
                        host
                    ),
                    transient: true,
                });
            }
        }

        if attempt == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    if let Some(refusal) = last_refusal {
        return Err(refusal);
    }
    for ip in &addrs {
        if is_blocked_ip_with(ip, true) {
            return Err(data_plane_refusal(format!(
                "data-plane upstream '{}' resolves to a blocked address ({})",
                host, ip
            )));
        }
    }
    Ok(())
}

/// Data-plane (inference) upstream policy check for targets dialed DIRECTLY
/// (the dial-time re-validation documented on the old `check_data_plane_url`).
/// DNS is re-resolved here at dial time via the system resolver (blocking
/// `getaddrinfo` offloaded via `spawn_blocking`, bounded 5s fail-closed) so a
/// provider hostname rebinding to an internal address AFTER the write-time
/// check is refused before the connection is attempted.
pub async fn check_data_plane_url(raw: &str) -> Result<(), DataPlaneRefusal> {
    check_data_plane_url_with_resolver(raw, |host| {
        (host, 0)
            .to_socket_addrs()
            .map(|it| it.map(|s| s.ip()).collect::<Vec<IpAddr>>())
            .map_err(|e| DnsLookupError::Failure(format!("{}", e)))
    })
    .await
}

/// Data-plane policy check for targets dialed through an explicit forward
/// proxy (proxied fast path): the trusted proxy owns DNS + egress for the
/// target, the gateway's local resolution does not determine where bytes go,
/// and for GFW-blocked domains local DNS is unreliable (observed >5s hangs
/// through CoreDNS→Chinese public resolvers). Fast policy only — name /
/// literal / allowlist, NO DNS. See
/// `.agents/notes/implemented/bug-fix/2026-10-06-egress-guard-proxied-dns-skip.md`.
pub async fn check_data_plane_url_proxied(raw: &str) -> Result<(), DataPlaneRefusal> {
    check_data_plane_policy_fast(raw)
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
                    [(
                        axum::http::header::LOCATION,
                        "http://169.254.169.254/latest/meta-data/",
                    )],
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
