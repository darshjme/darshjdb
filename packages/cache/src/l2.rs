// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
// l2: L2 cache tier — Postgres-backed persistent cache with adaptive
// (lz4 / zstd) compression, hash / list / zset value layouts, and an
// append-only stream model (XADD / XRANGE / XREAD / XLEN).
//
// Compression policy:
//   * payloads <  1 KiB           → lz4_flex   (low-CPU, low-latency)
//   * payloads >= 1 KiB           → zstd lvl 3 (high ratio, still fast)
// A single byte tag is prefixed to every stored payload so that reads know
// which decoder to use without consulting metadata:
//   0x01 = raw, 0x02 = lz4_flex, 0x03 = zstd(level=3)
//
// All SQL is parameterized via sqlx's `query!` / `query_as!`-free runtime
// builders — no string interpolation of user data, ever.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sqlx::PgPool;
use sqlx::Row;
use sqlx::postgres::PgRow;
use thiserror::Error;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

// ─── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum L2Error {
    #[error("sqlx: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("serde_json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("compression: {0}")]
    Compression(String),
    #[error("decompression: {0}")]
    Decompression(String),
    #[error("invalid payload tag: {0:#x}")]
    InvalidTag(u8),
    #[error("type mismatch: key {key} stored as {actual}, requested {expected}")]
    TypeMismatch {
        key: String,
        actual: String,
        expected: &'static str,
    },
    #[error("invalid stream id: {0}")]
    InvalidStreamId(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}

pub type L2Result<T> = Result<T, L2Error>;

// ─── Compression tags ────────────────────────────────────────────────────────

const TAG_RAW: u8 = 0x01;
const TAG_LZ4: u8 = 0x02;
const TAG_ZSTD: u8 = 0x03;

const COMPRESSION_THRESHOLD: usize = 1024; // bytes
const ZSTD_LEVEL: i32 = 3;

fn encode(payload: &[u8]) -> L2Result<Vec<u8>> {
    if payload.is_empty() {
        let mut out = Vec::with_capacity(1);
        out.push(TAG_RAW);
        return Ok(out);
    }
    if payload.len() < COMPRESSION_THRESHOLD {
        let compressed = lz4_flex::compress_prepend_size(payload);
        let mut out = Vec::with_capacity(compressed.len() + 1);
        out.push(TAG_LZ4);
        out.extend_from_slice(&compressed);
        Ok(out)
    } else {
        let compressed =
            zstd::stream::encode_all(payload, ZSTD_LEVEL).map_err(|e| L2Error::Compression(e.to_string()))?;
        let mut out = Vec::with_capacity(compressed.len() + 1);
        out.push(TAG_ZSTD);
        out.extend_from_slice(&compressed);
        Ok(out)
    }
}

fn decode(blob: &[u8]) -> L2Result<Vec<u8>> {
    if blob.is_empty() {
        return Ok(Vec::new());
    }
    let tag = blob[0];
    let body = &blob[1..];
    match tag {
        TAG_RAW => Ok(body.to_vec()),
        TAG_LZ4 => lz4_flex::decompress_size_prepended(body)
            .map_err(|e| L2Error::Decompression(e.to_string())),
        TAG_ZSTD => zstd::stream::decode_all(body).map_err(|e| L2Error::Decompression(e.to_string())),
        other => Err(L2Error::InvalidTag(other)),
    }
}

// ─── Kinds ───────────────────────────────────────────────────────────────────

const KIND_STRING: &str = "string";
const KIND_HASH: &str = "hash";
const KIND_LIST: &str = "list";
const KIND_ZSET: &str = "zset";

// ─── Stream entry ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StreamEntry {
    pub id: String,
    pub fields: HashMap<String, String>,
}

// ─── L2Cache ─────────────────────────────────────────────────────────────────

/// L2Cache — Postgres-backed persistent cache tier.
///
/// Construct via [`L2Cache::new`] with a shared `Arc<PgPool>`. Use
/// [`L2Cache::start_expiry_sweeper`] to launch the background TTL reaper task.
#[derive(Clone)]
pub struct L2Cache {
    pool: Arc<PgPool>,
    /// Monotonically-increasing per-process counter used as the sequence half
    /// of stream entry ids, so concurrent XADDs in the same millisecond never
    /// collide.
    seq: Arc<AtomicU64>,
}

