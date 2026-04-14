// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
// L1 in-memory cache: DashMap-backed, sub-microsecond hot path.
//
// Provides Redis-style data structures (string/hash/list/set/zset/stream-stub),
// approximate sketches (Bloom, HyperLogLog), TTL expiry, glob KEYS, and
// LRU-ish byte-budget eviction. String values are stored as lz4-compressed
// `Bytes` so the cache stays memory-efficient even for large payloads.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering as AtomicOrdering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use dashmap::DashMap;
use probabilistic_collections::bloom::BloomFilter;
use probabilistic_collections::hyperloglog::HyperLogLog;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as JsonValue};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Logical kind of a cache entry — used for type checks and stats segregation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EntryKind {
    String,
    Hash,
    List,
    Set,
    ZSet,
    Stream,
    Bloom,
    HyperLogLog,
}

impl EntryKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Hash => "hash",
            Self::List => "list",
            Self::Set => "set",
            Self::ZSet => "zset",
            Self::Stream => "stream",
            Self::Bloom => "bloom",
            Self::HyperLogLog => "hll",
        }
    }
}

/// One slot stored in the L1 DashMap.
///
/// `value` is the lz4-compressed encoding of the structured payload. We keep
/// the entry kind alongside it so we can reject WRONGTYPE operations cheaply
/// without paying decompression cost.
#[derive(Debug)]
pub struct CacheEntry {
    pub value: Bytes,
    pub expires_at: Option<Instant>,
    pub kind: EntryKind,
    pub size_bytes: usize,
    pub hits: AtomicU64,
    pub created_at: Instant,
    /// Monotonically updated on every read — used as the LRU tiebreaker.
    pub last_access_tick: AtomicU64,
}

impl CacheEntry {
    fn new(kind: EntryKind, value: Bytes, ttl: Option<Duration>) -> Self {
        let now = Instant::now();
        let size_bytes = value.len();
        Self {
            value,
            expires_at: ttl.map(|d| now + d),
            kind,
            size_bytes,
            hits: AtomicU64::new(0),
            created_at: now,
            last_access_tick: AtomicU64::new(0),
        }
    }

    fn is_expired(&self, now: Instant) -> bool {
        match self.expires_at {
            Some(t) => now >= t,
            None => false,
        }
    }

    fn touch(&self, ticker: &AtomicU64) {
        self.hits.fetch_add(1, AtomicOrdering::Relaxed);
        let tick = ticker.fetch_add(1, AtomicOrdering::Relaxed);
        self.last_access_tick.store(tick, AtomicOrdering::Relaxed);
    }
}

/// Snapshot of cache counters returned from [`L1Cache::stats`].
#[derive(Debug, Clone, Copy)]
pub struct CacheStats {
    pub keys: usize,
    pub used_bytes: usize,
    pub max_bytes: usize,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

// ---------------------------------------------------------------------------
// L1 cache
// ---------------------------------------------------------------------------

/// L1 (process-local) cache.
///
/// `max_bytes == 0` disables eviction.
pub struct L1Cache {
    store: DashMap<String, CacheEntry>,
    max_bytes: AtomicUsize,
    used_bytes: AtomicUsize,
    hits: AtomicU64,
    misses: AtomicU64,
    evictions: AtomicU64,
    /// Monotonic counter that drives LRU ordering (cheap, no clock cost).
    access_tick: AtomicU64,
    /// Bloom filters live in their own map — they are mutable and don't fit
    /// neatly inside `CacheEntry` semantics.
    blooms: DashMap<String, Mutex<BloomFilter<String>>>,
    /// HyperLogLog sketches likewise.
    hlls: DashMap<String, Mutex<HyperLogLog<String>>>,
}

impl Default for L1Cache {
    fn default() -> Self {
        Self::new(0)
    }
}

impl L1Cache {
    /// Create a new L1 cache. Pass `0` to disable byte-budget eviction.
    pub fn new(max_bytes: usize) -> Self {
        Self {
            store: DashMap::new(),
            max_bytes: AtomicUsize::new(max_bytes),
            used_bytes: AtomicUsize::new(0),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
            access_tick: AtomicU64::new(1),
            blooms: DashMap::new(),
            hlls: DashMap::new(),
        }
    }

