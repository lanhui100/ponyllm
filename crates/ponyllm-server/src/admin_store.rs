//! Admin-facing configuration persistence boundary (WEB-03 / multi-node HA).
//!
//! The server never touches the filesystem directly: the CLI (or any embedder)
//! injects a `ConfigStore` implementation at construction time. The SDK path
//! leaves it `None` — admin write endpoints then answer
//! `503 admin_store_unavailable` instead of breaking embedded builds.
//!
//! Multi-node HA (2026-09-28): the trait is `async` (kept dyn-compatible via
//! `#[async_trait]`) and carries an explicit version token so the Kubernetes
//! backend can implement optimistic concurrency on the Secret's
//! `resourceVersion` and map a 409 conflict back to the HTTP If-Match 412
//! semantics at the admin layer.

use std::collections::BTreeMap;

use async_trait::async_trait;
use ponyllm_config::ConfigFile;

/// Version carrier for optimistic concurrency across backends.
///
/// - [`ConfigVersion::File`] wraps the on-disk `config_version` (single-process
///   writes serialize on `admin_write_lock`, so the file backend is best-effort).
/// - [`ConfigVersion::Kubernetes`] wraps the Secret `resourceVersion` used as
///   the CAS precondition on every write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigVersion {
    File(u64),
    Kubernetes(String),
}

impl ConfigVersion {
    /// Opaque wire representation persisted alongside a loaded config; the
    /// backend that produced it is the only consumer that must understand it.
    pub fn as_string(&self) -> String {
        match self {
            ConfigVersion::File(v) => v.to_string(),
            ConfigVersion::Kubernetes(rv) => rv.clone(),
        }
    }
}

/// Store-level error model. `Conflict` is the optimistic-concurrency signal
/// (map to HTTP 412 at the admin layer); everything else is a real backend
/// failure (map to 500).
#[derive(Debug)]
pub enum ConfigStoreError {
    Io(std::io::Error),
    InvalidData(String),
    Conflict {
        expected: Option<ConfigVersion>,
        current: Option<ConfigVersion>,
    },
}

impl std::fmt::Display for ConfigStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigStoreError::Io(e) => write!(f, "config store io error: {}", e),
            ConfigStoreError::InvalidData(m) => write!(f, "config store invalid data: {}", m),
            ConfigStoreError::Conflict { expected, current } => write!(
                f,
                "config store version conflict (expected {:?}, current {:?})",
                expected.as_ref().map(|v| v.as_string()),
                current.as_ref().map(|v| v.as_string()),
            ),
        }
    }
}

impl std::error::Error for ConfigStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigStoreError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for ConfigStoreError {
    fn from(e: std::io::Error) -> Self {
        ConfigStoreError::Io(e)
    }
}

pub type StoreResult<T> = Result<T, ConfigStoreError>;

/// Configuration persistence boundary. `load` returns the parsed config and
/// the version token that `save` must present unchanged for the write to
/// proceed (optimistic concurrency).
#[async_trait]
pub trait ConfigStore: Send + Sync {
    /// Load the current configuration together with its version token.
    async fn load(&self) -> StoreResult<(ConfigFile, ConfigVersion)>;
    /// Persist the configuration atomically, guarded by `version` (the token
    /// returned by the most recent [`ConfigStore::load`]).
    async fn save(&self, config: &ConfigFile, version: &ConfigVersion) -> StoreResult<()>;
}

/// Filesystem-backed store: resolves the config path once at construction.
/// The version token is the on-disk `config_version` (informational; the file
/// backend has no cross-process CAS — single-instance deployments serialize
/// writes on `admin_write_lock`).
pub struct FileConfigStore {
    path: String,
}

impl FileConfigStore {
    pub fn new(path: impl Into<String>) -> Self {
        Self { path: path.into() }
    }
}

#[async_trait]
impl ConfigStore for FileConfigStore {
    async fn load(&self) -> StoreResult<(ConfigFile, ConfigVersion)> {
        let content = std::fs::read_to_string(&self.path)?;
        let config = toml::from_str::<ConfigFile>(&content)
            .map_err(|e| ConfigStoreError::InvalidData(e.to_string()))?;
        let version = ConfigVersion::File(config.config_version);
        Ok((config, version))
    }

