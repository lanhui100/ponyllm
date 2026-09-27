//! Database-only configuration for the commercial plane.
//!
//! Stage 1 hardening: the commercial store is configured exclusively from an
//! injected environment variable. The value is never written to a log, a
//! `Debug`/`Display` rendering, or an error message; only `redacted()` output
//! and the explicit [`CommercialDbConfig::database_url`] accessor (for opening
//! a connection) can expose it.

use std::fmt;

use crate::error::{redact_secrets, BillingError};

/// Environment variable that carries the commercial PostgreSQL URL.
pub const COMMERCIAL_DATABASE_URL_ENV: &str = "PONYLLM_COMMERCIAL_DATABASE_URL";

/// A validated, secret-bearing PostgreSQL connection target.
#[derive(Clone, PartialEq, Eq)]
pub struct CommercialDbConfig {
    url: String,
}

impl CommercialDbConfig {
    /// Read [`COMMERCIAL_DATABASE_URL_ENV`] and validate it.
    pub fn from_env() -> Result<Self, BillingError> {
        Self::from_env_var(COMMERCIAL_DATABASE_URL_ENV)
    }

    /// Read an explicitly named environment variable and validate it.
    pub fn from_env_var(var: &'static str) -> Result<Self, BillingError> {
        match std::env::var(var) {
            Ok(raw) => Self::parse(var, &raw),
            Err(std::env::VarError::NotPresent) => Err(BillingError::ConfigMissing { var }),
            Err(std::env::VarError::NotUnicode(_)) => Err(BillingError::ConfigInvalid {
                var,
                reason: "value is not valid UTF-8",
            }),
        }
    }

    /// Validate a raw value. The value is never echoed.
    pub fn parse(var: &'static str, raw: &str) -> Result<Self, BillingError> {
        let url = raw.trim();
        if url.is_empty() {
            return Err(BillingError::ConfigInvalid {
                var,
                reason: "value is empty",
            });
        }
        if url.eq_ignore_ascii_case("none")
            || url.eq_ignore_ascii_case("null")
            || url.eq_ignore_ascii_case("default")
        {
            return Err(BillingError::ConfigInvalid {
                var,
                reason: "placeholder credentials are rejected",
            });
        }
        if url.chars().any(char::is_whitespace) {
            return Err(BillingError::ConfigInvalid {
                var,
                reason: "value contains whitespace",
            });
        }
        let lower = url.to_ascii_lowercase();
        let scheme_end = if lower.starts_with("postgres://") {
            "postgres://".len()
        } else if lower.starts_with("postgresql://") {
            "postgresql://".len()
        } else {
            return Err(BillingError::ConfigInvalid {
                var,
                reason: "scheme must be postgres:// or postgresql://",
            });
        };

        let authority_and_path = &url[scheme_end..];
        if authority_and_path.is_empty() || authority_and_path.starts_with('/') {
            return Err(BillingError::ConfigInvalid {
                var,
                reason: "connection URL has no host",
            });
        }
        let authority = authority_and_path
            .split(['/', '?'])
            .next()
            .unwrap_or_default();
        if authority.is_empty() {
            return Err(BillingError::ConfigInvalid {
                var,
                reason: "connection URL has no host",
            });
        }
        if authority.starts_with('@') || authority.ends_with('@') {
            return Err(BillingError::ConfigInvalid {
                var,
                reason: "connection URL authority is malformed",
            });
        }
        let host_port = authority.rsplit('@').next().unwrap_or(authority);
        if host_port.starts_with(':') {
            return Err(BillingError::ConfigInvalid {
                var,
                reason: "connection URL has no host",
            });
        }
        if let Some((_, port)) = host_port.rsplit_once(':') {
            if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) {
                match port.parse::<u32>() {
                    Ok(value) if (1..=65535).contains(&value) => {}
                    _ => {
                        return Err(BillingError::ConfigInvalid {
                            var,
                            reason: "connection URL port is out of range",
                        })
                    }
                }
            }
        }

        Ok(Self {
            url: url.to_string(),
        })
    }

    /// The raw connection URL, for handing to the PostgreSQL driver.
    ///
    /// Callers must never log or persist the return value.
    pub fn database_url(&self) -> &str {
        &self.url
    }

    /// The connection target with credentials masked.
    pub fn redacted(&self) -> String {
        redact_secrets(&self.url)
    }
}

impl fmt::Debug for CommercialDbConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommercialDbConfig")
            .field("database_url", &self.redacted())
            .finish()
    }
}

impl fmt::Display for CommercialDbConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.redacted())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VAR: &str = "PONYLLM_COMMERCIAL_DATABASE_URL";

    #[test]
    fn accepts_postgres_and_postgresql_schemes() {
        assert!(CommercialDbConfig::parse(VAR, "postgres://u:p@localhost:5432/db").is_ok());
        assert!(CommercialDbConfig::parse(VAR, "postgresql://u@db.internal/db").is_ok());
        assert!(CommercialDbConfig::parse(VAR, "  postgres://u@localhost/db  ").is_ok());
    }

    #[test]
    fn rejects_missing_and_placeholder_values() {
        for bad in ["", "   ", "none", "NONE", "null", "default"] {
            assert!(
                CommercialDbConfig::parse(VAR, bad).is_err(),
                "accepted {bad:?}"
            );
        }
    }

    #[test]
    fn rejects_wrong_scheme_and_missing_host() {
        for bad in [
            "mysql://u:p@localhost/db",
            "https://localhost/db",
            "postgres:///db",
            "postgres://",
            "postgres://u:p@localhost:99999/db",
            "postgres://u:p@:5432/db",
            "postgres://host/db with space",
        ] {
            assert!(
                CommercialDbConfig::parse(VAR, bad).is_err(),
                "accepted {bad:?}"
            );
        }
    }

    #[test]
    fn error_messages_never_contain_the_value() {
        let secret = "postgres://operator:sup3r-s3cret@localhost:99999/db";
        let err = CommercialDbConfig::parse(VAR, secret).unwrap_err();
        let rendered = err.to_string();
        assert!(!rendered.contains("sup3r-s3cret"), "{rendered}");
        assert!(rendered.contains("value redacted"), "{rendered}");
    }

    #[test]
    fn debug_and_display_are_redacted() {
        let config =
            CommercialDbConfig::parse(VAR, "postgres://u:topsecret@localhost:5432/db").unwrap();
        let debug = format!("{config:?}");
        let display = config.to_string();
        assert!(!debug.contains("topsecret"), "{debug}");
        assert!(!display.contains("topsecret"), "{display}");
        assert!(debug.contains("<redacted>@localhost:5432/db"), "{debug}");
    }

    #[test]
    fn from_env_reads_only_the_named_variable() {
        // Serialized by the crate's single env-touching test; see also
        // `from_env_reports_missing` in the migrations test module.
        std::env::set_var(COMMERCIAL_DATABASE_URL_ENV, "postgres://u:p@127.0.0.1:5432/db");
        let config = CommercialDbConfig::from_env().expect("env present");
        assert_eq!(config.database_url(), "postgres://u:p@127.0.0.1:5432/db");
        std::env::remove_var(COMMERCIAL_DATABASE_URL_ENV);
        assert!(matches!(
            CommercialDbConfig::from_env(),
            Err(BillingError::ConfigMissing { var }) if var == COMMERCIAL_DATABASE_URL_ENV
        ));
    }
}