    pub fn set_max_bytes(&self, max_bytes: usize) {
        self.max_bytes.store(max_bytes, AtomicOrdering::Relaxed);
        self.maybe_evict();
    }

    // ----- low-level value codec -------------------------------------------

    fn encode(payload: &[u8]) -> Bytes {
        // lz4 is sub-μs on small payloads; we accept a tiny CPU cost for
        // material RSS savings on hot keys.
        Bytes::from(lz4_flex::compress_prepend_size(payload))
    }

    fn decode(bytes: &Bytes) -> Result<Vec<u8>, CacheError> {
        lz4_flex::decompress_size_prepended(bytes).map_err(|e| CacheError::Codec(e.to_string()))
    }

    fn decode_json(bytes: &Bytes) -> Result<JsonValue, CacheError> {
        let raw = Self::decode(bytes)?;
        serde_json::from_slice(&raw).map_err(|e| CacheError::Codec(e.to_string()))
    }

    fn encode_json(value: &JsonValue) -> Bytes {
        Self::encode(&serde_json::to_vec(value).expect("JSON serialization is infallible here"))
    }

    // ----- internal insert/remove with byte accounting ---------------------

    fn insert_entry(&self, key: String, entry: CacheEntry) {
        let new_size = entry.size_bytes;
        if let Some(prev) = self.store.insert(key, entry) {
            self.used_bytes
                .fetch_sub(prev.size_bytes, AtomicOrdering::Relaxed);
        }
        self.used_bytes
            .fetch_add(new_size, AtomicOrdering::Relaxed);
        self.maybe_evict();
    }

    fn remove_entry(&self, key: &str) -> Option<CacheEntry> {
        let (_, prev) = self.store.remove(key)?;
        self.used_bytes
            .fetch_sub(prev.size_bytes, AtomicOrdering::Relaxed);
        Some(prev)
    }

    fn check_kind(&self, key: &str, expected: EntryKind) -> Result<(), CacheError> {
        let mut wrong: Option<EntryKind> = None;
        let mut expired = false;
        if let Some(entry) = self.store.get(key) {
            if entry.is_expired(Instant::now()) {
                expired = true;
            } else if entry.kind != expected {
                wrong = Some(entry.kind);
            }
        }
        if expired {
            self.remove_entry(key);
        }
        if let Some(actual) = wrong {
            return Err(CacheError::WrongType { expected, actual });
        }
        Ok(())
    }

    fn load_json(&self, key: &str, expected: EntryKind) -> Result<Option<JsonValue>, CacheError> {
        let mut wrong: Option<EntryKind> = None;
        let mut expired = false;
        let mut payload: Option<JsonValue> = None;
        if let Some(entry) = self.store.get(key) {
            if entry.is_expired(Instant::now()) {
                expired = true;
            } else if entry.kind != expected {
                wrong = Some(entry.kind);
            } else {
                entry.touch(&self.access_tick);
                payload = Some(Self::decode_json(&entry.value)?);
            }
        }
        if expired {
            self.remove_entry(key);
        }
        if let Some(actual) = wrong {
            return Err(CacheError::WrongType { expected, actual });
        }
        Ok(payload)
    }

    // ----- string ops ------------------------------------------------------

    /// SET key value [PX ttl]
    pub fn set(&self, key: &str, value: &[u8], ttl: Option<Duration>) {
        let entry = CacheEntry::new(EntryKind::String, Self::encode(value), ttl);
        self.insert_entry(key.to_string(), entry);
        metrics::counter!("ddb_cache_l1_set_total").increment(1);
    }

    /// GET key
    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        let now = Instant::now();
        let mut expired = false;
        let result = match self.store.get(key) {
            Some(entry) => {
                if entry.is_expired(now) {
                    expired = true;
                    None
                } else if entry.kind != EntryKind::String {
                    None
                } else {
                    entry.touch(&self.access_tick);
                    Self::decode(&entry.value).ok()
                }
            }
            None => None,
        };
        if expired {
            self.remove_entry(key);
        }
        match result {
            Some(v) => {
                self.hits.fetch_add(1, AtomicOrdering::Relaxed);
                metrics::counter!("ddb_cache_l1_hit_total").increment(1);
                Some(v)
            }
            None => {
                self.misses.fetch_add(1, AtomicOrdering::Relaxed);
                metrics::counter!("ddb_cache_l1_miss_total").increment(1);
                None
            }
        }
    }

