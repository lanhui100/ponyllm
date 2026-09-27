//! Opt-in commercial profile (`[commercial]`), Stage 1 scaffolding.
//!
//! Normative contracts: `docs/commercial-stage0-rfc.md` (`CONFIG_STAGE_CONTRACT`,
//! `MONEY_CONTRACT`, `LEASE_FENCING_CONTRACT`) and the Stage 1 boundary in
//! `.agents/notes/proposed/architecture/2026-09-27-commercial-platform-roadmap.md`
//! §0.5/§1.1. This module adds configuration, schema and validation scaffolding
//! only: **paid inference stays hard-disabled** — `enabled` defaults to `false`
//! and no runtime in this workspace consumes the section yet.
//!
//! # Zero migration
//!
//! An old TOML without a `[commercial]` table deserializes to
//! [`CommercialConfig::default()`] (`enabled = false`, every money-affecting
//! value at its canonical value). Nothing has to be rewritten on upgrade.
//!
//! # Fail closed
//!
//! [`CommercialConfig::validate`] rejects, with a distinct typed error per
//! condition: a non-`USD` currency, a lease other than 30s, a heartbeat other
//! than 10s, retention under 180 days, and — while enabled — an
//! empty/`none`/placeholder admin reference or an empty/unparsable/wildcard
//! listen address. Money-affecting values are rejected even while disabled: a
//! persisted value that can never become valid must not lie in wait for a later
//! `enabled = true`. Unknown TOML keys and wrong value types fail at parse time
//! (`deny_unknown_fields`), never silently falling back to a default.
//!
//! # Secrets are never persisted
//!
//! The section deliberately has **no field that can hold a raw secret value**.
//! `commercial_admin_ref` is an opaque *name/reference* (an environment variable
//! or secret-manager key) whose value is injected out-of-band at startup, and
//! `commercial_bind` is a listen address. [`COMMERCIAL_CONFIG_KEYS`] is the
//! frozen allowlist of serialized keys and
//! `tests::commercial_section_serializes_exactly_the_documented_keys` pins it;
//! `deny_unknown_fields` additionally rejects a TOML that tries to smuggle a
//! secret-bearing key such as `commercial_admin_secret` into the table.

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

/// The only currency supported by the commercial profile (RFC `MONEY_CONTRACT`).
pub const COMMERCIAL_CURRENCY_USD: &str = "USD";

/// Canonical lease duration in seconds (RFC `LEASE_FENCING_CONTRACT`).
pub const COMMERCIAL_LEASE_SECONDS: u64 = 30;

/// Canonical heartbeat period in seconds (RFC `LEASE_FENCING_CONTRACT`).
pub const COMMERCIAL_HEARTBEAT_SECONDS: u64 = 10;

/// Minimum idempotency/audit retention in days (RFC `IDEMPOTENCY_CONTRACT`).
pub const COMMERCIAL_MIN_RETENTION_DAYS: u64 = 180;

/// Frozen allowlist of serialized `[commercial]` keys.
///
/// The section must never grow a key outside this list: none of them can hold a
/// raw secret value. Any change here is a contract change and must update the
/// RFC, the README, and the key-set regression test together.
pub const COMMERCIAL_CONFIG_KEYS: &[&str] = &[
    "enabled",
    "currency",
    "lease_seconds",
    "heartbeat_seconds",
    "egress_allowlist",
    "retention_days",
    "commercial_bind",
    "commercial_admin_ref",
];

/// Admin-reference values that are placeholders rather than a real
/// out-of-band secret name (RFC: empty/`none`/default credentials reject
/// startup). Compared case-insensitively against the trimmed value.
const ADMIN_REF_PLACEHOLDERS: &[&str] = &[
    "none",
    "null",
    "nil",
    "default",
    "changeme",
    "change-me",
    "placeholder",
    "todo",
    "tbd",
    "secret",
    "password",
    "xxx",
];

