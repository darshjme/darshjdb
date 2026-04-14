// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
// ddb-cache :: DdbCache — unified in-process cache engine backing the
// RESP3 protocol server (packages/cache-server) and the HTTP REST cache
// API (`/api/cache/*`). Supports:
//
//   - STRING values with optional TTL
//   - HASH (field → value maps)
//   - LIST (deques with LPUSH/RPUSH/LPOP/RPOP/LRANGE)
//   - ZSET (sorted sets, score-ordered)
//   - STREAM (append-only entries, XADD/XRANGE/XREAD)
//   - BLOOM filter (BFADD/BFEXISTS) — approximate membership
//   - HLL (PFADD/PFCOUNT) — approximate cardinality
//   - PUB/SUB fan-out via tokio broadcast channels
//   - glob-style KEYS pattern matching

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, broadcast};

// ---------------------------------------------------------------------------
// Public stats surface
// ---------------------------------------------------------------------------

/// Aggregate statistics for the DdbCache instance.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DdbCacheStats {
    pub strings: u64,
    pub hashes: u64,
    pub lists: u64,
    pub zsets: u64,
    pub streams: u64,
    pub blooms: u64,
    pub hlls: u64,
    pub hits: u64,
    pub misses: u64,
    pub expired: u64,
    pub channels: u64,
}

// ---------------------------------------------------------------------------
// Internal entry types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct StringEntry {
    value: Vec<u8>,
    expires_at: Option<Instant>,
}

impl StringEntry {
    fn is_expired(&self) -> bool {
        match self.expires_at {
            Some(deadline) => Instant::now() >= deadline,
            None => false,
        }
    }
}

#[derive(Debug, Clone)]
struct ZSetMember {
    score: f64,
    member: String,
}

#[derive(Debug, Default)]
struct ZSetEntry {
    members: Vec<ZSetMember>,
}

impl ZSetEntry {
    fn add(&mut self, score: f64, member: String) -> bool {
        if let Some(existing) = self.members.iter_mut().find(|m| m.member == member) {
            existing.score = score;
            self.sort();
            false
        } else {
            self.members.push(ZSetMember { score, member });
            self.sort();
            true
        }
    }

    fn sort(&mut self) {
        self.members.sort_by(|a, b| {
            a.score
                .partial_cmp(&b.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.member.cmp(&b.member))
        });
    }

    fn remove(&mut self, member: &str) -> bool {
        let len = self.members.len();
        self.members.retain(|m| m.member != member);
        self.members.len() != len
    }

    fn rank(&self, member: &str) -> Option<usize> {
        self.members.iter().position(|m| m.member == member)
    }

    fn score(&self, member: &str) -> Option<f64> {
        self.members
            .iter()
            .find(|m| m.member == member)
            .map(|m| m.score)
    }

    fn range(&self, start: i64, stop: i64) -> Vec<(String, f64)> {
        let len = self.members.len() as i64;
        if len == 0 {
            return Vec::new();
        }
        let norm = |i: i64| -> i64 {
            if i < 0 {
                (len + i).max(0)
            } else {
                i.min(len - 1)
            }
        };
        let s = norm(start);
        let e = norm(stop);
        if s > e {
            return Vec::new();
        }
        self.members[s as usize..=e as usize]
            .iter()
            .map(|m| (m.member.clone(), m.score))
            .collect()
    }

    fn range_by_score(&self, min: f64, max: f64) -> Vec<(String, f64)> {
        self.members
            .iter()
            .filter(|m| m.score >= min && m.score <= max)
            .map(|m| (m.member.clone(), m.score))
            .collect()
    }
}

/// Append-only stream entry produced by `XADD`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamEntry {
    pub id: String,
    pub fields: Vec<(String, String)>,
}

#[derive(Debug, Default)]
struct StreamState {
    entries: Vec<StreamEntry>,
    last_ms: u64,
    last_seq: u64,
}