    /// DEL key — returns true if the key was present.
    pub fn del(&self, key: &str) -> bool {
        let removed = self.remove_entry(key).is_some();
        let bf_removed = self.blooms.remove(key).is_some();
        let hll_removed = self.hlls.remove(key).is_some();
        let any = removed || bf_removed || hll_removed;
        if any {
            metrics::counter!("ddb_cache_l1_del_total").increment(1);
        }
        any
    }

    /// EXISTS key
    pub fn exists(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut expired = false;
        let present = match self.store.get(key) {
            Some(e) => {
                if e.is_expired(now) {
                    expired = true;
                    false
                } else {
                    true
                }
            }
            None => self.blooms.contains_key(key) || self.hlls.contains_key(key),
        };
        if expired {
            self.remove_entry(key);
        }
        present
    }

    /// EXPIRE key ttl — returns false if the key is missing.
    pub fn expire(&self, key: &str, ttl: Duration) -> bool {
        let mut entry = match self.store.get_mut(key) {
            Some(e) => e,
            None => return false,
        };
        entry.expires_at = Some(Instant::now() + ttl);
        true
    }

    /// TTL key — returns:
    ///   `None` if the key is missing,
    ///   `Some(None)` if the key has no TTL,
    ///   `Some(Some(d))` for the remaining duration.
    pub fn ttl(&self, key: &str) -> Option<Option<Duration>> {
        let entry = self.store.get(key)?;
        Some(
            entry
                .expires_at
                .map(|t| t.saturating_duration_since(Instant::now())),
        )
    }

    /// KEYS pattern — supports `*`, `?` and literal chars.
    pub fn keys(&self, pattern: &str) -> Vec<String> {
        let now = Instant::now();
        let mut out = Vec::new();
        for entry in self.store.iter() {
            if entry.is_expired(now) {
                continue;
            }
            if glob_match(pattern, entry.key()) {
                out.push(entry.key().clone());
            }
        }
        out.sort();
        out
    }

    /// FLUSHDB — drop everything.
    pub fn flush(&self) {
        self.store.clear();
        self.blooms.clear();
        self.hlls.clear();
        self.used_bytes.store(0, AtomicOrdering::Relaxed);
    }

    pub fn stats(&self) -> CacheStats {
        CacheStats {
            keys: self.store.len(),
            used_bytes: self.used_bytes.load(AtomicOrdering::Relaxed),
            max_bytes: self.max_bytes.load(AtomicOrdering::Relaxed),
            hits: self.hits.load(AtomicOrdering::Relaxed),
            misses: self.misses.load(AtomicOrdering::Relaxed),
            evictions: self.evictions.load(AtomicOrdering::Relaxed),
        }
    }

    // ----- hash ops --------------------------------------------------------

    pub fn hset(&self, key: &str, field: &str, value: &[u8]) -> Result<bool, CacheError> {
        self.check_kind(key, EntryKind::Hash)?;
        let mut map = match self.load_json(key, EntryKind::Hash)? {
            Some(JsonValue::Object(m)) => m,
            _ => JsonMap::new(),
        };
        let field_value = JsonValue::String(base64_encode(value));
        let is_new = map.insert(field.to_string(), field_value).is_none();
        let entry = CacheEntry::new(
            EntryKind::Hash,
            Self::encode_json(&JsonValue::Object(map)),
            None,
        );
        self.insert_entry(key.to_string(), entry);
        Ok(is_new)
    }

    pub fn hget(&self, key: &str, field: &str) -> Result<Option<Vec<u8>>, CacheError> {
        let map = match self.load_json(key, EntryKind::Hash)? {
            Some(JsonValue::Object(m)) => m,
            _ => return Ok(None),
        };
        match map.get(field) {
            Some(JsonValue::String(s)) => Ok(Some(base64_decode(s)?)),
            _ => Ok(None),
        }
    }

    pub fn hgetall(&self, key: &str) -> Result<Vec<(String, Vec<u8>)>, CacheError> {
        let map = match self.load_json(key, EntryKind::Hash)? {
            Some(JsonValue::Object(m)) => m,
            _ => return Ok(Vec::new()),
        };
        let mut out = Vec::with_capacity(map.len());
        for (k, v) in map {
            if let JsonValue::String(s) = v {
                out.push((k, base64_decode(&s)?));
            }
        }
        Ok(out)
    }