    async fn save(&self, config: &ConfigFile, _version: &ConfigVersion) -> StoreResult<()> {
        config.save_to_path(&self.path)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Kubernetes Secret backed store
// ---------------------------------------------------------------------------

/// Raw view of a Secret that the store needs. Deliberately decoupled from
/// k8s-openapi types so tests can inject an in-memory fake without touching
/// the Kubernetes resource model.
#[derive(Debug, Clone, Default)]
pub struct SecretSnapshot {
    /// `metadata.resourceVersion` (the optimistic-concurrency token).
    pub resource_version: Option<String>,
    /// Base64-decoded raw bytes per data key.
    pub data: BTreeMap<String, Vec<u8>>,
}

/// Minimal Kubernetes Secret read/patch surface (GET + JSON Merge PATCH with a
/// resourceVersion precondition). Implemented by [`KubeSecretApi`] in
/// production; tests use in-memory fakes and wiremock-backed servers.
#[async_trait]
pub trait SecretApi: Send + Sync {
    /// GET the named Secret in the configured namespace.
    async fn get(&self, name: &str) -> StoreResult<SecretSnapshot>;
    /// JSON Merge PATCH: replace (a submap of) `data` and carry
    /// `metadata.resourceVersion` so the API server rejects stale writes with
    /// a 409 Conflict, mapped to [`ConfigStoreError::Conflict`].
    async fn patch_data(
        &self,
        name: &str,
        resource_version: &str,
        data: &BTreeMap<String, Vec<u8>>,
    ) -> StoreResult<()>;
    /// Namespace for the target resource.
    fn namespace(&self) -> &str;
}

/// Kubernetes Secrets are base64-encoded on the wire (RFC 4648 std alphabet,
/// with padding). Production read/write goes through k8s-openapi's
/// `ByteString`, which handles base64 internally; the helpers below exist for
/// tests to assert the wire format.
#[cfg(test)]
fn base64_encode(raw: &[u8]) -> String {
    use base64::Engine;
    const BASE64_STANDARD: base64::engine::GeneralPurpose =
        base64::engine::GeneralPurpose::new(&base64::alphabet::STANDARD, base64::engine::general_purpose::PAD);
    BASE64_STANDARD.encode(raw)
}

#[cfg(test)]
fn base64_decode(encoded: &str) -> StoreResult<Vec<u8>> {
    use base64::Engine;
    const BASE64_STANDARD: base64::engine::GeneralPurpose =
        base64::engine::GeneralPurpose::new(&base64::alphabet::STANDARD, base64::engine::general_purpose::PAD);
    BASE64_STANDARD
        .decode(encoded)
        .map_err(|e| ConfigStoreError::InvalidData(format!("invalid base64 in Secret data: {}", e)))
}

/// Production [`SecretApi`] backed by kube-rs against the real API server.
#[derive(Clone)]
pub struct KubeSecretApi {
    client: kube::Client,
    namespace: String,
}

impl KubeSecretApi {
    pub fn new(client: kube::Client, namespace: impl Into<String>) -> Self {
        Self {
            client,
            namespace: namespace.into(),
        }
    }

    fn api(&self) -> kube::Api<k8s_openapi::api::core::v1::Secret> {
        kube::Api::namespaced(self.client.clone(), self.namespace.as_str())
    }
}

fn map_kube_err(e: kube::Error, expected_rv: Option<&str>) -> ConfigStoreError {
    match e {
        kube::Error::Api(ref api) if api.code == 409 => ConfigStoreError::Conflict {
            expected: expected_rv.map(|rv| ConfigVersion::Kubernetes(rv.to_string())),
            current: None,
        },
        kube::Error::Api(ref api) if api.code == 404 => {
            ConfigStoreError::InvalidData(format!("Secret not found: {}", api.message))
        }
        other => ConfigStoreError::Io(std::io::Error::other(other.to_string())),
    }
}

#[async_trait]
impl SecretApi for KubeSecretApi {
    async fn get(&self, name: &str) -> StoreResult<SecretSnapshot> {
        let secret = self
            .api()
            .get(name)
            .await
            .map_err(|e| map_kube_err(e, None))?;
        let mut data = BTreeMap::new();
        if let Some(map) = secret.data {
            for (k, v) in map {
                // k8s-openapi ByteString holds the base64-DECODED raw bytes.
                data.insert(k, v.0);
            }
        }
        Ok(SecretSnapshot {
            resource_version: secret.metadata.resource_version,
            data,
        })
    }

    async fn patch_data(
        &self,
        name: &str,
        resource_version: &str,
        data: &BTreeMap<String, Vec<u8>>,
    ) -> StoreResult<()> {
        use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
        use k8s_openapi::ByteString;

        let data_map: BTreeMap<String, ByteString> = data
            .iter()
            .map(|(k, v)| (k.clone(), ByteString(v.clone())))
            .collect();
        let patch = k8s_openapi::api::core::v1::Secret {
            metadata: ObjectMeta {
                resource_version: Some(resource_version.to_string()),
                ..Default::default()
            },
            data: Some(data_map),
            ..Default::default()
        };
        let pp = kube::api::PatchParams::default();
        let merge = kube::api::Patch::Merge(&patch);
        self.api()
            .patch(name, &pp, &merge)
            .await
            .map(|_| ())
            .map_err(|e| map_kube_err(e, Some(resource_version)))
    }

    fn namespace(&self) -> &str {
        &self.namespace
    }
}

/// Kubernetes Secret backed store: the configuration truth source for
/// multi-node deployments. Reads `data['ponyllm.toml']` from the Secret named
/// by the operator (default `ponyllm-live-config`); writes patch that single
/// data key under the `resourceVersion` optimistic-lock precondition.
pub struct KubernetesConfigStore {
    api: std::sync::Arc<dyn SecretApi>,
    secret_name: String,
    data_key: String,
}

impl KubernetesConfigStore {
    /// Point the store at an arbitrary [`SecretApi`] (production kube client
    /// or test fake).
    pub fn with_api(
        api: std::sync::Arc<dyn SecretApi>,
        secret_name: impl Into<String>,
        data_key: impl Into<String>,
    ) -> Self {
        Self {
            api,
            secret_name: secret_name.into(),
            data_key: data_key.into(),
        }
    }

    /// Build the store from the ambient kube environment (in-cluster service
    /// account, or local kubeconfig for dev). Secret: `ponyllm-live-config`,
    /// data key: `ponyllm.toml`.
    pub async fn from_env(secret_name: &str) -> StoreResult<Self> {
        let config = kube::Config::infer()
            .await
            .map_err(|e| ConfigStoreError::Io(std::io::Error::other(format!("kube config: {}", e))))?;
        let client = kube::Client::try_from(config)
            .map_err(|e| ConfigStoreError::Io(std::io::Error::other(format!("kube client: {}", e))))?;
        let namespace = std::env::var("PONYLLM_NAMESPACE")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| {
                // In-cluster default; harmless when running with a local kubeconfig.
                "ponyllm".to_string()
            });
        Ok(Self::with_api(
            std::sync::Arc::new(KubeSecretApi::new(client, namespace)),
            secret_name,
            "ponyllm.toml",
        ))
    }

    pub fn data_key(&self) -> &str {
        &self.data_key
    }

    pub fn secret_name(&self) -> &str {
        &self.secret_name
    }
}

#[async_trait]
impl ConfigStore for KubernetesConfigStore {
    async fn load(&self) -> StoreResult<(ConfigFile, ConfigVersion)> {
        let snap = self.api.get(&self.secret_name).await?;
        let raw = snap
            .data
            .get(self.data_key.as_str())
            .ok_or_else(|| {
                ConfigStoreError::InvalidData(format!(
                    "Secret '{}' has no data key '{}'",
                    self.secret_name, self.data_key
                ))
            })?
            .clone();
        let content = String::from_utf8(raw)
            .map_err(|e| ConfigStoreError::InvalidData(format!("config not UTF-8: {}", e)))?;
        let config = toml::from_str::<ConfigFile>(&content)
            .map_err(|e| ConfigStoreError::InvalidData(e.to_string()))?;
        let version = ConfigVersion::Kubernetes(snap.resource_version.unwrap_or_default());
        Ok((config, version))
    }