/// Typed, non-secret validation failure for the `[commercial]` section.
///
/// One variant per fail-closed condition so startup can log a stable code
/// ([`CommercialConfigError::code`]) and tests can assert the exact reason.
/// Variants never carry the full config, and [`CommercialConfigError::BindUnsafe`]
/// carries only the operator-supplied listen address (never a credential).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommercialConfigError {
    /// `currency` is not exactly `USD`.
    UnsupportedCurrency { found: String },
    /// `lease_seconds` differs from the canonical 30.
    InvalidLeaseSeconds { found: u64 },
    /// `heartbeat_seconds` differs from the canonical 10.
    InvalidHeartbeatSeconds { found: u64 },
    /// `retention_days` is below the 180-day floor.
    InsufficientRetentionDays { found: u64 },
    /// Enabled with an absent/empty `commercial_admin_ref`.
    AdminReferenceMissing,
    /// Enabled with a placeholder `commercial_admin_ref` (e.g. `none`).
    AdminReferencePlaceholder { found: String },
    /// Enabled with an absent/empty `commercial_bind`.
    BindMissing,
    /// Enabled with a `commercial_bind` that is not `host:port`.
    BindUnparsable { found: String },
    /// Enabled with a wildcard/unspecified/multicast address or port 0.
    BindUnsafe { found: String, reason: &'static str },
}

impl CommercialConfigError {
    /// Stable machine code for startup logs and operator runbooks.
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedCurrency { .. } => "invalid_currency",
            Self::InvalidLeaseSeconds { .. } => "invalid_lease_seconds",
            Self::InvalidHeartbeatSeconds { .. } => "invalid_heartbeat_seconds",
            Self::InsufficientRetentionDays { .. } => "insufficient_retention_days",
            Self::AdminReferenceMissing => "commercial_admin_ref_missing",
            Self::AdminReferencePlaceholder { .. } => "commercial_admin_ref_placeholder",
            Self::BindMissing => "commercial_bind_missing",
            Self::BindUnparsable { .. } => "commercial_bind_unparsable",
            Self::BindUnsafe { .. } => "commercial_bind_unsafe",
        }
    }
}

impl std::fmt::Display for CommercialConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedCurrency { found } => write!(
                f,
                "[{}] commercial.currency must be \"{}\" (ISO-4217, RFC MONEY_CONTRACT); got {:?}",
                self.code(),
                COMMERCIAL_CURRENCY_USD,
                found
            ),
            Self::InvalidLeaseSeconds { found } => write!(
                f,
                "[{}] commercial.lease_seconds must be exactly {} (RFC LEASE_FENCING_CONTRACT); got {}",
                self.code(),
                COMMERCIAL_LEASE_SECONDS,
                found
            ),
            Self::InvalidHeartbeatSeconds { found } => write!(
                f,
                "[{}] commercial.heartbeat_seconds must be exactly {} (RFC LEASE_FENCING_CONTRACT); got {}",
                self.code(),
                COMMERCIAL_HEARTBEAT_SECONDS,
                found
            ),
            Self::InsufficientRetentionDays { found } => write!(
                f,
                "[{}] commercial.retention_days must be >= {} (RFC IDEMPOTENCY_CONTRACT); got {}",
                self.code(),
                COMMERCIAL_MIN_RETENTION_DAYS,
                found
            ),
            Self::AdminReferenceMissing => write!(
                f,
                "[{}] commercial is enabled but commercial_admin_ref is empty; set the name of the out-of-band injected admin secret/role (never the secret value itself)",
                self.code()
            ),
            Self::AdminReferencePlaceholder { found: _ } => write!(
                f,
                "[{}] commercial_admin_ref is invalid or placeholder; must match ^env:[A-Z0-9_]+$ or ^secret:[a-zA-Z0-9_/.-]+$",
                self.code()
            ),
            Self::BindMissing => write!(
                f,
                "[{}] commercial is enabled but commercial_bind is empty; set a dedicated host:port listener",
                self.code()
            ),
            Self::BindUnparsable { found } => write!(
                f,
                "[{}] commercial_bind {:?} is not a valid host:port listen address",
                self.code(),
                found
            ),
            Self::BindUnsafe { found, reason } => write!(
                f,
                "[{}] commercial_bind {:?} is unsafe: {}",
                self.code(),
                found,
                reason
            ),
        }
    }
}

impl std::error::Error for CommercialConfigError {}