    pub fn hdel(&self, key: &str, field: &str) -> Result<bool, CacheError> {
        self.check_kind(key, EntryKind::Hash)?;
        let mut map = match self.load_json(key, EntryKind::Hash)? {
            Some(JsonValue::Object(m)) => m,
            _ => return Ok(false),
        };
        let removed = map.remove(field).is_some();
        if map.is_empty() {
            self.remove_entry(key);
        } else {
            let entry = CacheEntry::new(
                EntryKind::Hash,
                Self::encode_json(&JsonValue::Object(map)),
                None,
            );
            self.insert_entry(key.to_string(), entry);
        }
        Ok(removed)
    }

    pub fn hlen(&self, key: &str) -> Result<usize, CacheError> {
        match self.load_json(key, EntryKind::Hash)? {
            Some(JsonValue::Object(m)) => Ok(m.len()),
            _ => Ok(0),
        }
    }

    // ----- list ops --------------------------------------------------------

    fn load_list(&self, key: &str) -> Result<Vec<String>, CacheError> {
        match self.load_json(key, EntryKind::List)? {
            Some(JsonValue::Array(arr)) => Ok(arr
                .into_iter()
                .filter_map(|v| match v {
                    JsonValue::String(s) => Some(s),
                    _ => None,
                })
                .collect()),
            _ => Ok(Vec::new()),
        }
    }

    fn store_list(&self, key: &str, list: Vec<String>) {
        if list.is_empty() {
            self.remove_entry(key);
            return;
        }
        let json = JsonValue::Array(list.into_iter().map(JsonValue::String).collect());
        let entry = CacheEntry::new(EntryKind::List, Self::encode_json(&json), None);
        self.insert_entry(key.to_string(), entry);
    }

    pub fn lpush(&self, key: &str, value: &str) -> Result<usize, CacheError> {
        self.check_kind(key, EntryKind::List)?;
        let mut list = self.load_list(key)?;
        list.insert(0, value.to_string());
        let len = list.len();
        self.store_list(key, list);
        Ok(len)
    }

    pub fn rpush(&self, key: &str, value: &str) -> Result<usize, CacheError> {
        self.check_kind(key, EntryKind::List)?;
        let mut list = self.load_list(key)?;
        list.push(value.to_string());
        let len = list.len();
        self.store_list(key, list);
        Ok(len)
    }

    pub fn lpop(&self, key: &str) -> Result<Option<String>, CacheError> {
        self.check_kind(key, EntryKind::List)?;
        let mut list = self.load_list(key)?;
        if list.is_empty() {
            return Ok(None);
        }
        let v = list.remove(0);
        self.store_list(key, list);
        Ok(Some(v))
    }

    pub fn rpop(&self, key: &str) -> Result<Option<String>, CacheError> {
        self.check_kind(key, EntryKind::List)?;
        let mut list = self.load_list(key)?;
        let v = list.pop();
        self.store_list(key, list);
        Ok(v)
    }

    pub fn lrange(&self, key: &str, start: i64, stop: i64) -> Result<Vec<String>, CacheError> {
        let list = self.load_list(key)?;
        let len = list.len() as i64;
        if len == 0 {
            return Ok(Vec::new());
        }
        let norm = |i: i64| -> i64 {
            if i < 0 { (len + i).max(0) } else { i.min(len - 1) }
        };
        let s = norm(start);
        let e = norm(stop);
        if s > e {
            return Ok(Vec::new());
        }
        Ok(list[s as usize..=e as usize].to_vec())
    }

    pub fn llen(&self, key: &str) -> Result<usize, CacheError> {
        Ok(self.load_list(key)?.len())
    }

    // ----- zset ops --------------------------------------------------------

    fn load_zset(&self, key: &str) -> Result<Vec<(f64, String)>, CacheError> {
        match self.load_json(key, EntryKind::ZSet)? {
            Some(JsonValue::Array(arr)) => {
                let mut out = Vec::with_capacity(arr.len());
                for v in arr {
                    if let JsonValue::Array(pair) = v
                        && pair.len() == 2
                    {
                        let score = pair[0].as_f64().unwrap_or(0.0);
                        let member = pair[1].as_str().unwrap_or("").to_string();
                        out.push((score, member));
                    }
                }
                Ok(out)
            }
            _ => Ok(Vec::new()),
        }
    }

