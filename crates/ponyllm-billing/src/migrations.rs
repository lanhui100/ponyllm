//! Forward-only commercial migrations: discovery, ordering and fail-closed
//! planning.
//!
//! Migrations live in `migrations/commercial/*.sql` at the workspace root. Each
//! file is named `<version>_<slug>.sql`; files are applied in filename order
//! and the embedded version must be strictly increasing. There are no down
//! migrations: rollback disables commercial ingress and reverts code, it never
//! rewrites applied schema.
//!
//! [`Migrations::plan`] is a pure function over the files on disk and the rows
//! already recorded in `commercial_schema_migrations`, so the fail-closed rules
//! are unit-testable without a database.

use std::path::{Path, PathBuf};

use crate::checksum::checksum;
use crate::error::BillingError;

/// Directory (relative to the workspace root) that holds the SQL files.
pub const MIGRATIONS_DIR_RELATIVE: &str = "migrations/commercial";

/// Optional environment override for the migration directory.
pub const MIGRATIONS_DIR_ENV: &str = "PONYLLM_COMMERCIAL_MIGRATIONS_DIR";

/// A single migration file loaded from disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationFile {
    version: i64,
    filename: String,
    sql: String,
    checksum: String,
}

impl MigrationFile {
    /// Monotonic version parsed from the filename prefix.
    pub fn version(&self) -> i64 {
        self.version
    }

    /// Bare filename, e.g. `0003_ledger.sql`.
    pub fn filename(&self) -> &str {
        &self.filename
    }

    /// Raw SQL body.
    pub fn sql(&self) -> &str {
        &self.sql
    }

    /// Lowercase hexadecimal SHA-256 of the raw file bytes.
    pub fn checksum(&self) -> &str {
        &self.checksum
    }
}

/// A migration version recorded as applied on a target database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedMigration {
    /// Applied version.
    pub version: i64,
    /// Filename recorded at apply time.
    pub filename: String,
    /// Checksum recorded at apply time.
    pub checksum: String,
    /// Database-clock application time.
    pub applied_at: chrono::DateTime<chrono::Utc>,
}

/// The ordered set of migration files present on disk.
#[derive(Debug, Clone)]
pub struct Migrations {
    files: Vec<MigrationFile>,
}

impl Migrations {
    /// Load the repository migrations, honouring [`MIGRATIONS_DIR_ENV`].
    pub fn discover() -> Result<Self, BillingError> {
        let dir = match std::env::var(MIGRATIONS_DIR_ENV) {
            Ok(value) if !value.trim().is_empty() => PathBuf::from(value),
            _ => Self::default_dir(),
        };
        Self::load_from_dir(dir)
    }

