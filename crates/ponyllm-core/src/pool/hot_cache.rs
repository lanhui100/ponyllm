use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::RwLock;
use std::time::{Duration, Instant};

const NUM_SHARDS: usize = 64;
const MIN_CACHE_PROMPT_LEN: usize = 1024;
const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(300); // 5 minutes
const MAX_ENTRIES_PER_SHARD: usize = 2048;

/// 24-byte compact prefix fingerprint
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CacheFingerprint {
    pub len: u32,
    pub head_hash: u64,
    pub tail_hash: u64,
    pub mid_hash: u64,
}

/// Helper to safely slice string on char boundaries
fn safe_char_slice(s: &str, start_byte: usize, max_len: usize) -> &str {
    if s.is_empty() {
        return "";
    }
    let actual_start = if start_byte >= s.len() {
        s.len()
    } else {
        let mut idx = start_byte;
        while idx > 0 && !s.is_char_boundary(idx) {
            idx -= 1;
        }
        idx
    };

    let target_end = (actual_start + max_len).min(s.len());
    let mut actual_end = target_end;
    while actual_end > actual_start && !s.is_char_boundary(actual_end) {
        actual_end -= 1;
    }

    &s[actual_start..actual_end]
}

impl CacheFingerprint {
    pub fn compute(prompt: &str) -> Option<Self> {
        if prompt.len() < MIN_CACHE_PROMPT_LEN {
            return None;
        }

        let len = prompt.len() as u32;
        let head_slice = safe_char_slice(prompt, 0, 512);
        let tail_start = prompt.len().saturating_sub(512);
        let tail_slice = safe_char_slice(prompt, tail_start, 512);
        let mid_start = prompt.len() / 4;
        let mid_slice = safe_char_slice(prompt, mid_start, 512);

        let mut h1 = DefaultHasher::new();
        head_slice.hash(&mut h1);
        let head_hash = h1.finish();

        let mut h2 = DefaultHasher::new();
        tail_slice.hash(&mut h2);
        let tail_hash = h2.finish();

        let mut h3 = DefaultHasher::new();
        mid_slice.hash(&mut h3);
        let mid_hash = h3.finish();

        Some(Self {
            len,
            head_hash,
            tail_hash,
            mid_hash,
        })
    }
}

#[derive(Debug, Clone)]
struct CacheEntry {
    provider_name: String,
    last_seen: Instant,
}

/// 64-shard lock-striped hot cache tracker
#[derive(Debug)]
pub struct HotCacheTracker {
    shards: Vec<RwLock<HashMap<CacheFingerprint, CacheEntry>>>,
    ttl: Duration,
}

impl Default for HotCacheTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl HotCacheTracker {
    pub fn new() -> Self {
        let mut shards = Vec::with_capacity(NUM_SHARDS);
        for _ in 0..NUM_SHARDS {
            shards.push(RwLock::new(HashMap::new()));
        }
        Self {
            shards,
            ttl: DEFAULT_CACHE_TTL,
        }
    }

    fn shard_idx(&self, fp: &CacheFingerprint) -> usize {
        (fp.head_hash ^ fp.tail_hash) as usize % NUM_SHARDS
    }

    /// Record a prompt dispatch to a specific provider
    pub fn record_dispatch(&self, prompt: &str, provider: &str) {
        if let Some(fp) = CacheFingerprint::compute(prompt) {
            let idx = self.shard_idx(&fp);
            if let Ok(mut map) = self.shards[idx].write() {
                if map.len() >= MAX_ENTRIES_PER_SHARD {
                    let now = Instant::now();
                    map.retain(|_, entry| now.duration_since(entry.last_seen) < self.ttl);
                }
                map.insert(
                    fp,
                    CacheEntry {
                        provider_name: provider.to_string(),
                        last_seen: Instant::now(),
                    },
                );
            }
        }
    }

    /// Probe if a prompt prefix matches a recently used hot provider
    pub fn probe_cached_provider(&self, prompt: &str) -> Option<String> {
        let fp = CacheFingerprint::compute(prompt)?;
        let idx = self.shard_idx(&fp);
        let map = self.shards[idx].read().ok()?;
        if let Some(entry) = map.get(&fp) {
            if entry.last_seen.elapsed() < self.ttl {
                return Some(entry.provider_name.clone());
            }
        }
        None
    }

    /// Cleanup expired entries across all shards
    pub fn cleanup_expired(&self) {
        let now = Instant::now();
        for shard in &self.shards {
            if let Ok(mut map) = shard.write() {
                map.retain(|_, entry| now.duration_since(entry.last_seen) < self.ttl);
            }
        }
    }
}
