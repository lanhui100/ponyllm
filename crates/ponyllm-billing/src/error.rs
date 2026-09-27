//! Fail-closed error type for the commercial persistence boundary.
//!
//! Every variant is safe to log: `redact_secrets` strips connection-string
//! credentials before a message leaves this module, and no variant carries a
//! raw database URL, tenant secret, or idempotency key value.

use std::fmt;

/// Errors returned by the commercial persistence boundary.
///
/// The variants are deliberately coarse so callers can map them to public
/// error codes without leaking internal state.
#[derive(Debug, thiserror::Error)]
pub enum BillingError {
    /// A required configuration variable is not present in the environment.
    #[error("commercial configuration variable {var} is not set")]
    ConfigMissing { var: &'static str },

    /// A configuration variable is present but unusable. The raw value is
    /// never included in the message.
    #[error("commercial configuration variable {var} is invalid: {reason} (value redacted)")]
    ConfigInvalid {
        var: &'static str,
        reason: &'static str,
    },

    /// The migration directory could not be read.
    #[error("commercial migration directory is unreadable: {path}")]
    MigrationDirUnreadable {
        path: String,
        #[source]
        source: std::io::Error,
    },

    /// A migration filename or body could not be interpreted.
    #[error("commercial migration file {file} is invalid: {reason}")]
    MigrationFileInvalid { file: String, reason: String },

    /// A migration that is already recorded on the target database no longer
    /// matches the file on disk. This is the fail-closed checksum gate.
    #[error(
        "migration {version} checksum mismatch: recorded {recorded}, file {actual}; refusing to continue (fail closed)"
    )]
    ChecksumMismatch {
        version: i64,
        recorded: String,
        actual: String,
    },

    /// A requested migration version has no corresponding file.
    #[error("requested migration version {version} is missing on disk")]
    MissingVersion { version: i64 },

    /// Migration files or recorded versions are not a well-formed ordered set.
    #[error("commercial migration order is invalid: {detail}")]
    MigrationOrderInvalid { detail: String },

    /// Checked `u128` arithmetic overflowed (including sub-underflow).
    #[error("amount arithmetic overflow")]
    AmountOverflow,

    /// A negative amount was supplied where only non-negative money is legal.
    #[error("amount must not be negative")]
    NegativeAmount,

    /// The amount was not a non-negative integer micro-USD value.
    #[error("amount is invalid: {reason}")]
    InvalidAmount { reason: &'static str },

    /// A database operation failed. Details are redacted.
    #[error("commercial database operation failed (details redacted): {detail}")]
    Database { detail: String },
}

/// Replace credential material in a free-form string with `<redacted>`.
///
/// This is intentionally conservative: authority sections of URLs and
/// `password=`/`pass=`/`secret=` assignments are stripped entirely. It is used
/// before any driver error text is surfaced.
pub fn redact_secrets(text: &str) -> String {
    let mut out = redact_url_authorities(text);
    for key in ["password=", "pass=", "secret=", "pwd="] {
        out = redact_assignment(&out, key);
    }
    out
}

fn redact_url_authorities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find("://") {
        let (head, tail) = rest.split_at(pos + 3);
        out.push_str(head);
        let end = tail
            .find(|c: char| c == '/' || c == '?' || c == '#' || c.is_whitespace())
            .unwrap_or(tail.len());
        let (authority, remainder) = tail.split_at(end);
        match authority.rfind('@') {
            Some(at) => {
                out.push_str("<redacted>");
                out.push_str(&authority[at..]);
            }
            None => out.push_str(authority),
        }
        rest = remainder;
    }
    out.push_str(rest);
    out
}

fn redact_assignment(text: &str, key: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    while let Some(rel) = lower[cursor..].find(key) {
        let start = cursor + rel;
        let value_start = start + key.len();
        if value_start > text.len() {
            break;
        }
        out.push_str(&text[cursor..value_start]);
        let value_end = text[value_start..]
            .find(|c: char| c.is_whitespace() || c == '&' || c == ';' || c == '"' || c == '\'')
            .map(|offset| value_start + offset)
            .unwrap_or(text.len());
        if value_end > value_start {
            out.push_str("<redacted>");
        }
        cursor = value_end;
    }
    out.push_str(&text[cursor..]);
    out
}

/// Map a driver error into a redacted [`BillingError`].
pub(crate) fn database_error(source: tokio_postgres::Error) -> BillingError {
    BillingError::Database {
        detail: describe_database_error(&source),
    }
}

/// Human-readable, credential-free description of a driver error.
///
/// `Error::to_string` only reports `db error`; the useful server text lives in
/// the attached `DbError`. This includes the severity, message, SQLSTATE,
/// constraint and table names, but deliberately excludes row `detail`/`hint`
/// payloads (which can echo data values), and everything is passed through
/// [`redact_secrets`].
pub fn describe_database_error(error: &tokio_postgres::Error) -> String {
    let Some(db) = error.as_db_error() else {
        return redact_secrets(&error.to_string());
    };
    let mut out = format!("{}: {} ({})", db.severity(), db.message(), db.code().code());
    if let Some(constraint) = db.constraint() {
        out.push_str(&format!(" [constraint {constraint}]"));
    }
    if let Some(table) = db.table() {
        out.push_str(&format!(" [table {table}]"));
    }
    redact_secrets(&out)
}

impl BillingError {
    /// Construct a redacted database error from driver text.
    pub fn database(detail: impl fmt::Display) -> Self {
        BillingError::Database {
            detail: redact_secrets(&detail.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_url_credentials() {
        let redacted = redact_secrets("postgres://user:s3cret@db.example:5432/ponyllm failed");
        assert!(!redacted.contains("s3cret"), "{redacted}");
        assert!(redacted.contains("<redacted>@db.example:5432/ponyllm"), "{redacted}");
    }

    #[test]
    fn redacts_password_assignments() {
        let redacted = redact_secrets("dsn password=hunter2 host=db");
        assert!(!redacted.contains("hunter2"), "{redacted}");
        assert!(redacted.contains("password=<redacted>"), "{redacted}");
    }

    #[test]
    fn keeps_plain_text_unchanged() {
        assert_eq!(redact_secrets("connection refused"), "connection refused");
    }

    #[test]
    fn error_messages_expose_server_text_without_credentials() {
        let err = BillingError::database("dsn postgres://u:pw@host/db refused");
        let rendered = err.to_string();
        assert!(rendered.contains("refused"), "{rendered}");
        assert!(!rendered.contains("pw@"), "{rendered}");
    }
}