    /// The compile-time workspace path to the migrations directory.
    pub fn default_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join(MIGRATIONS_DIR_RELATIVE)
    }

    /// Load `*.sql` from `dir` in filename order, validating version prefixes.
    pub fn load_from_dir(dir: impl AsRef<Path>) -> Result<Self, BillingError> {
        let dir = dir.as_ref();
        let entries = std::fs::read_dir(dir).map_err(|source| BillingError::MigrationDirUnreadable {
            path: dir.display().to_string(),
            source,
        })?;

        let mut names: Vec<String> = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| BillingError::MigrationDirUnreadable {
                path: dir.display().to_string(),
                source,
            })?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("sql") {
                continue;
            }
            if !path.is_file() {
                continue;
            }
            match path.file_name().and_then(|name| name.to_str()) {
                Some(name) => names.push(name.to_string()),
                None => {
                    return Err(BillingError::MigrationFileInvalid {
                        file: path.display().to_string(),
                        reason: "filename is not valid UTF-8".to_string(),
                    })
                }
            }
        }
        names.sort();

        let mut files = Vec::with_capacity(names.len());
        for name in names {
            let path = dir.join(&name);
            let bytes = std::fs::read(&path).map_err(|source| {
                BillingError::MigrationDirUnreadable {
                    path: path.display().to_string(),
                    source,
                }
            })?;
            let sql = String::from_utf8(bytes.clone()).map_err(|_| {
                BillingError::MigrationFileInvalid {
                    file: name.clone(),
                    reason: "file is not valid UTF-8".to_string(),
                }
            })?;
            let version = parse_version(&name)?;
            files.push(MigrationFile {
                version,
                filename: name,
                sql,
                checksum: checksum(&bytes),
            });
        }

        for pair in files.windows(2) {
            if pair[0].version >= pair[1].version {
                return Err(BillingError::MigrationOrderInvalid {
                    detail: format!(
                        "{} (v{}) must sort before {} (v{}); versions must be unique and increasing in filename order",
                        pair[0].filename, pair[0].version, pair[1].filename, pair[1].version
                    ),
                });
            }
        }

        if files.is_empty() {
            return Err(BillingError::MigrationDirUnreadable {
                path: dir.display().to_string(),
                source: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no *.sql migrations found",
                ),
            });
        }

        Ok(Self { files })
    }

    /// All files in apply order.
    pub fn files(&self) -> &[MigrationFile] {
        &self.files
    }

    /// All versions in apply order.
    pub fn versions(&self) -> Vec<i64> {
        self.files.iter().map(|file| file.version).collect()
    }

    /// Number of migration files on disk.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// True when no migration files are present (never returned by
    /// [`Migrations::load_from_dir`], which fails closed).
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Compute the migrations that still need to run, failing closed when the
    /// recorded state disagrees with the files on disk.
    ///
    /// Fail-closed rules:
    /// - a recorded version with no file on disk is [`BillingError::MissingVersion`];
    /// - a recorded checksum that differs from the file is
    ///   [`BillingError::ChecksumMismatch`];
    /// - recorded versions must be a prefix of the file order, otherwise
    ///   [`BillingError::MigrationOrderInvalid`];
    /// - a requested `target_version` must exist on disk, otherwise
    ///   [`BillingError::MissingVersion`].
    pub fn plan(
        &self,
        applied: &[AppliedMigration],
        target_version: Option<i64>,
    ) -> Result<Vec<MigrationFile>, BillingError> {
        let mut recorded: Vec<&AppliedMigration> = applied.iter().collect();
        recorded.sort_by_key(|entry| entry.version);
        for pair in recorded.windows(2) {
            if pair[0].version == pair[1].version {
                return Err(BillingError::MigrationOrderInvalid {
                    detail: format!("migration version {} is recorded twice", pair[0].version),
                });
            }
        }

        // Every recorded version must exist on disk.
        for entry in &recorded {
            if !self.files.iter().any(|file| file.version == entry.version) {
                return Err(BillingError::MissingVersion {
                    version: entry.version,
                });
            }
        }

        // Recorded versions must be exactly the leading prefix of the files.
        if recorded.len() > self.files.len() {
            let extra = recorded[self.files.len()];
            return Err(BillingError::MigrationOrderInvalid {
                detail: format!(
                    "version {} is recorded but the file order has only {} migrations",
                    extra.version,
                    self.files.len()
                ),
            });
        }
        for (index, entry) in recorded.iter().enumerate() {
            let file = &self.files[index];
            if file.version != entry.version {
                return Err(BillingError::MigrationOrderInvalid {
                    detail: format!(
                        "recorded version {} is out of order; expected version {} at position {}",
                        entry.version,
                        file.version,
                        index + 1
                    ),
                });
            }
            if file.checksum != entry.checksum {
                return Err(BillingError::ChecksumMismatch {
                    version: entry.version,
                    recorded: entry.checksum.clone(),
                    actual: file.checksum.clone(),
                });
            }
        }

        let start = recorded.len();
        match target_version {
            None => Ok(self.files[start..].to_vec()),
            Some(target) => {
                let index = self
                    .files
                    .iter()
                    .position(|file| file.version == target)
                    .ok_or(BillingError::MissingVersion { version: target })?;
                if index < start {
                    return Ok(Vec::new());
                }
                Ok(self.files[start..=index].to_vec())
            }
        }
    }

    /// Ensure a requested version exists, returning
    /// [`BillingError::MissingVersion`] otherwise.
    pub fn require_version(&self, version: i64) -> Result<&MigrationFile, BillingError> {
        self.files
            .iter()
            .find(|file| file.version == version)
            .ok_or(BillingError::MissingVersion { version })
    }
}