    async fn save(&self, config: &ConfigFile, version: &ConfigVersion) -> StoreResult<()> {
        let resource_version = match version {
            ConfigVersion::Kubernetes(rv) => rv.clone(),
            ConfigVersion::File(_) => {
                return Err(ConfigStoreError::InvalidData(
                    "kubernetes config store requires a Kubernetes(resourceVersion) version token"
                        .to_string(),
                ))
            }
        };
        let content = toml::to_string_pretty(config)
            .map_err(|e| ConfigStoreError::InvalidData(format!("config serialization: {}", e)))?;
        let mut data = BTreeMap::new();
        data.insert(self.data_key.clone(), content.into_bytes());
        self.api
            .patch_data(&self.secret_name, &resource_version, &data)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ponyllm_core::pool::GatewayRoutingStrategy;

    fn sample_config() -> ConfigFile {
        toml::from_str(ponyllm_config::generate_sample_config()).unwrap()
    }

    #[tokio::test]
    async fn file_store_roundtrip_preserves_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ponyllm.toml");
        std::fs::write(&path, ponyllm_config::generate_sample_config()).unwrap();

        let store = FileConfigStore::new(path.to_str().unwrap());
        let (loaded, version) = store.load().await.unwrap();
        assert!(loaded.gateway.web_enabled);
        assert_eq!(loaded.gateway.bind, "127.0.0.1:8080");
        assert_eq!(version, ConfigVersion::File(loaded.config_version));
        let mut modified = loaded.clone();
        modified.gateway.default_strategy = GatewayRoutingStrategy::Speed;
        store.save(&modified, &version).await.unwrap();

        let (reloaded, _) = store.load().await.unwrap();
        assert_eq!(
            reloaded.gateway.default_strategy,
            GatewayRoutingStrategy::Speed
        );
    }