/// Opt-in commercial profile: the `[commercial]` TOML table.
///
/// Absent by default with `enabled = false`; see the module docs for the
/// fail-closed and no-persisted-secret contracts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommercialConfig {
    /// Master switch. Defaults to `false`; paid inference remains hard-disabled
    /// until the Stage 2 reservation/settlement path is integrated.
    #[serde(default)]
    pub enabled: bool,
    /// ISO-4217 currency of the commercial ledger; only `USD` is accepted.
    #[serde(default = "default_commercial_currency")]
    pub currency: String,
    /// Work-ownership lease duration in seconds; must equal 30.
    #[serde(default = "default_commercial_lease_seconds")]
    pub lease_seconds: u64,
    /// Lease heartbeat period in seconds; must equal 10.
    #[serde(default = "default_commercial_heartbeat_seconds")]
    pub heartbeat_seconds: u64,
    /// Egress allowlist of exact `https` hostnames (Stage 1.5 enforces the
    /// policy; this stage only carries the list). Empty = no commercial egress.
    #[serde(default)]
    pub egress_allowlist: Vec<String>,
    /// Idempotency/audit retention in days; must be >= 180.
    #[serde(default = "default_commercial_retention_days")]
    pub retention_days: u64,
    /// Dedicated commercial listener as `host:port` (IPv6 in brackets).
    /// Empty = unset; required when [`Self::enabled`]. This is an address, not
    /// a credential.
    #[serde(default)]
    pub commercial_bind: String,
    /// Name/reference of the separately injected commercial admin secret/role
    /// (for example `env:PONYLLM_COMMERCIAL_ADMIN`). **The value itself is
    /// never persisted here or anywhere in this section**; it is supplied
    /// out-of-band at startup. Empty = unset; required when [`Self::enabled`].
    #[serde(default)]
    pub commercial_admin_ref: String,
}

pub fn default_commercial_currency() -> String {
    COMMERCIAL_CURRENCY_USD.to_string()
}

pub fn default_commercial_lease_seconds() -> u64 {
    COMMERCIAL_LEASE_SECONDS
}

pub fn default_commercial_heartbeat_seconds() -> u64 {
    COMMERCIAL_HEARTBEAT_SECONDS
}

pub fn default_commercial_retention_days() -> u64 {
    COMMERCIAL_MIN_RETENTION_DAYS
}

impl Default for CommercialConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            currency: default_commercial_currency(),
            lease_seconds: default_commercial_lease_seconds(),
            heartbeat_seconds: default_commercial_heartbeat_seconds(),
            egress_allowlist: Vec::new(),
            retention_days: default_commercial_retention_days(),
            commercial_bind: String::new(),
            commercial_admin_ref: String::new(),
        }
    }
}

impl CommercialConfig {
    /// Fail-closed validation gate. Callers (server startup, the admin config
    /// swap path) must not activate commercial mode unless this returns `Ok`.
    ///
    /// Money-affecting values (currency, lease, heartbeat, retention) are
    /// checked regardless of [`Self::enabled`]: an invalid persisted value is
    /// rejected instead of silently armed for a later `enabled = true`.
    /// Identity/listener requirements are checked only while enabled, so the
    /// disabled default profile needs no placeholder values.
    pub fn validate(&self) -> Result<(), CommercialConfigError> {
        if self.currency != COMMERCIAL_CURRENCY_USD {
            return Err(CommercialConfigError::UnsupportedCurrency {
                found: self.currency.clone(),
            });
        }
        if self.lease_seconds != COMMERCIAL_LEASE_SECONDS {
            return Err(CommercialConfigError::InvalidLeaseSeconds {
                found: self.lease_seconds,
            });
        }
        if self.heartbeat_seconds != COMMERCIAL_HEARTBEAT_SECONDS {
            return Err(CommercialConfigError::InvalidHeartbeatSeconds {
                found: self.heartbeat_seconds,
            });
        }
        if self.retention_days < COMMERCIAL_MIN_RETENTION_DAYS {
            return Err(CommercialConfigError::InsufficientRetentionDays {
                found: self.retention_days,
            });
        }

        if !self.enabled {
            return Ok(());
        }

        self.validate_admin_ref()?;
        self.validate_bind()?;
        Ok(())
    }

