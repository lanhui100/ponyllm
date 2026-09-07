//! Admin-facing configuration persistence boundary (WEB-03).
//!
//! The server never touches the filesystem directly: the CLI (or any embedder)
//! injects a `ConfigStore` implementation at construction time. The SDK path
//! leaves it `None` — admin write endpoints then answer
//! `503 admin_store_unavailable` instead of breaking embedded builds.

use ponyllm_config::ConfigFile;

pub trait ConfigStore: Send + Sync {
    /// Load the current on-disk configuration.
    fn load(&self) -> std::io::Result<ConfigFile>;
    /// Persist the configuration atomically (temp file + rename semantics).
    fn save(&self, config: &ConfigFile) -> std::io::Result<()>;
}

/// Filesystem-backed store: resolves the config path once at construction.
pub struct FileConfigStore {
    path: String,
}

impl FileConfigStore {
    pub fn new(path: impl Into<String>) -> Self {
        Self { path: path.into() }
    }
}

impl ConfigStore for FileConfigStore {
    fn load(&self) -> std::io::Result<ConfigFile> {
        let content = std::fs::read_to_string(&self.path)?;
        toml::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    fn save(&self, config: &ConfigFile) -> std::io::Result<()> {
        config.save_to_path(&self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_store_roundtrip_preserves_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ponyllm.toml");
        std::fs::write(&path, ponyllm_config::generate_sample_config()).unwrap();

        let store = FileConfigStore::new(path.to_str().unwrap());
        let loaded = store.load().unwrap();
        assert!(loaded.gateway.web_enabled);
        assert_eq!(loaded.gateway.bind, "127.0.0.1:8080");
        let mut modified = loaded.clone();
        modified.gateway.default_strategy = ponyllm_core::pool::GatewayRoutingStrategy::Speed;
        store.save(&modified).unwrap();

        let reloaded = store.load().unwrap();
        assert_eq!(
            reloaded.gateway.default_strategy,
            ponyllm_core::pool::GatewayRoutingStrategy::Speed
        );
    }

    #[test]
    fn file_store_load_missing_file_is_io_error() {
        let store = FileConfigStore::new("Z:/definitely/missing/ponyllm.toml");
        assert!(store.load().is_err());
    }
}