impl StreamState {
    fn next_id(&mut self) -> String {
        let now_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
        if now_ms == self.last_ms {
            self.last_seq += 1;
        } else {
            self.last_ms = now_ms;
            self.last_seq = 0;
        }
        format!("{}-{}", self.last_ms, self.last_seq)
    }
}

#[derive(Debug)]
struct BloomFilter {
    bits: Vec<u64>,
    k: usize,
    m: usize,
}

impl BloomFilter {
    fn new() -> Self {
        Self {
            bits: vec![0u64; 8192],
            k: 4,
            m: 8192 * 64,
        }
    }

    fn hashes(&self, item: &[u8]) -> [usize; 4] {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut out = [0usize; 4];
        for (i, slot) in out.iter_mut().enumerate() {
            let mut h = DefaultHasher::new();
            (i as u64).hash(&mut h);
            item.hash(&mut h);
            *slot = (h.finish() as usize) % self.m;
        }
        out
    }

    fn add(&mut self, item: &[u8]) -> bool {
        let mut was_new = false;
        for idx in self.hashes(item).iter().take(self.k) {
            let word = idx / 64;
            let bit = idx % 64;
            if self.bits[word] & (1u64 << bit) == 0 {
                was_new = true;
            }
            self.bits[word] |= 1u64 << bit;
        }
        was_new
    }

    fn contains(&self, item: &[u8]) -> bool {
        for idx in self.hashes(item).iter().take(self.k) {
            let word = idx / 64;
            let bit = idx % 64;
            if self.bits[word] & (1u64 << bit) == 0 {
                return false;
            }
        }
        true
    }
}

#[derive(Debug, Default)]
struct HyperLogLog {
    seen: std::collections::HashSet<u64>,
}

impl HyperLogLog {
    fn add(&mut self, item: &[u8]) -> bool {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        item.hash(&mut h);
        self.seen.insert(h.finish())
    }

    fn count(&self) -> u64 {
        self.seen.len() as u64
    }
}

// ---------------------------------------------------------------------------
// DdbCache
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct DdbCache {
    inner: Arc<DdbCacheInner>,
}

struct DdbCacheInner {
    strings: DashMap<String, StringEntry>,
    hashes: DashMap<String, HashMap<String, Vec<u8>>>,
    lists: DashMap<String, VecDeque<Vec<u8>>>,
    zsets: DashMap<String, ZSetEntry>,
    streams: DashMap<String, StreamState>,
    blooms: Mutex<HashMap<String, BloomFilter>>,
    hlls: Mutex<HashMap<String, HyperLogLog>>,
    channels: DashMap<String, broadcast::Sender<PubSubMessage>>,

    hits: AtomicU64,
    misses: AtomicU64,
    expired: AtomicU64,
}

/// A pub/sub message delivered on a channel.
#[derive(Debug, Clone)]
pub struct PubSubMessage {
    pub channel: String,
    pub payload: Vec<u8>,
}

impl Default for DdbCache {
    fn default() -> Self {
        Self::new()
    }
}