    fn validate_admin_ref(&self) -> Result<(), CommercialConfigError> {
        let trimmed = self.commercial_admin_ref.trim();
        if trimmed.is_empty() {
            return Err(CommercialConfigError::AdminReferenceMissing);
        }
        let lower = trimmed.to_ascii_lowercase();
        if ADMIN_REF_PLACEHOLDERS.contains(&lower.as_str()) {
            return Err(CommercialConfigError::AdminReferencePlaceholder {
                found: trimmed.to_string(),
            });
        }
        // Strict reference grammar (security P1-2): must be an out-of-band reference,
        // strictly matching `env:[A-Z0-9_]+` or `secret:[a-zA-Z0-9_/.-]+`.
        // Raw API keys or literal credentials (e.g. `sk-...`, `bearer ...`) are explicitly blocked.
        let is_valid_ref = (trimmed.starts_with("env:")
            && trimmed[4..].chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            && !trimmed[4..].is_empty())
            || (trimmed.starts_with("secret:")
                && trimmed[7..].chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '/' | '.' | '-'))
                && !trimmed[7..].is_empty());
        if !is_valid_ref || lower.starts_with("sk-") || lower.starts_with("bearer ") {
            return Err(CommercialConfigError::AdminReferencePlaceholder {
                found: trimmed.to_string(),
            });
        }
        Ok(())
    }

    fn validate_bind(&self) -> Result<(), CommercialConfigError> {
        let raw = self.commercial_bind.trim();
        if raw.is_empty() {
            return Err(CommercialConfigError::BindMissing);
        }
        let (host, port) = parse_host_port(raw).ok_or_else(|| {
            CommercialConfigError::BindUnparsable {
                found: raw.to_string(),
            }
        })?;
        if port == 0 {
            return Err(CommercialConfigError::BindUnsafe {
                found: raw.to_string(),
                reason: "port 0 asks the OS for an arbitrary port",
            });
        }
        if host.contains('*') {
            return Err(CommercialConfigError::BindUnsafe {
                found: raw.to_string(),
                reason: "wildcard host",
            });
        }
        if let Ok(ip) = host.parse::<IpAddr>() {
            let unsafe_ip = match ip {
                IpAddr::V4(v4) => {
                    v4.is_unspecified()
                        || v4.is_multicast()
                        || v4 == std::net::Ipv4Addr::BROADCAST
                }
                IpAddr::V6(v6) => {
                    v6.is_unspecified()
                        || v6.is_multicast()
                        || v6.to_ipv4_mapped()
                            .map(|m| {
                                m.is_unspecified()
                                    || m.is_multicast()
                                    || m == std::net::Ipv4Addr::BROADCAST
                            })
                            .unwrap_or(false)
                }
            };
            if unsafe_ip {
                return Err(CommercialConfigError::BindUnsafe {
                    found: raw.to_string(),
                    reason: "wildcard/unspecified/multicast listen address",
                });
            }
            return Ok(());
        }
        if !is_valid_hostname(host) {
            return Err(CommercialConfigError::BindUnparsable {
                found: raw.to_string(),
            });
        }
        Ok(())
    }
}

/// Split `host:port` (or `[v6]:port`) without DNS resolution. Returns `None`
/// when the shape, host or port is invalid.
fn parse_host_port(raw: &str) -> Option<(&str, u16)> {
    if raw.chars().any(|c| c.is_whitespace()) {
        return None;
    }
    if let Some(rest) = raw.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        let port = tail.strip_prefix(':')?;
        if host.is_empty() {
            return None;
        }
        return Some((host, port.parse::<u16>().ok()?));
    }
    let (host, port) = raw.rsplit_once(':')?;
    // A bare (unbracketed) IPv6 literal or a stray colon is not a valid bind.
    if host.is_empty() || host.contains(':') || host.contains('/') {
        return None;
    }
    Some((host, port.parse::<u16>().ok()?))
}