    #[tokio::test]
    async fn file_store_load_missing_file_is_io_error() {
        let store = FileConfigStore::new("Z:/definitely/missing/ponyllm.toml");
        let err = store.load().await.unwrap_err();
        assert!(matches!(err, ConfigStoreError::Io(_)), "got {:?}", err);
    }

    #[tokio::test]
    async fn file_store_ignores_version_token_for_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ponyllm.toml");
        std::fs::write(&path, ponyllm_config::generate_sample_config()).unwrap();
        let store = FileConfigStore::new(path.to_str().unwrap());
        let (cfg, _) = store.load().await.unwrap();
        // A stale token must not block the file backend (single-instance CAS).
        store
            .save(&cfg, &ConfigVersion::File(cfg.config_version.saturating_sub(1)))
            .await
            .unwrap();
    }

    // ---------------------------------------------------------------------
    // In-memory fake SecretApi + KubernetesConfigStore trait-level tests
    // ---------------------------------------------------------------------

    #[derive(Default)]
    struct FakeSecretApi {
        secret: std::sync::Mutex<FakeSecretState>,
    }

    #[derive(Default)]
    struct FakeSecretState {
        rv: u64,
        data: BTreeMap<String, Vec<u8>>,
        /// When set, every patch is rejected with a conflict (simulates an
        /// external writer bumping the resourceVersion mid-flight).
        force_conflict: bool,
        /// Payloads captured from the last patch (for assertions).
        last_patch: Option<(String, BTreeMap<String, Vec<u8>>)>,
    }

    impl FakeSecretApi {
        fn seed_toml(&self, cfg: &ConfigFile) {
            let content = toml::to_string_pretty(cfg).unwrap();
            let mut guard = self.secret.lock().unwrap();
            guard.rv = 100;
            guard.data.insert("ponyllm.toml".to_string(), content.into_bytes());
        }

        fn version_bytes(&self) -> SecretSnapshot {
            let guard = self.secret.lock().unwrap();
            SecretSnapshot {
                resource_version: Some(guard.rv.to_string()),
                data: guard.data.clone(),
            }
        }
    }

    #[async_trait]
    impl SecretApi for FakeSecretApi {
        async fn get(&self, _name: &str) -> StoreResult<SecretSnapshot> {
            Ok(self.version_bytes())
        }