    fn store_zset(&self, key: &str, mut zset: Vec<(f64, String)>) {
        zset.sort_by(|a, b| match a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal) {
            Ordering::Equal => a.1.cmp(&b.1),
            ord => ord,
        });
        if zset.is_empty() {
            self.remove_entry(key);
            return;
        }
        let json = JsonValue::Array(
            zset.into_iter()
                .map(|(score, member)| {
                    JsonValue::Array(vec![
                        JsonValue::Number(
                            serde_json::Number::from_f64(score)
                                .unwrap_or_else(|| serde_json::Number::from(0)),
                        ),
                        JsonValue::String(member),
                    ])
                })
                .collect(),
        );
        let entry = CacheEntry::new(EntryKind::ZSet, Self::encode_json(&json), None);
        self.insert_entry(key.to_string(), entry);
    }

    pub fn zadd(&self, key: &str, score: f64, member: &str) -> Result<bool, CacheError> {
        self.check_kind(key, EntryKind::ZSet)?;
        let mut zset = self.load_zset(key)?;
        let mut is_new = true;
        for (s, m) in zset.iter_mut() {
            if m == member {
                *s = score;
                is_new = false;
                break;
            }
        }
        if is_new {
            zset.push((score, member.to_string()));
        }
        self.store_zset(key, zset);
        Ok(is_new)
    }

    pub fn zrange(&self, key: &str, start: i64, stop: i64) -> Result<Vec<String>, CacheError> {
        let zset = self.load_zset(key)?;
        let len = zset.len() as i64;
        if len == 0 {
            return Ok(Vec::new());
        }
        let norm = |i: i64| -> i64 {
            if i < 0 { (len + i).max(0) } else { i.min(len - 1) }
        };
        let s = norm(start);
        let e = norm(stop);
        if s > e {
            return Ok(Vec::new());
        }
        Ok(zset[s as usize..=e as usize]
            .iter()
            .map(|(_, m)| m.clone())
            .collect())
    }

    pub fn zrangebyscore(&self, key: &str, min: f64, max: f64) -> Result<Vec<String>, CacheError> {
        let zset = self.load_zset(key)?;
        Ok(zset
            .into_iter()
            .filter(|(s, _)| *s >= min && *s <= max)
            .map(|(_, m)| m)
            .collect())
    }

    pub fn zrank(&self, key: &str, member: &str) -> Result<Option<usize>, CacheError> {
        let zset = self.load_zset(key)?;
        Ok(zset.iter().position(|(_, m)| m == member))
    }

    pub fn zrem(&self, key: &str, member: &str) -> Result<bool, CacheError> {
        self.check_kind(key, EntryKind::ZSet)?;
        let mut zset = self.load_zset(key)?;
        let len_before = zset.len();
        zset.retain(|(_, m)| m != member);
        let removed = zset.len() != len_before;
        self.store_zset(key, zset);
        Ok(removed)
    }

    pub fn zcard(&self, key: &str) -> Result<usize, CacheError> {
        Ok(self.load_zset(key)?.len())
    }

    pub fn zscore(&self, key: &str, member: &str) -> Result<Option<f64>, CacheError> {
        let zset = self.load_zset(key)?;
        Ok(zset.into_iter().find(|(_, m)| m == member).map(|(s, _)| s))
    }

    // ----- bloom filter ----------------------------------------------------

    pub fn bf_add(&self, key: &str, item: &str) {
        let entry = self
            .blooms
            .entry(key.to_string())
            .or_insert_with(|| Mutex::new(BloomFilter::<String>::new(100_000, 0.01)));
        let mut bf = entry.lock().expect("bloom filter mutex poisoned");
        bf.insert(&item.to_string());
    }

    pub fn bf_exists(&self, key: &str, item: &str) -> bool {
        match self.blooms.get(key) {
            Some(entry) => {
                let bf = entry.lock().expect("bloom filter mutex poisoned");
                bf.contains(&item.to_string())
            }
            None => false,
        }
    }

    // ----- HyperLogLog -----------------------------------------------------

    pub fn pf_add(&self, key: &str, item: &str) {
        let entry = self
            .hlls
            .entry(key.to_string())
            .or_insert_with(|| Mutex::new(HyperLogLog::<String>::new(0.01)));
        let mut hll = entry.lock().expect("HLL mutex poisoned");
        hll.insert(&item.to_string());
    }

    pub fn pf_count(&self, key: &str) -> u64 {
        match self.hlls.get(key) {
            Some(entry) => {
                let hll = entry.lock().expect("HLL mutex poisoned");
                hll.len() as u64
            }
            None => 0,
        }
    }

    // ----- eviction --------------------------------------------------------

    fn maybe_evict(&self) {
        let max = self.max_bytes.load(AtomicOrdering::Relaxed);
        if max == 0 {
            return;
        }
        let mut used = self.used_bytes.load(AtomicOrdering::Relaxed);
        if used <= max {
            return;
        }
        let target = (max as f64 * 0.9) as usize;

        // Build an LRU index sorted by last_access tick. We tolerate the
        // O(n log n) cost here because eviction is a rare, bursty event and
        // the hot path remains lock-free.
        let mut lru: BTreeMap<u64, Vec<String>> = BTreeMap::new();
        for entry in self.store.iter() {
            let tick = entry.last_access_tick.load(AtomicOrdering::Relaxed);
            lru.entry(tick).or_default().push(entry.key().clone());
        }

        'outer: for (_tick, keys) in lru.into_iter() {
            for key in keys {
                if used <= target {
                    break 'outer;
                }
                if self.remove_entry(&key).is_some() {
                    used = self.used_bytes.load(AtomicOrdering::Relaxed);
                    self.evictions.fetch_add(1, AtomicOrdering::Relaxed);
                    metrics::counter!("ddb_cache_l1_evict_total").increment(1);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum CacheError {
    WrongType {
        expected: EntryKind,
        actual: EntryKind,
    },
    Codec(String),
}

impl std::fmt::Display for CacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongType { expected, actual } => write!(
                f,
                "WRONGTYPE expected {} got {}",
                expected.as_str(),
                actual.as_str()
            ),
            Self::Codec(msg) => write!(f, "codec error: {msg}"),
        }
    }
}