fn parse_version(filename: &str) -> Result<i64, BillingError> {
    let prefix: String = filename
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    if prefix.is_empty() {
        return Err(BillingError::MigrationFileInvalid {
            file: filename.to_string(),
            reason: "filename must start with a numeric version prefix".to_string(),
        });
    }
    let version = prefix
        .parse::<i64>()
        .map_err(|_| BillingError::MigrationFileInvalid {
            file: filename.to_string(),
            reason: "version prefix does not fit in i64".to_string(),
        })?;
    if version <= 0 {
        return Err(BillingError::MigrationFileInvalid {
            file: filename.to_string(),
            reason: "version prefix must be a positive integer".to_string(),
        });
    }
    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let unique = format!(
                "ponyllm-billing-migrations-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            );
            let path = std::env::temp_dir().join(unique);
            std::fs::create_dir_all(&path).expect("create temp dir");
            TempDir(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn write(&self, name: &str, body: &str) {
            std::fs::write(self.0.join(name), body).expect("write migration");
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn applied(version: i64, checksum: &str) -> AppliedMigration {
        AppliedMigration {
            version,
            filename: format!("{version:04}_x.sql"),
            checksum: checksum.to_string(),
            applied_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn loads_files_in_filename_order_regardless_of_creation_order() {
        let dir = TempDir::new();
        dir.write("0003_ledger.sql", "SELECT 3;");
        dir.write("0001_roles.sql", "SELECT 1;");
        dir.write("0002_tenants.sql", "SELECT 2;");
        dir.write("notes.txt", "ignored");
        let migrations = Migrations::load_from_dir(dir.path()).expect("load");
        assert_eq!(migrations.versions(), vec![1, 2, 3]);
        assert_eq!(
            migrations.files()[0].filename(),
            "0001_roles.sql"
        );
        assert_eq!(migrations.files()[2].sql(), "SELECT 3;");
        assert_eq!(migrations.files()[0].checksum().len(), 64);
    }

    #[test]
    fn rejects_duplicate_or_out_of_order_versions() {
        let dir = TempDir::new();
        dir.write("0002_a.sql", "SELECT 1;");
        dir.write("0002_b.sql", "SELECT 2;");
        let err = Migrations::load_from_dir(dir.path()).unwrap_err();
        assert!(
            matches!(err, BillingError::MigrationOrderInvalid { .. }),
            "{err}"
        );

        // Unpadded prefixes sort lexicographically as 10 before 2, which the
        // loader rejects instead of silently applying 10 first.
        let dir = TempDir::new();
        dir.write("10_ten.sql", "SELECT 1;");
        dir.write("2_two.sql", "SELECT 2;");
        let err = Migrations::load_from_dir(dir.path()).unwrap_err();
        assert!(
            matches!(err, BillingError::MigrationOrderInvalid { .. }),
            "{err}"
        );
    }

    #[test]
    fn rejects_non_numeric_prefix_and_empty_directory() {
        let dir = TempDir::new();
        dir.write("roles.sql", "SELECT 1;");
        let err = Migrations::load_from_dir(dir.path()).unwrap_err();
        assert!(
            matches!(err, BillingError::MigrationFileInvalid { .. }),
            "{err}"
        );

        let empty = TempDir::new();
        let err = Migrations::load_from_dir(empty.path()).unwrap_err();
        assert!(
            matches!(err, BillingError::MigrationDirUnreadable { .. }),
            "{err}"
        );
    }

    #[test]
    fn plan_returns_pending_suffix_and_fails_closed_on_checksum_drift() {
        let dir = TempDir::new();
        dir.write("0001_a.sql", "SELECT 1;");
        dir.write("0002_b.sql", "SELECT 2;");
        dir.write("0003_c.sql", "SELECT 3;");
        let migrations = Migrations::load_from_dir(dir.path()).expect("load");

        // Nothing applied: everything is pending, in order.
        let pending = migrations.plan(&[], None).expect("plan");
        assert_eq!(
            pending.iter().map(|file| file.version).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );

        // Version 1 applied with the correct checksum: 2 and 3 remain.
        let v1 = migrations.require_version(1).unwrap().clone();
        let pending = migrations
            .plan(&[applied(1, v1.checksum())], None)
            .expect("plan");
        assert_eq!(
            pending.iter().map(|file| file.version).collect::<Vec<_>>(),
            vec![2, 3]
        );

        // Version 1 applied with a different checksum: fail closed.
        let err = migrations
            .plan(&[applied(1, "deadbeef")], None)
            .unwrap_err();
        match err {
            BillingError::ChecksumMismatch {
                version,
                recorded,
                actual,
            } => {
                assert_eq!(version, 1);
                assert_eq!(recorded, "deadbeef");
                assert_eq!(actual, v1.checksum());
            }
            other => panic!("expected checksum mismatch, got {other}"),
        }

        // All applied: no pending work.
        let all: Vec<AppliedMigration> = migrations
            .files()
            .iter()
            .map(|file| applied(file.version(), file.checksum()))
            .collect();
        assert!(migrations.plan(&all, None).expect("plan").is_empty());
    }

    #[test]
    fn plan_fails_closed_on_missing_and_out_of_order_versions() {
        let dir = TempDir::new();
        dir.write("0001_a.sql", "SELECT 1;");
        dir.write("0002_b.sql", "SELECT 2;");
        dir.write("0003_c.sql", "SELECT 3;");
        let migrations = Migrations::load_from_dir(dir.path()).expect("load");

        // A recorded version that no longer exists on disk.
        let err = migrations.plan(&[applied(7, "abc")], None).unwrap_err();
        assert!(matches!(err, BillingError::MissingVersion { version: 7 }), "{err}");

        // A hole in the recorded set (2 without 1).
        let v2 = migrations.require_version(2).unwrap().clone();
        let err = migrations
            .plan(&[applied(2, v2.checksum())], None)
            .unwrap_err();
        assert!(
            matches!(err, BillingError::MigrationOrderInvalid { .. }),
            "{err}"
        );

        // A requested target that does not exist.
        let err = migrations.plan(&[], Some(99)).unwrap_err();
        assert!(matches!(err, BillingError::MissingVersion { version: 99 }), "{err}");

        // A requested target that is a legal prefix.
        let pending = migrations.plan(&[], Some(2)).expect("plan");
        assert_eq!(
            pending.iter().map(|file| file.version).collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn repository_migrations_are_present_ordered_and_well_formed() {
        let migrations = Migrations::discover().expect("discover repository migrations");
        assert_eq!(migrations.versions(), vec![1, 2, 3, 4, 5, 6]);
        for file in migrations.files() {
            assert!(!file.sql().trim().is_empty(), "{} is empty", file.filename());
            assert_eq!(file.checksum().len(), 64);
            assert!(file.filename().ends_with(".sql"));
        }
    }

    #[test]
    fn filesystem_error_is_reported_not_panicked() {
        let missing = std::env::temp_dir().join("ponyllm-billing-does-not-exist-xyz");
        let err = Migrations::load_from_dir(&missing).unwrap_err();
        assert!(
            matches!(err, BillingError::MigrationDirUnreadable { .. }),
            "{err}"
        );
    }
}