/// Conservative hostname check (labels of 1..=63 alphanumerics/`-`/`_`,
/// optional single trailing dot). IP literals are handled before this.
fn is_valid_hostname(host: &str) -> bool {
    let host = host.strip_suffix('.').unwrap_or(host);
    if host.is_empty() || host.len() > 253 {
        return false;
    }
    host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Enabled profile that passes validation, used as the base for
    /// single-field failure tests.
    fn enabled_profile() -> CommercialConfig {
        CommercialConfig {
            enabled: true,
            commercial_bind: "127.0.0.1:9443".to_string(),
            commercial_admin_ref: "env:PONYLLM_COMMERCIAL_ADMIN".to_string(),
            ..CommercialConfig::default()
        }
    }

    fn section_keys(config: &CommercialConfig) -> Vec<String> {
        let file = crate::ConfigFile {
            commercial: config.clone(),
            ..crate::ConfigFile::default()
        };
        let text = toml::to_string_pretty(&file).expect("config serializes");
        let value: toml::Value = toml::from_str(&text).expect("serialized config parses");
        let mut keys: Vec<String> = value
            .get("commercial")
            .and_then(|v| v.as_table())
            .expect("[commercial] table is always emitted")
            .keys()
            .cloned()
            .collect();
        keys.sort();
        keys
    }

    // -- zero migration ----------------------------------------------------

    #[test]
    fn old_config_without_commercial_section_defaults_to_disabled() {
        let old_toml = r#"
[gateway]
bind = "127.0.0.1:8080"
max_retries = 3
flight_recorder_capacity = 200
api_key = "test-key"

[providers.deepseek]
base_url = "https://api.deepseek.com"
default_model = "deepseek-chat"
strategy = "priority"
keys = [
    { id = "k1", api_key = "sk-x", priority = 1, weight = 10 },
]
"#;
        let cfg: crate::ConfigFile = toml::from_str(old_toml).unwrap();
        let commercial = &cfg.commercial;
        assert!(!commercial.enabled, "absent section must default to disabled");
        assert_eq!(commercial.currency, "USD");
        assert_eq!(commercial.lease_seconds, 30);
        assert_eq!(commercial.heartbeat_seconds, 10);
        assert!(commercial.egress_allowlist.is_empty());
        assert_eq!(commercial.retention_days, 180);
        assert_eq!(commercial.commercial_bind, "");
        assert_eq!(commercial.commercial_admin_ref, "");
        assert_eq!(commercial, &CommercialConfig::default());
        assert_eq!(commercial.validate(), Ok(()));
    }

    #[test]
    fn disabled_mode_accepts_default_empty_values() {
        assert!(CommercialConfig::default().validate().is_ok());
        // Explicit, if redundant, opt-out is also accepted.
        let explicit = CommercialConfig {
            enabled: false,
            ..CommercialConfig::default()
        };
        assert_eq!(explicit.validate(), Ok(()));
        // And it survives a TOML round-trip with the same result.
        let file = crate::ConfigFile {
            commercial: explicit,
            ..crate::ConfigFile::default()
        };
        let text = toml::to_string_pretty(&file).unwrap();
        assert!(text.contains("[commercial]"));
        let reloaded: crate::ConfigFile = toml::from_str(&text).unwrap();
        assert_eq!(reloaded.commercial, CommercialConfig::default());
        assert_eq!(reloaded.commercial.validate(), Ok(()));
    }

    // -- round-trip --------------------------------------------------------

    #[test]
    fn commercial_section_toml_roundtrip_with_section_present() {
        let profile = CommercialConfig {
            enabled: true,
            currency: "USD".to_string(),
            lease_seconds: 30,
            heartbeat_seconds: 10,
            egress_allowlist: vec![
                "api.example.com".to_string(),
                "hooks.example.com".to_string(),
            ],
            retention_days: 365,
            commercial_bind: "[::1]:9443".to_string(),
            commercial_admin_ref: "env:PONYLLM_COMMERCIAL_ADMIN".to_string(),
        };
        let file = crate::ConfigFile {
            commercial: profile.clone(),
            ..crate::ConfigFile::default()
        };
        let text = toml::to_string_pretty(&file).unwrap();
        assert!(text.contains("[commercial]"));
        assert!(text.contains("egress_allowlist"));
        let reloaded: crate::ConfigFile = toml::from_str(&text).unwrap();
        assert_eq!(reloaded.commercial, profile);
        assert_eq!(reloaded.commercial.validate(), Ok(()));
    }

    // -- fail-closed validation -------------------------------------------

    #[test]
    fn validate_rejects_enabled_without_admin_reference() {
        let mut profile = enabled_profile();
        profile.commercial_admin_ref = String::new();
        assert_eq!(
            profile.validate(),
            Err(CommercialConfigError::AdminReferenceMissing)
        );
        profile.commercial_admin_ref = "   ".to_string();
        assert_eq!(
            profile.validate(),
            Err(CommercialConfigError::AdminReferenceMissing)
        );
    }

    #[test]
    fn validate_rejects_placeholder_admin_reference() {
        for placeholder in ["none", "NONE", "None ", "default", "changeme", "placeholder"] {
            let mut profile = enabled_profile();
            profile.commercial_admin_ref = placeholder.to_string();
            match profile.validate() {
                Err(CommercialConfigError::AdminReferencePlaceholder { found }) => {
                    assert_eq!(found, placeholder.trim())
                }
                other => panic!("placeholder {:?} must fail, got {:?}", placeholder, other),
            }
        }
    }

    #[test]
    fn validate_rejects_non_usd_currency() {
        for bad in ["eur", "usd", "US", "USDD", "USD ", ""] {
            let profile = CommercialConfig {
                currency: bad.to_string(),
                ..enabled_profile()
            };
            match profile.validate() {
                Err(CommercialConfigError::UnsupportedCurrency { found }) => {
                    assert_eq!(found, bad)
                }
                other => panic!("currency {:?} must fail, got {:?}", bad, other),
            }
        }
    }

    #[test]
    fn validate_rejects_lease_seconds_other_than_30() {
        for bad in [0u64, 10, 29, 31, 60] {
            let profile = CommercialConfig {
                lease_seconds: bad,
                ..enabled_profile()
            };
            assert_eq!(
                profile.validate(),
                Err(CommercialConfigError::InvalidLeaseSeconds { found: bad })
            );
        }
    }

    #[test]
    fn validate_rejects_heartbeat_seconds_other_than_10() {
        for bad in [0u64, 5, 9, 11, 30] {
            let profile = CommercialConfig {
                heartbeat_seconds: bad,
                ..enabled_profile()
            };
            assert_eq!(
                profile.validate(),
                Err(CommercialConfigError::InvalidHeartbeatSeconds { found: bad })
            );
        }
    }

    #[test]
    fn validate_rejects_retention_below_180_days() {
        for bad in [0u64, 1, 179] {
            let profile = CommercialConfig {
                retention_days: bad,
                ..enabled_profile()
            };
            assert_eq!(
                profile.validate(),
                Err(CommercialConfigError::InsufficientRetentionDays { found: bad })
            );
        }
        // 180 and above are accepted (RFC floor).
        for ok in [180u64, 181, 365] {
            let profile = CommercialConfig {
                retention_days: ok,
                ..enabled_profile()
            };
            assert_eq!(profile.validate(), Ok(()), "retention {} must pass", ok);
        }
    }

    #[test]
    fn validate_rejects_missing_or_unsafe_bind() {
        let mut missing = enabled_profile();
        missing.commercial_bind = "  ".to_string();
        assert_eq!(missing.validate(), Err(CommercialConfigError::BindMissing));

        // Unparsable shapes.
        for bad in [
            "127.0.0.1",
            ":9443",
            "127.0.0.1:",
            "127.0.0.1:notaport",
            "127.0.0.1:70000",
            "::1:9443",
            "[::1:9443",
            "http://127.0.0.1:9443",
            "bad host:9443",
            "127.0.0.1:9443 evil",
            // Missing port is a shape error, not an unsafe address.
            "0.0.0.0",
            // IPv6 literals must be bracketed (`SocketAddr` syntax); an
            // unbracketed form is ambiguous, never guessed.
            "0:0:0:0:0:0:0:0:9443",
        ] {
            let profile = CommercialConfig {
                commercial_bind: bad.to_string(),
                ..enabled_profile()
            };
            assert!(
                matches!(
                    profile.validate(),
                    Err(CommercialConfigError::BindUnparsable { .. })
                ),
                "bind {:?} must be unparsable, got {:?}",
                bad,
                profile.validate()
            );
        }

        // Wildcard/unsafe addresses and port 0.
        for bad in [
            "0.0.0.0:9443",
            "[::]:9443",
            "*:9443",
            "127.0.0.1:0",
            "255.255.255.255:9443",
            "[ff02::1]:9443",
        ] {
            let profile = CommercialConfig {
                commercial_bind: bad.to_string(),
                ..enabled_profile()
            };
            let result = profile.validate();
            assert!(
                matches!(result, Err(CommercialConfigError::BindUnsafe { .. })),
                "bind {:?} must be unsafe, got {:?}",
                bad,
                result
            );
        }

        // Explicit, non-wildcard listeners pass.
        for ok in [
            "127.0.0.1:9443",
            "[::1]:9443",
            "commercial.internal:9443",
            "commercial-gw.example.com:8443",
            "localhost:9443",
        ] {
            let profile = CommercialConfig {
                commercial_bind: ok.to_string(),
                ..enabled_profile()
            };
            assert_eq!(profile.validate(), Ok(()), "bind {} must pass", ok);
        }
    }

    #[test]
    fn money_affecting_values_fail_even_while_disabled() {
        // Fail closed: an invalid persisted value must not lie in wait for a
        // later `enabled = true`, and must not be silently replaced by the
        // canonical default.
        let cases = [
            CommercialConfig {
                currency: "EUR".to_string(),
                ..CommercialConfig::default()
            },
            CommercialConfig {
                lease_seconds: 60,
                ..CommercialConfig::default()
            },
            CommercialConfig {
                heartbeat_seconds: 1,
                ..CommercialConfig::default()
            },
            CommercialConfig {
                retention_days: 30,
                ..CommercialConfig::default()
            },
        ];
        for profile in cases {
            assert!(!profile.enabled);
            assert!(
                profile.validate().is_err(),
                "disabled profile must still reject {:?}",
                profile
            );
        }
    }

    // -- serde fail-fast ---------------------------------------------------

    #[test]
    fn error_codes_are_stable_and_distinct() {
        let all = [
            CommercialConfigError::UnsupportedCurrency {
                found: "EUR".to_string(),
            },
            CommercialConfigError::InvalidLeaseSeconds { found: 60 },
            CommercialConfigError::InvalidHeartbeatSeconds { found: 1 },
            CommercialConfigError::InsufficientRetentionDays { found: 1 },
            CommercialConfigError::AdminReferenceMissing,
            CommercialConfigError::AdminReferencePlaceholder {
                found: "none".to_string(),
            },
            CommercialConfigError::BindMissing,
            CommercialConfigError::BindUnparsable {
                found: "nope".to_string(),
            },
            CommercialConfigError::BindUnsafe {
                found: "0.0.0.0:9443".to_string(),
                reason: "wildcard",
            },
        ];
        let mut codes: Vec<&str> = all.iter().map(|e| e.code()).collect();
        codes.sort_unstable();
        let total = codes.len();
        codes.dedup();
        assert_eq!(codes.len(), total, "every condition needs its own code");
        for err in &all {
            assert!(
                err.to_string().contains(err.code()),
                "Display must carry the stable code: {err}"
            );
        }
    }

    #[test]
    fn commercial_section_rejects_unknown_keys_and_wrong_types() {
        // An unknown key would be a silent default (or a secret sink): reject.
        let with_secret =
            "[commercial]\nenabled = false\ncommercial_admin_secret = \"sk-live-oops\"\n";
        assert!(
            toml::from_str::<crate::ConfigFile>(with_secret).is_err(),
            "unknown secret-bearing key must fail to deserialize"
        );
        let with_typo = "[commercial]\nlease_second = 30\n";
        assert!(toml::from_str::<crate::ConfigFile>(with_typo).is_err());

        // Wrong types never coerce to a default.
        for bad in [
            "[commercial]\nenabled = \"yes\"\n",
            "[commercial]\nlease_seconds = -1\n",
            "[commercial]\nretention_days = \"180\"\n",
            "[commercial]\ncurrency = 840\n",
            "[commercial]\negress_allowlist = \"api.example.com\"\n",
            "[commercial]\ncommercial_bind = 9443\n",
        ] {
            assert!(
                toml::from_str::<crate::ConfigFile>(bad).is_err(),
                "{} must fail to deserialize",
                bad
            );
        }

        // An out-of-range but well-typed value parses (it is a valid u64) and
        // is rejected by the validation gate instead of being silently reset.
        let out_of_range: crate::ConfigFile =
            toml::from_str("[commercial]\nlease_seconds = 60\n").unwrap();
        assert_eq!(out_of_range.commercial.lease_seconds, 60);
        assert_eq!(
            out_of_range.commercial.validate(),
            Err(CommercialConfigError::InvalidLeaseSeconds { found: 60 })
        );
    }

    // -- no persisted secret ----------------------------------------------
    #[test]
    fn commercial_section_serializes_exactly_the_documented_keys() {
        let mut expected: Vec<String> =
            COMMERCIAL_CONFIG_KEYS.iter().map(|k| k.to_string()).collect();
        expected.sort();

        // Both the disabled default and a fully populated profile must emit
        // exactly the frozen allowlist, so a new secret-bearing field cannot
        // slip into the persisted section unnoticed.
        assert_eq!(section_keys(&CommercialConfig::default()), expected);
        assert_eq!(section_keys(&enabled_profile()), expected);

        for key in COMMERCIAL_CONFIG_KEYS {
            let lower = key.to_ascii_lowercase();
            for forbidden in ["secret", "password", "token", "credential", "api_key"] {
                assert!(
                    !lower.contains(forbidden),
                    "documented commercial key {:?} looks secret-bearing",
                    key
                );
            }
        }
    }
}