impl std::error::Error for CacheError {}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn base64_encode(bytes: &[u8]) -> String {
    // Tiny, dependency-free base64 encode (standard alphabet).
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let mut i = 0;
    while i + 3 <= bytes.len() {
        let n = ((bytes[i] as u32) << 16) | ((bytes[i + 1] as u32) << 8) | (bytes[i + 2] as u32);
        out.push(ALPHABET[((n >> 18) & 0x3F) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3F) as usize] as char);
        out.push(ALPHABET[((n >> 6) & 0x3F) as usize] as char);
        out.push(ALPHABET[(n & 0x3F) as usize] as char);
        i += 3;
    }
    let rem = bytes.len() - i;
    if rem == 1 {
        let n = (bytes[i] as u32) << 16;
        out.push(ALPHABET[((n >> 18) & 0x3F) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3F) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let n = ((bytes[i] as u32) << 16) | ((bytes[i + 1] as u32) << 8);
        out.push(ALPHABET[((n >> 18) & 0x3F) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3F) as usize] as char);
        out.push(ALPHABET[((n >> 6) & 0x3F) as usize] as char);
        out.push('=');
    }
    out
}

fn base64_decode(s: &str) -> Result<Vec<u8>, CacheError> {
    fn val(c: u8) -> Result<u8, CacheError> {
        match c {
            b'A'..=b'Z' => Ok(c - b'A'),
            b'a'..=b'z' => Ok(c - b'a' + 26),
            b'0'..=b'9' => Ok(c - b'0' + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            _ => Err(CacheError::Codec("invalid base64".into())),
        }
    }
    let bytes = s.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return Err(CacheError::Codec("invalid base64 length".into()));
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let mut i = 0;
    while i < bytes.len() {
        let c0 = bytes[i];
        let c1 = bytes[i + 1];
        let c2 = bytes[i + 2];
        let c3 = bytes[i + 3];
        let v0 = val(c0)?;
        let v1 = val(c1)?;
        out.push((v0 << 2) | (v1 >> 4));
        if c2 != b'=' {
            let v2 = val(c2)?;
            out.push(((v1 & 0x0F) << 4) | (v2 >> 2));
            if c3 != b'=' {
                let v3 = val(c3)?;
                out.push(((v2 & 0x03) << 6) | v3);
            }
        }
        i += 4;
    }
    Ok(out)
}

/// Tiny glob matcher supporting `*` and `?`.
fn glob_match(pattern: &str, text: &str) -> bool {
    fn rec(p: &[u8], t: &[u8]) -> bool {
        if p.is_empty() {
            return t.is_empty();
        }
        match p[0] {
            b'*' => {
                if rec(&p[1..], t) {
                    return true;
                }
                if !t.is_empty() && rec(p, &t[1..]) {
                    return true;
                }
                false
            }
            b'?' => !t.is_empty() && rec(&p[1..], &t[1..]),
            c => !t.is_empty() && t[0] == c && rec(&p[1..], &t[1..]),
        }
    }
    rec(pattern.as_bytes(), text.as_bytes())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn set_get_del_roundtrip() {
        let c = L1Cache::new(0);
        c.set("k", b"hello", None);
        assert_eq!(c.get("k").as_deref(), Some(&b"hello"[..]));
        assert!(c.exists("k"));
        assert!(c.del("k"));
        assert!(c.get("k").is_none());
        assert!(!c.exists("k"));
    }

    #[test]
    fn ttl_expiry() {
        let c = L1Cache::new(0);
        c.set("k", b"v", Some(Duration::from_millis(40)));
        assert!(c.get("k").is_some());
        sleep(Duration::from_millis(80));
        assert!(c.get("k").is_none());
    }

    #[test]
    fn expire_and_ttl_query() {
        let c = L1Cache::new(0);
        c.set("k", b"v", None);
        assert!(c.expire("k", Duration::from_secs(60)));
        let t = c.ttl("k").expect("key present");
        let remaining = t.expect("ttl set");
        assert!(remaining.as_secs() <= 60 && remaining.as_secs() >= 58);
        assert!(!c.expire("missing", Duration::from_secs(1)));
    }

    #[test]
    fn keys_glob() {
        let c = L1Cache::new(0);
        for k in ["user:1", "user:2", "post:1"] {
            c.set(k, b"x", None);
        }
        let mut all = c.keys("*");
        all.sort();
        assert_eq!(all, vec!["post:1", "user:1", "user:2"]);
        let users = c.keys("user:*");
        assert_eq!(users, vec!["user:1", "user:2"]);
        let q = c.keys("user:?");
        assert_eq!(q, vec!["user:1", "user:2"]);
    }

    #[test]
    fn flush_resets_state() {
        let c = L1Cache::new(0);
        c.set("a", b"x", None);
        c.set("b", b"y", None);
        c.flush();
        let s = c.stats();
        assert_eq!(s.keys, 0);
        assert_eq!(s.used_bytes, 0);
    }

    #[test]
    fn stats_track_hits_and_misses() {
        let c = L1Cache::new(0);
        c.set("k", b"v", None);
        let _ = c.get("k");
        let _ = c.get("missing");
        let s = c.stats();
        assert_eq!(s.hits, 1);
        assert_eq!(s.misses, 1);
    }

    #[test]
    fn hash_ops() {
        let c = L1Cache::new(0);
        assert!(c.hset("h", "f1", b"v1").unwrap());
        assert!(c.hset("h", "f2", b"v2").unwrap());
        assert!(!c.hset("h", "f1", b"v1b").unwrap());
        assert_eq!(c.hget("h", "f1").unwrap().as_deref(), Some(&b"v1b"[..]));
        assert_eq!(c.hlen("h").unwrap(), 2);
        let mut all = c.hgetall("h").unwrap();
        all.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(all[0].0, "f1");
        assert_eq!(all[1].0, "f2");
        assert!(c.hdel("h", "f1").unwrap());
        assert_eq!(c.hlen("h").unwrap(), 1);
        assert!(c.hget("h", "f1").unwrap().is_none());
    }

    #[test]
    fn list_ops() {
        let c = L1Cache::new(0);
        assert_eq!(c.rpush("l", "a").unwrap(), 1);
        assert_eq!(c.rpush("l", "b").unwrap(), 2);
        assert_eq!(c.lpush("l", "z").unwrap(), 3);
        assert_eq!(c.llen("l").unwrap(), 3);
        assert_eq!(c.lrange("l", 0, -1).unwrap(), vec!["z", "a", "b"]);
        assert_eq!(c.lpop("l").unwrap().as_deref(), Some("z"));
        assert_eq!(c.rpop("l").unwrap().as_deref(), Some("b"));
        assert_eq!(c.llen("l").unwrap(), 1);
    }

    #[test]
    fn zset_ops() {
        let c = L1Cache::new(0);
        assert!(c.zadd("z", 1.0, "a").unwrap());
        assert!(c.zadd("z", 2.0, "b").unwrap());
        assert!(c.zadd("z", 3.0, "c").unwrap());
        assert!(!c.zadd("z", 1.5, "a").unwrap()); // update
        assert_eq!(c.zcard("z").unwrap(), 3);
        assert_eq!(c.zscore("z", "a").unwrap(), Some(1.5));
        assert_eq!(c.zrange("z", 0, -1).unwrap(), vec!["a", "b", "c"]);
        assert_eq!(
            c.zrangebyscore("z", 1.6, 2.5).unwrap(),
            vec!["b".to_string()]
        );
        assert_eq!(c.zrank("z", "b").unwrap(), Some(1));
        assert!(c.zrem("z", "b").unwrap());
        assert_eq!(c.zcard("z").unwrap(), 2);
    }

    #[test]
    fn wrong_type_errors() {
        let c = L1Cache::new(0);
        c.set("k", b"v", None);
        let err = c.hset("k", "f", b"v").unwrap_err();
        match err {
            CacheError::WrongType { .. } => {}
            _ => panic!("expected WRONGTYPE"),
        }
    }

    #[test]
    fn bloom_filter() {
        let c = L1Cache::new(0);
        c.bf_add("bf", "alice");
        c.bf_add("bf", "bob");
        assert!(c.bf_exists("bf", "alice"));
        assert!(c.bf_exists("bf", "bob"));
        assert!(!c.bf_exists("bf", "nope_xyz_definitely_not_present_ever_12345"));
        assert!(!c.bf_exists("missing-key", "alice"));
    }

    #[test]
    fn hyperloglog() {
        let c = L1Cache::new(0);
        for i in 0..1000 {
            c.pf_add("h", &format!("user:{i}"));
        }
        let est = c.pf_count("h");
        // HLL with 1% error → expect within a generous 20% band.
        assert!(est > 800 && est < 1200, "HLL estimate {est} out of range");
        assert_eq!(c.pf_count("missing"), 0);
    }

    #[test]
    fn eviction_under_byte_budget() {
        // Tight budget: 256 bytes. Inserting larger payloads must evict.
        let c = L1Cache::new(256);
        for i in 0..32 {
            let payload = vec![b'x'; 64];
            c.set(&format!("k{i}"), &payload, None);
            // Touch earliest key to keep it hot
            if i % 4 == 0 {
                let _ = c.get("k0");
            }
        }
        let s = c.stats();
        assert!(s.evictions > 0, "expected evictions, got stats={s:?}");
        assert!(
            s.used_bytes <= 256,
            "used_bytes {} should be under budget",
            s.used_bytes
        );
        // The hottest key should still be present.
        assert!(c.get("k0").is_some(), "k0 was hot — should not be evicted");
    }

    #[test]
    fn glob_matcher_basic() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("user:*", "user:42"));
        assert!(glob_match("user:?", "user:1"));
        assert!(!glob_match("user:?", "user:12"));
        assert!(glob_match("a*c", "abbbc"));
        assert!(!glob_match("a*c", "abbb"));
    }

    #[test]
    fn base64_roundtrip() {
        for sample in &[
            b"".to_vec(),
            b"a".to_vec(),
            b"ab".to_vec(),
            b"abc".to_vec(),
            b"hello world!".to_vec(),
        ] {
            let enc = base64_encode(sample);
            let dec = base64_decode(&enc).unwrap();
            assert_eq!(&dec, sample);
        }
    }

    #[test]
    fn set_max_bytes_triggers_eviction() {
        let c = L1Cache::new(0);
        let payload = [b'x'; 64];
        for i in 0..10 {
            c.set(&format!("k{i}"), &payload, None);
        }
        let before = c.stats().keys;
        c.set_max_bytes(128);
        let after = c.stats();
        assert!(after.keys < before);
        assert!(after.used_bytes <= 128);
    }
}