        async fn patch_data(
            &self,
            _name: &str,
            resource_version: &str,
            data: &BTreeMap<String, Vec<u8>>,
        ) -> StoreResult<()> {
            let mut guard = self.secret.lock().unwrap();
            if guard.force_conflict || guard.rv.to_string() != resource_version {
                return Err(ConfigStoreError::Conflict {
                    expected: Some(ConfigVersion::Kubernetes(resource_version.to_string())),
                    current: Some(ConfigVersion::Kubernetes(guard.rv.to_string())),
                });
            }
            guard.data.extend(data.clone());
            guard.last_patch = Some((resource_version.to_string(), data.clone()));
            guard.rv += 1;
            Ok(())
        }

        fn namespace(&self) -> &str {
            "ponyllm"
        }
    }

    fn kube_store(fake: std::sync::Arc<FakeSecretApi>) -> KubernetesConfigStore {
        KubernetesConfigStore::with_api(fake, "ponyllm-live-config", "ponyllm.toml")
    }

    #[tokio::test]
    async fn kube_store_roundtrip_and_version_carrier() {
        let fake = std::sync::Arc::new(FakeSecretApi::default());
        let cfg = sample_config();
        fake.seed_toml(&cfg);
        let store = kube_store(fake.clone());

        let (loaded, version) = store.load().await.unwrap();
        assert_eq!(version, ConfigVersion::Kubernetes("100".to_string()));
        assert!(loaded.gateway.web_enabled);

        let mut modified = loaded.clone();
        modified.gateway.default_strategy = GatewayRoutingStrategy::Speed;
        store.save(&modified, &version).await.unwrap();

        let (reloaded, version2) = store.load().await.unwrap();
        assert_eq!(
            reloaded.gateway.default_strategy,
            GatewayRoutingStrategy::Speed
        );
        assert_eq!(version2, ConfigVersion::Kubernetes("101".to_string()));
        assert_eq!(reloaded.config_version, modified.config_version);
    }

    #[tokio::test]
    async fn kube_store_stale_version_maps_to_conflict() {
        let fake = std::sync::Arc::new(FakeSecretApi::default());
        let cfg = sample_config();
        fake.seed_toml(&cfg);
        let store = kube_store(fake.clone());

        let (loaded, version) = store.load().await.unwrap();
        assert_eq!(version, ConfigVersion::Kubernetes("100".to_string()));

        // Another writer bumps the Secret behind our back.
        fake.secret.lock().unwrap().rv = 200;
        let mut modified = loaded.clone();
        modified.gateway.default_strategy = GatewayRoutingStrategy::Speed;
        let err = store.save(&modified, &version).await.unwrap_err();
        assert!(
            matches!(err, ConfigStoreError::Conflict { .. }),
            "expected Conflict, got {:?}",
            err
        );
    }

    #[tokio::test]
    async fn kube_store_forced_conflict_even_with_fresh_rv() {
        let fake = std::sync::Arc::new(FakeSecretApi::default());
        let cfg = sample_config();
        fake.seed_toml(&cfg);
        fake.secret.lock().unwrap().force_conflict = true;
        let store = kube_store(fake.clone());

        let (loaded, version) = store.load().await.unwrap();
        let err = store.save(&loaded, &version).await.unwrap_err();
        assert!(matches!(err, ConfigStoreError::Conflict { .. }));
    }

    #[tokio::test]
    async fn kube_store_missing_data_key_is_invalid_data() {
        let fake = std::sync::Arc::new(FakeSecretApi::default());
        // No seed: data map empty => missing 'ponyllm.toml' key.
        let store = kube_store(fake.clone());
        let err = store.load().await.unwrap_err();
        assert!(matches!(err, ConfigStoreError::InvalidData(_)), "got {:?}", err);
    }

    #[test]
    fn wire_base64_roundtrip_helpers() {
        let raw = b"hello ponyllm \xf0\x9f\x9a\x80";
        let encoded = base64_encode(raw);
        assert_eq!(base64_decode(&encoded).unwrap(), raw);
        // The wire format must be the standard padded encoding Kubernetes uses.
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
    }
}