impl L2Cache {
    pub fn new(pool: Arc<PgPool>) -> Self {
        Self {
            pool,
            seq: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    // ── Internal helpers ───────────────────────────────────────────────────

    fn expires_at(ttl: Option<Duration>) -> Option<DateTime<Utc>> {
        ttl.map(|d| Utc::now() + chrono::Duration::from_std(d).unwrap_or_else(|_| chrono::Duration::seconds(0)))
    }

    async fn upsert(
        &self,
        key: &str,
        kind: &str,
        raw: &[u8],
        expires_at: Option<DateTime<Utc>>,
    ) -> L2Result<()> {
        let value = encode(raw)?;
        let size = raw.len() as i32;
        sqlx::query(
            r#"
            INSERT INTO kv_store (key, value, kind, expires_at, size_bytes, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, now(), now())
            ON CONFLICT (key) DO UPDATE
              SET value      = EXCLUDED.value,
                  kind       = EXCLUDED.kind,
                  expires_at = EXCLUDED.expires_at,
                  size_bytes = EXCLUDED.size_bytes,
                  updated_at = now()
            "#,
        )
        .bind(key)
        .bind(value)
        .bind(kind)
        .bind(expires_at)
        .bind(size)
        .execute(&*self.pool)
        .await?;
        Ok(())
    }

    async fn fetch(&self, key: &str, expected_kind: &'static str) -> L2Result<Option<Vec<u8>>> {
        let row: Option<PgRow> = sqlx::query(
            r#"
            SELECT value, kind, expires_at
              FROM kv_store
             WHERE key = $1
               AND (expires_at IS NULL OR expires_at > now())
            "#,
        )
        .bind(key)
        .fetch_optional(&*self.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };
        let kind: String = row.try_get("kind")?;
        if kind != expected_kind {
            return Err(L2Error::TypeMismatch {
                key: key.to_string(),
                actual: kind,
                expected: expected_kind,
            });
        }
        let blob: Vec<u8> = row.try_get("value")?;
        Ok(Some(decode(&blob)?))
    }

    // ── Generic key ops ────────────────────────────────────────────────────

    pub async fn set(&self, key: &str, value: &[u8], ttl: Option<Duration>) -> L2Result<()> {
        self.upsert(key, KIND_STRING, value, Self::expires_at(ttl)).await
    }

    pub async fn get(&self, key: &str) -> L2Result<Option<Vec<u8>>> {
        self.fetch(key, KIND_STRING).await
    }

    pub async fn del(&self, key: &str) -> L2Result<bool> {
        let res = sqlx::query("DELETE FROM kv_store WHERE key = $1")
            .bind(key)
            .execute(&*self.pool)
            .await?;
        // also nuke any stream rows under this key
        sqlx::query("DELETE FROM kv_streams WHERE stream_key = $1")
            .bind(key)
            .execute(&*self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn exists(&self, key: &str) -> L2Result<bool> {
        let row: Option<PgRow> = sqlx::query(
            r#"
            SELECT 1 AS hit
              FROM kv_store
             WHERE key = $1
               AND (expires_at IS NULL OR expires_at > now())
            "#,
        )
        .bind(key)
        .fetch_optional(&*self.pool)
        .await?;
        Ok(row.is_some())
    }

    pub async fn expire(&self, key: &str, ttl: Duration) -> L2Result<bool> {
        let new_expiry = Utc::now()
            + chrono::Duration::from_std(ttl).unwrap_or_else(|_| chrono::Duration::seconds(0));
        let res = sqlx::query(
            "UPDATE kv_store SET expires_at = $2, updated_at = now() WHERE key = $1",
        )
        .bind(key)
        .bind(new_expiry)
        .execute(&*self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Returns remaining TTL in seconds, or:
    ///   * `Ok(None)` if the key does not exist,
    ///   * `Ok(Some(-1))` if the key exists with no TTL (persist),
    ///   * `Ok(Some(n))` for n >= 0 seconds remaining.
    pub async fn ttl(&self, key: &str) -> L2Result<Option<i64>> {
        let row: Option<PgRow> = sqlx::query(
            r#"
            SELECT expires_at
              FROM kv_store
             WHERE key = $1
               AND (expires_at IS NULL OR expires_at > now())
            "#,
        )
        .bind(key)
        .fetch_optional(&*self.pool)
        .await?;
        match row {
            None => Ok(None),
            Some(r) => {
                let exp: Option<DateTime<Utc>> = r.try_get("expires_at")?;
                match exp {
                    None => Ok(Some(-1)),
                    Some(t) => {
                        let secs = (t - Utc::now()).num_seconds().max(0);
                        Ok(Some(secs))
                    }
                }
            }
        }
    }

    /// SQL `LIKE` pattern walk. `%` and `_` are honored; pass `"*"` to mean
    /// "all keys" (translated to `%`).
    pub async fn keys(&self, pattern: &str) -> L2Result<Vec<String>> {
        let like = if pattern == "*" {
            "%".to_string()
        } else {
            pattern.replace('*', "%").replace('?', "_")
        };
        let rows = sqlx::query(
            r#"
            SELECT key
              FROM kv_store
             WHERE key LIKE $1
               AND (expires_at IS NULL OR expires_at > now())
             ORDER BY key
            "#,
        )
        .bind(like)
        .fetch_all(&*self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| r.try_get::<String, _>("key").unwrap_or_default())
            .collect())
    }

    // ── Hash ops (HSET / HGET / HGETALL) ───────────────────────────────────
    //
    // Hash payloads are stored as a single JSON object inside `kv_store.value`
    // (encoded by the same compression pipeline as strings). HSET reads the
    // current map, mutates, writes back inside a single transaction.

    pub async fn hset(&self, key: &str, field: &str, value: &str) -> L2Result<bool> {
        let mut tx = self.pool.begin().await?;
        let row: Option<PgRow> = sqlx::query(
            r#"
            SELECT value, kind FROM kv_store WHERE key = $1 FOR UPDATE
            "#,
        )
        .bind(key)
        .fetch_optional(&mut *tx)
        .await?;

        let mut map: HashMap<String, String> = match row {
            Some(r) => {
                let kind: String = r.try_get("kind")?;
                if kind != KIND_HASH {
                    return Err(L2Error::TypeMismatch {
                        key: key.to_string(),
                        actual: kind,
                        expected: KIND_HASH,
                    });
                }
                let blob: Vec<u8> = r.try_get("value")?;
                let raw = decode(&blob)?;
                serde_json::from_slice(&raw).unwrap_or_default()
            }
            None => HashMap::new(),
        };
        let is_new = !map.contains_key(field);
        map.insert(field.to_string(), value.to_string());

        let raw = serde_json::to_vec(&map)?;
        let value_blob = encode(&raw)?;
        let size = raw.len() as i32;
        sqlx::query(
            r#"
            INSERT INTO kv_store (key, value, kind, size_bytes, created_at, updated_at)
            VALUES ($1, $2, $3, $4, now(), now())
            ON CONFLICT (key) DO UPDATE
              SET value      = EXCLUDED.value,
                  kind       = EXCLUDED.kind,
                  size_bytes = EXCLUDED.size_bytes,
                  updated_at = now()
            "#,
        )
        .bind(key)
        .bind(value_blob)
        .bind(KIND_HASH)
        .bind(size)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(is_new)
    }

    pub async fn hget(&self, key: &str, field: &str) -> L2Result<Option<String>> {
        let map = self.hgetall(key).await?;
        Ok(map.get(field).cloned())
    }

    pub async fn hgetall(&self, key: &str) -> L2Result<HashMap<String, String>> {
        match self.fetch(key, KIND_HASH).await? {
            None => Ok(HashMap::new()),
            Some(raw) => Ok(serde_json::from_slice(&raw).unwrap_or_default()),
        }
    }

    // ── List ops (LPUSH / RPUSH / LRANGE) ──────────────────────────────────

    async fn list_load(&self, key: &str, tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> L2Result<Vec<String>> {
        let row: Option<PgRow> = sqlx::query(
            r#"SELECT value, kind FROM kv_store WHERE key = $1 FOR UPDATE"#,
        )
        .bind(key)
        .fetch_optional(&mut **tx)
        .await?;

        match row {
            None => Ok(Vec::new()),
            Some(r) => {
                let kind: String = r.try_get("kind")?;
                if kind != KIND_LIST {
                    return Err(L2Error::TypeMismatch {
                        key: key.to_string(),
                        actual: kind,
                        expected: KIND_LIST,
                    });
                }
                let blob: Vec<u8> = r.try_get("value")?;
                let raw = decode(&blob)?;
                Ok(serde_json::from_slice(&raw).unwrap_or_default())
            }
        }
    }

    async fn list_store(
        &self,
        key: &str,
        list: &[String],
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> L2Result<()> {
        let raw = serde_json::to_vec(list)?;
        let blob = encode(&raw)?;
        let size = raw.len() as i32;
        sqlx::query(
            r#"
            INSERT INTO kv_store (key, value, kind, size_bytes, created_at, updated_at)
            VALUES ($1, $2, $3, $4, now(), now())
            ON CONFLICT (key) DO UPDATE
              SET value      = EXCLUDED.value,
                  kind       = EXCLUDED.kind,
                  size_bytes = EXCLUDED.size_bytes,
                  updated_at = now()
            "#,
        )
        .bind(key)
        .bind(blob)
        .bind(KIND_LIST)
        .bind(size)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    pub async fn lpush(&self, key: &str, value: &str) -> L2Result<usize> {
        let mut tx = self.pool.begin().await?;
        let mut list = self.list_load(key, &mut tx).await?;
        list.insert(0, value.to_string());
        let len = list.len();
        self.list_store(key, &list, &mut tx).await?;
        tx.commit().await?;
        Ok(len)
    }

    pub async fn rpush(&self, key: &str, value: &str) -> L2Result<usize> {
        let mut tx = self.pool.begin().await?;
        let mut list = self.list_load(key, &mut tx).await?;
        list.push(value.to_string());
        let len = list.len();
        self.list_store(key, &list, &mut tx).await?;
        tx.commit().await?;
        Ok(len)
    }

    /// Inclusive on both ends. Negative indices count from the tail
    /// (Redis-compatible). Returns an empty vec if the key does not exist.
    pub async fn lrange(&self, key: &str, start: i64, stop: i64) -> L2Result<Vec<String>> {
        let raw = match self.fetch(key, KIND_LIST).await? {
            None => return Ok(Vec::new()),
            Some(r) => r,
        };
        let list: Vec<String> = serde_json::from_slice(&raw).unwrap_or_default();
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

    // ── Sorted set ops (ZADD / ZRANGE) ─────────────────────────────────────
    //
    // Stored as `Vec<(f64, String)>` JSON-serialized; reads sort on demand.

    pub async fn zadd(&self, key: &str, score: f64, member: &str) -> L2Result<bool> {
        let mut tx = self.pool.begin().await?;
        let row: Option<PgRow> = sqlx::query(
            r#"SELECT value, kind FROM kv_store WHERE key = $1 FOR UPDATE"#,
        )
        .bind(key)
        .fetch_optional(&mut *tx)
        .await?;

        let mut zset: Vec<(f64, String)> = match row {
            Some(r) => {
                let kind: String = r.try_get("kind")?;
                if kind != KIND_ZSET {
                    return Err(L2Error::TypeMismatch {
                        key: key.to_string(),
                        actual: kind,
                        expected: KIND_ZSET,
                    });
                }
                let blob: Vec<u8> = r.try_get("value")?;
                let raw = decode(&blob)?;
                serde_json::from_slice(&raw).unwrap_or_default()
            }
            None => Vec::new(),
        };
        let mut added = true;
        if let Some(pos) = zset.iter().position(|(_, m)| m == member) {
            zset[pos].0 = score;
            added = false;
        } else {
            zset.push((score, member.to_string()));
        }

        let raw = serde_json::to_vec(&zset)?;
        let blob = encode(&raw)?;
        let size = raw.len() as i32;
        sqlx::query(
            r#"
            INSERT INTO kv_store (key, value, kind, size_bytes, created_at, updated_at)
            VALUES ($1, $2, $3, $4, now(), now())
            ON CONFLICT (key) DO UPDATE
              SET value      = EXCLUDED.value,
                  kind       = EXCLUDED.kind,
                  size_bytes = EXCLUDED.size_bytes,
                  updated_at = now()
            "#,
        )
        .bind(key)
        .bind(blob)
        .bind(KIND_ZSET)
        .bind(size)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(added)
    }

    /// Returns members in ascending score order, inclusive on both indices.
    pub async fn zrange(&self, key: &str, start: i64, stop: i64) -> L2Result<Vec<String>> {
        let raw = match self.fetch(key, KIND_ZSET).await? {
            None => return Ok(Vec::new()),
            Some(r) => r,
        };
        let mut zset: Vec<(f64, String)> = serde_json::from_slice(&raw).unwrap_or_default();
        zset.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
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

    // ── Stream ops (XADD / XREAD / XRANGE / XLEN) ──────────────────────────

    /// XADD — appends a new entry. Returns the assigned `<ms>-<seq>` id.
    pub async fn xadd(&self, stream_key: &str, fields: &HashMap<String, String>) -> L2Result<String> {
        let unix_ms = Utc::now().timestamp_millis().max(0) as u64;
        let seq = self.seq.fetch_add(1, Ordering::SeqCst);
        let entry_id = format!("{}-{}", unix_ms, seq);

        let json: JsonValue = serde_json::to_value(fields)?;
        sqlx::query(
            r#"
            INSERT INTO kv_streams (stream_key, entry_id, fields, created_at)
            VALUES ($1, $2, $3, now())
            "#,
        )
        .bind(stream_key)
        .bind(&entry_id)
        .bind(json)
        .execute(&*self.pool)
        .await?;
        Ok(entry_id)
    }

    /// XLEN — total entry count for a stream.
    pub async fn xlen(&self, stream_key: &str) -> L2Result<i64> {
        let row = sqlx::query(
            "SELECT COUNT(*)::BIGINT AS n FROM kv_streams WHERE stream_key = $1",
        )
        .bind(stream_key)
        .fetch_one(&*self.pool)
        .await?;
        Ok(row.try_get::<i64, _>("n")?)
    }

    /// XRANGE — inclusive scan over `[start_id, end_id]`. Use `"-"` and `"+"`
    /// for the open-ended ends.
    pub async fn xrange(
        &self,
        stream_key: &str,
        start_id: &str,
        end_id: &str,
    ) -> L2Result<Vec<StreamEntry>> {
        // Ordering by created_at ASC then entry_id ASC keeps results stable
        // even when many entries land in the same millisecond.
        let rows = if start_id == "-" && end_id == "+" {
            sqlx::query(
                r#"
                SELECT entry_id, fields
                  FROM kv_streams
                 WHERE stream_key = $1
                 ORDER BY created_at ASC, entry_id ASC
                "#,
            )
            .bind(stream_key)
            .fetch_all(&*self.pool)
            .await?
        } else if start_id == "-" {
            sqlx::query(
                r#"
                SELECT entry_id, fields
                  FROM kv_streams
                 WHERE stream_key = $1 AND entry_id <= $2
                 ORDER BY created_at ASC, entry_id ASC
                "#,
            )
            .bind(stream_key)
            .bind(end_id)
            .fetch_all(&*self.pool)
            .await?
        } else if end_id == "+" {
            sqlx::query(
                r#"
                SELECT entry_id, fields
                  FROM kv_streams
                 WHERE stream_key = $1 AND entry_id >= $2
                 ORDER BY created_at ASC, entry_id ASC
                "#,
            )
            .bind(stream_key)
            .bind(start_id)
            .fetch_all(&*self.pool)
            .await?
        } else {
            sqlx::query(
                r#"
                SELECT entry_id, fields
                  FROM kv_streams
                 WHERE stream_key = $1 AND entry_id >= $2 AND entry_id <= $3
                 ORDER BY created_at ASC, entry_id ASC
                "#,
            )
            .bind(stream_key)
            .bind(start_id)
            .bind(end_id)
            .fetch_all(&*self.pool)
            .await?
        };

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let id: String = row.try_get("entry_id")?;
            let fields_json: JsonValue = row.try_get("fields")?;
            let fields: HashMap<String, String> = serde_json::from_value(fields_json).unwrap_or_default();
            out.push(StreamEntry { id, fields });
        }
        Ok(out)
    }

    /// XREAD — read up to `count` entries with ids strictly greater than
    /// `last_id`. Pass `"0"` (or `"0-0"`) to read from the beginning.
    pub async fn xread(
        &self,
        stream_key: &str,
        last_id: &str,
        count: i64,
    ) -> L2Result<Vec<StreamEntry>> {
        if count <= 0 {
            return Err(L2Error::InvalidArgument("count must be > 0".into()));
        }
        let rows = sqlx::query(
            r#"
            SELECT entry_id, fields
              FROM kv_streams
             WHERE stream_key = $1
               AND entry_id > $2
             ORDER BY created_at ASC, entry_id ASC
             LIMIT $3
            "#,
        )
        .bind(stream_key)
        .bind(last_id)
        .bind(count)
        .fetch_all(&*self.pool)
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let id: String = row.try_get("entry_id")?;
            let fields_json: JsonValue = row.try_get("fields")?;
            let fields: HashMap<String, String> = serde_json::from_value(fields_json).unwrap_or_default();
            out.push(StreamEntry { id, fields });
        }
        Ok(out)
    }

    // ── Background expiry sweep ────────────────────────────────────────────

    /// Spawns a tokio task that, every 60 seconds, deletes up to 1000 expired
    /// rows from `kv_store`. Returns the `JoinHandle` so callers can abort
    /// during shutdown.
    pub fn start_expiry_sweeper(self: &Arc<Self>) -> JoinHandle<()> {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(60));
            // Skip the immediate tick — let the system warm up first.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                match this.sweep_expired_once(1000).await {
                    Ok(0) => debug!(target: "ddb_cache::l2", "expiry sweep: 0 rows"),
                    Ok(n) => info!(target: "ddb_cache::l2", removed = n, "expiry sweep deleted rows"),
                    Err(e) => {
                        error!(target: "ddb_cache::l2", error = %e, "expiry sweep failed");
                        // Brief backoff so a flapping DB doesn't burn the loop.
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
            }
        })
    }

    /// One-shot expiry sweep. Returns the number of rows deleted.
    pub async fn sweep_expired_once(&self, limit: i64) -> L2Result<u64> {
        let res = sqlx::query(
            r#"
            DELETE FROM kv_store
             WHERE key IN (
                 SELECT key
                   FROM kv_store
                  WHERE expires_at IS NOT NULL
                    AND expires_at < now()
                  LIMIT $1
             )
            "#,
        )
        .bind(limit)
        .execute(&*self.pool)
        .await?;
        let n = res.rows_affected();
        if n > 0 {
            warn!(target: "ddb_cache::l2", reaped = n, "expired keys removed");
        }
        Ok(n)
    }
}

#[cfg(test)]
mod codec_tests {
    use super::*;

    #[test]
    fn roundtrip_small_uses_lz4() {
        let payload = b"hello, ddb cache";
        let encoded = encode(payload).unwrap();
        assert_eq!(encoded[0], TAG_LZ4);
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn roundtrip_large_uses_zstd() {
        let payload = vec![b'X'; 4096];
        let encoded = encode(&payload).unwrap();
        assert_eq!(encoded[0], TAG_ZSTD);
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn empty_payload_is_raw() {
        let encoded = encode(b"").unwrap();
        assert_eq!(encoded, vec![TAG_RAW]);
        let decoded = decode(&encoded).unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn invalid_tag_errors() {
        let bad = [0xFFu8, 1, 2, 3];
        let err = decode(&bad).unwrap_err();
        matches!(err, L2Error::InvalidTag(0xFF));
    }
}