impl DdbCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(DdbCacheInner {
                strings: DashMap::new(),
                hashes: DashMap::new(),
                lists: DashMap::new(),
                zsets: DashMap::new(),
                streams: DashMap::new(),
                blooms: Mutex::new(HashMap::new()),
                hlls: Mutex::new(HashMap::new()),
                channels: DashMap::new(),
                hits: AtomicU64::new(0),
                misses: AtomicU64::new(0),
                expired: AtomicU64::new(0),
            }),
        }
    }

    // ── STRING ─────────────────────────────────────────────────────────

    pub fn set(&self, key: impl Into<String>, value: impl Into<Vec<u8>>, ttl: Option<Duration>) {
        let expires_at = ttl.map(|d| Instant::now() + d);
        self.inner.strings.insert(
            key.into(),
            StringEntry {
                value: value.into(),
                expires_at,
            },
        );
    }

    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        let mut drop_expired = false;
        let result = match self.inner.strings.get(key) {
            Some(entry) if entry.is_expired() => {
                drop_expired = true;
                None
            }
            Some(entry) => Some(entry.value.clone()),
            None => None,
        };
        if drop_expired {
            self.inner.strings.remove(key);
            self.inner.expired.fetch_add(1, Ordering::Relaxed);
            self.inner.misses.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        if result.is_some() {
            self.inner.hits.fetch_add(1, Ordering::Relaxed);
        } else {
            self.inner.misses.fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    pub fn del(&self, key: &str) -> bool {
        let mut removed = false;
        if self.inner.strings.remove(key).is_some() {
            removed = true;
        }
        if self.inner.hashes.remove(key).is_some() {
            removed = true;
        }
        if self.inner.lists.remove(key).is_some() {
            removed = true;
        }
        if self.inner.zsets.remove(key).is_some() {
            removed = true;
        }
        if self.inner.streams.remove(key).is_some() {
            removed = true;
        }
        removed
    }

    pub fn exists(&self, key: &str) -> bool {
        if let Some(entry) = self.inner.strings.get(key)
            && !entry.is_expired()
        {
            return true;
        }
        self.inner.hashes.contains_key(key)
            || self.inner.lists.contains_key(key)
            || self.inner.zsets.contains_key(key)
            || self.inner.streams.contains_key(key)
    }

    pub fn expire(&self, key: &str, ttl: Duration) -> bool {
        if let Some(mut entry) = self.inner.strings.get_mut(key) {
            entry.expires_at = Some(Instant::now() + ttl);
            return true;
        }
        false
    }

    pub fn ttl(&self, key: &str) -> i64 {
        match self.inner.strings.get(key) {
            Some(entry) => match entry.expires_at {
                Some(deadline) => {
                    let now = Instant::now();
                    if deadline <= now {
                        -2
                    } else {
                        (deadline - now).as_secs() as i64
                    }
                }
                None => -1,
            },
            None => -2,
        }
    }

    pub fn keys(&self, pattern: &str) -> Vec<String> {
        self.inner
            .strings
            .iter()
            .filter(|e| !e.is_expired())
            .map(|e| e.key().clone())
            .filter(|k| glob_match(pattern, k))
            .collect()
    }

    // ── HASH ───────────────────────────────────────────────────────────

    pub fn hset(&self, key: &str, field: impl Into<String>, value: impl Into<Vec<u8>>) -> bool {
        let mut entry = self.inner.hashes.entry(key.to_string()).or_default();
        entry.insert(field.into(), value.into()).is_none()
    }

    pub fn hget(&self, key: &str, field: &str) -> Option<Vec<u8>> {
        self.inner
            .hashes
            .get(key)
            .and_then(|m| m.get(field).cloned())
    }

    pub fn hgetall(&self, key: &str) -> Vec<(String, Vec<u8>)> {
        self.inner
            .hashes
            .get(key)
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default()
    }

    pub fn hdel(&self, key: &str, field: &str) -> bool {
        match self.inner.hashes.get_mut(key) {
            Some(mut m) => m.remove(field).is_some(),
            None => false,
        }
    }

    pub fn hlen(&self, key: &str) -> usize {
        self.inner.hashes.get(key).map(|m| m.len()).unwrap_or(0)
    }

    // ── LIST ───────────────────────────────────────────────────────────

    pub fn lpush(&self, key: &str, value: impl Into<Vec<u8>>) -> usize {
        let mut entry = self.inner.lists.entry(key.to_string()).or_default();
        entry.push_front(value.into());
        entry.len()
    }

    pub fn rpush(&self, key: &str, value: impl Into<Vec<u8>>) -> usize {
        let mut entry = self.inner.lists.entry(key.to_string()).or_default();
        entry.push_back(value.into());
        entry.len()
    }

    pub fn lpop(&self, key: &str) -> Option<Vec<u8>> {
        self.inner
            .lists
            .get_mut(key)
            .and_then(|mut q| q.pop_front())
    }

    pub fn rpop(&self, key: &str) -> Option<Vec<u8>> {
        self.inner.lists.get_mut(key).and_then(|mut q| q.pop_back())
    }

    pub fn lrange(&self, key: &str, start: i64, stop: i64) -> Vec<Vec<u8>> {
        let Some(list) = self.inner.lists.get(key) else {
            return Vec::new();
        };
        let len = list.len() as i64;
        if len == 0 {
            return Vec::new();
        }
        let norm = |i: i64| -> i64 {
            if i < 0 {
                (len + i).max(0)
            } else {
                i.min(len - 1)
            }
        };
        let s = norm(start);
        let e = norm(stop);
        if s > e {
            return Vec::new();
        }
        list.iter()
            .skip(s as usize)
            .take((e - s + 1) as usize)
            .cloned()
            .collect()
    }

    // ── ZSET ───────────────────────────────────────────────────────────

    pub fn zadd(&self, key: &str, score: f64, member: impl Into<String>) -> bool {
        let mut entry = self.inner.zsets.entry(key.to_string()).or_default();
        entry.add(score, member.into())
    }

    pub fn zrem(&self, key: &str, member: &str) -> bool {
        match self.inner.zsets.get_mut(key) {
            Some(mut z) => z.remove(member),
            None => false,
        }
    }

    pub fn zrank(&self, key: &str, member: &str) -> Option<usize> {
        self.inner.zsets.get(key).and_then(|z| z.rank(member))
    }

    pub fn zscore(&self, key: &str, member: &str) -> Option<f64> {
        self.inner.zsets.get(key).and_then(|z| z.score(member))
    }

    pub fn zrange(&self, key: &str, start: i64, stop: i64) -> Vec<(String, f64)> {
        self.inner
            .zsets
            .get(key)
            .map(|z| z.range(start, stop))
            .unwrap_or_default()
    }

    pub fn zrangebyscore(&self, key: &str, min: f64, max: f64) -> Vec<(String, f64)> {
        self.inner
            .zsets
            .get(key)
            .map(|z| z.range_by_score(min, max))
            .unwrap_or_default()
    }

    // ── STREAM ─────────────────────────────────────────────────────────

    pub fn xadd(&self, key: &str, fields: Vec<(String, String)>) -> String {
        let mut entry = self.inner.streams.entry(key.to_string()).or_default();
        let id = entry.next_id();
        entry.entries.push(StreamEntry {
            id: id.clone(),
            fields,
        });
        id
    }

    pub fn xrange(&self, key: &str, start: &str, end: &str) -> Vec<StreamEntry> {
        let Some(stream) = self.inner.streams.get(key) else {
            return Vec::new();
        };
        stream
            .entries
            .iter()
            .filter(|e| {
                let past_start = start == "-" || e.id.as_str() >= start;
                let before_end = end == "+" || e.id.as_str() <= end;
                past_start && before_end
            })
            .cloned()
            .collect()
    }

    pub fn xread(&self, key: &str, after_id: &str) -> Vec<StreamEntry> {
        let Some(stream) = self.inner.streams.get(key) else {
            return Vec::new();
        };
        stream
            .entries
            .iter()
            .filter(|e| e.id.as_str() > after_id)
            .cloned()
            .collect()
    }

    // ── BLOOM ──────────────────────────────────────────────────────────

    pub async fn bfadd(&self, key: &str, item: &[u8]) -> bool {
        let mut guard = self.inner.blooms.lock().await;
        guard
            .entry(key.to_string())
            .or_insert_with(BloomFilter::new)
            .add(item)
    }

    pub async fn bfexists(&self, key: &str, item: &[u8]) -> bool {
        let guard = self.inner.blooms.lock().await;
        guard.get(key).map(|b| b.contains(item)).unwrap_or(false)
    }

    // ── HLL ────────────────────────────────────────────────────────────

    pub async fn pfadd(&self, key: &str, item: &[u8]) -> bool {
        let mut guard = self.inner.hlls.lock().await;
        guard.entry(key.to_string()).or_default().add(item)
    }

    pub async fn pfcount(&self, key: &str) -> u64 {
        let guard = self.inner.hlls.lock().await;
        guard.get(key).map(|h| h.count()).unwrap_or(0)
    }

    // ── PUB/SUB ────────────────────────────────────────────────────────

    pub fn subscribe(&self, channel: &str) -> broadcast::Receiver<PubSubMessage> {
        let entry = self
            .inner
            .channels
            .entry(channel.to_string())
            .or_insert_with(|| broadcast::channel::<PubSubMessage>(1024).0);
        entry.subscribe()
    }

    pub fn publish(&self, channel: &str, payload: impl Into<Vec<u8>>) -> usize {
        match self.inner.channels.get(channel) {
            Some(sender) => {
                let msg = PubSubMessage {
                    channel: channel.to_string(),
                    payload: payload.into(),
                };
                sender.send(msg).unwrap_or(0)
            }
            None => 0,
        }
    }

    // ── Admin ──────────────────────────────────────────────────────────

    pub fn flush(&self) {
        self.inner.strings.clear();
        self.inner.hashes.clear();
        self.inner.lists.clear();
        self.inner.zsets.clear();
        self.inner.streams.clear();
    }

    pub fn stats(&self) -> DdbCacheStats {
        DdbCacheStats {
            strings: self.inner.strings.len() as u64,
            hashes: self.inner.hashes.len() as u64,
            lists: self.inner.lists.len() as u64,
            zsets: self.inner.zsets.len() as u64,
            streams: self.inner.streams.len() as u64,
            blooms: 0,
            hlls: 0,
            hits: self.inner.hits.load(Ordering::Relaxed),
            misses: self.inner.misses.load(Ordering::Relaxed),
            expired: self.inner.expired.load(Ordering::Relaxed),
            channels: self.inner.channels.len() as u64,
        }
    }

    pub fn info(&self) -> String {
        let s = self.stats();
        let mut out = String::new();
        out.push_str(&format!(
            "ddb_cache_version:{}\n",
            env!("CARGO_PKG_VERSION")
        ));
        out.push_str(&format!("strings:{}\n", s.strings));
        out.push_str(&format!("hashes:{}\n", s.hashes));
        out.push_str(&format!("lists:{}\n", s.lists));
        out.push_str(&format!("zsets:{}\n", s.zsets));
        out.push_str(&format!("streams:{}\n", s.streams));
        out.push_str(&format!("hits:{}\n", s.hits));
        out.push_str(&format!("misses:{}\n", s.misses));
        out.push_str(&format!("expired:{}\n", s.expired));
        out.push_str(&format!("channels:{}\n", s.channels));
        out
    }

    pub fn type_of(&self, key: &str) -> KeyType {
        if let Some(entry) = self.inner.strings.get(key)
            && !entry.is_expired()
        {
            return KeyType::String;
        }
        if self.inner.hashes.contains_key(key) {
            return KeyType::Hash;
        }
        if self.inner.lists.contains_key(key) {
            return KeyType::List;
        }
        if self.inner.zsets.contains_key(key) {
            return KeyType::ZSet;
        }
        if self.inner.streams.contains_key(key) {
            return KeyType::Stream;
        }
        KeyType::None
    }

    #[doc(hidden)]
    pub fn debug_string_snapshot(&self) -> BTreeMap<String, Vec<u8>> {
        self.inner
            .strings
            .iter()
            .filter(|e| !e.is_expired())
            .map(|e| (e.key().clone(), e.value().value.clone()))
            .collect()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum KeyType {
    None,
    String,
    Hash,
    List,
    ZSet,
    Stream,
}

// ---------------------------------------------------------------------------
// Glob matcher
// ---------------------------------------------------------------------------

/// Minimal glob matcher supporting `*` and `?`.
pub fn glob_match(pattern: &str, haystack: &str) -> bool {
    fn helper(p: &[u8], h: &[u8]) -> bool {
        let mut pi = 0usize;
        let mut hi = 0usize;
        let mut star: Option<(usize, usize)> = None;
        while hi < h.len() {
            if pi < p.len() && (p[pi] == b'?' || p[pi] == h[hi]) {
                pi += 1;
                hi += 1;
            } else if pi < p.len() && p[pi] == b'*' {
                star = Some((pi, hi));
                pi += 1;
            } else if let Some((sp, sh)) = star {
                pi = sp + 1;
                hi = sh + 1;
                star = Some((sp, sh + 1));
            } else {
                return false;
            }
        }
        while pi < p.len() && p[pi] == b'*' {
            pi += 1;
        }
        pi == p.len()
    }
    helper(pattern.as_bytes(), haystack.as_bytes())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_roundtrip_and_ttl() {
        let cache = DdbCache::new();
        cache.set("k", b"v".to_vec(), None);
        assert_eq!(cache.get("k"), Some(b"v".to_vec()));
        assert!(cache.exists("k"));
        assert_eq!(cache.ttl("k"), -1);

        cache.set("t", b"w".to_vec(), Some(Duration::from_secs(60)));
        assert!(cache.ttl("t") >= 0);
        assert!(cache.del("t"));
        assert_eq!(cache.get("t"), None);
    }

    #[test]
    fn hash_ops() {
        let cache = DdbCache::new();
        assert!(cache.hset("h", "f", b"v".to_vec()));
        assert_eq!(cache.hget("h", "f"), Some(b"v".to_vec()));
        assert_eq!(cache.hlen("h"), 1);
        let all = cache.hgetall("h");
        assert_eq!(all.len(), 1);
        assert!(cache.hdel("h", "f"));
    }

    #[test]
    fn list_push_pop_range() {
        let cache = DdbCache::new();
        cache.rpush("l", b"a".to_vec());
        cache.rpush("l", b"b".to_vec());
        cache.lpush("l", b"z".to_vec());
        assert_eq!(cache.lrange("l", 0, -1).len(), 3);
        assert_eq!(cache.lpop("l"), Some(b"z".to_vec()));
        assert_eq!(cache.rpop("l"), Some(b"b".to_vec()));
    }

    #[test]
    fn zset_ordering() {
        let cache = DdbCache::new();
        cache.zadd("z", 2.0, "b");
        cache.zadd("z", 1.0, "a");
        cache.zadd("z", 3.0, "c");
        let r = cache.zrange("z", 0, -1);
        assert_eq!(r[0].0, "a");
        assert_eq!(r[2].0, "c");
        assert_eq!(cache.zrank("z", "b"), Some(1));
        assert_eq!(cache.zscore("z", "c"), Some(3.0));
        let by_score = cache.zrangebyscore("z", 1.5, 2.5);
        assert_eq!(by_score.len(), 1);
        assert_eq!(by_score[0].0, "b");
    }

    #[test]
    fn keys_glob_match() {
        let cache = DdbCache::new();
        cache.set("user:1", b"".to_vec(), None);
        cache.set("user:2", b"".to_vec(), None);
        cache.set("other", b"".to_vec(), None);
        let mut keys = cache.keys("user:*");
        keys.sort();
        assert_eq!(keys, vec!["user:1".to_string(), "user:2".to_string()]);
    }

    #[test]
    fn pubsub_delivery() {
        let cache = DdbCache::new();
        let mut rx = cache.subscribe("news");
        let n = cache.publish("news", b"hello".to_vec());
        assert!(n >= 1);
        let msg = rx.try_recv().unwrap();
        assert_eq!(msg.channel, "news");
        assert_eq!(msg.payload, b"hello".to_vec());
    }

    #[test]
    fn stream_xadd_xrange() {
        let cache = DdbCache::new();
        let id1 = cache.xadd("s", vec![("k".into(), "v".into())]);
        let id2 = cache.xadd("s", vec![("k2".into(), "v2".into())]);
        assert_ne!(id1, id2);
        let all = cache.xrange("s", "-", "+");
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn bloom_and_hll() {
        let cache = DdbCache::new();
        assert!(cache.bfadd("b", b"x").await);
        assert!(cache.bfexists("b", b"x").await);
        assert!(!cache.bfexists("b", b"never").await);

        cache.pfadd("h", b"a").await;
        cache.pfadd("h", b"b").await;
        cache.pfadd("h", b"a").await;
        assert_eq!(cache.pfcount("h").await, 2);
    }
}
