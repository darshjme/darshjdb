// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
// ddb-cache-server :: http — HTTP REST cache API exposed at
// `/api/cache/*`. This router is designed to be mounted on the existing
// Axum router in `packages/server` via `.merge(cache_http_router(cache))`,
// matching the pattern used by `webhook_routes`/`api_key_routes` (a
// self-stated sub-router).
//
// Routes (Slice 11 Part B):
//
//   GET    /api/cache/:key                   → value (404 if missing)
//   PUT    /api/cache/:key                   → {value, ttl_seconds?}
//   DELETE /api/cache/:key                   → 204
//   GET    /api/cache/:key/ttl               → {ttl_seconds}
//   POST   /api/cache/:key/expire            → {ttl_seconds}
//   GET    /api/cache/keys?pattern=…         → [keys]
//
//   POST   /api/cache/hash/:key              → {field, value}
//   GET    /api/cache/hash/:key              → {field: value, …}
//
//   POST   /api/cache/list/:key/push         → {side, values}
//   GET    /api/cache/list/:key?start=&stop= → [values]
//
//   POST   /api/cache/zset/:key              → {score, member}
//   GET    /api/cache/zset/:key              → [{member, score}]
//
//   GET    /api/cache/stats                  → DdbCacheStats

use std::sync::Arc;
use std::time::Duration;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
};
use ddb_cache::{DdbCache, DdbCacheStats};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone)]
pub struct CacheHttpState {
    pub cache: Arc<DdbCache>,
}

/// Build the `/api/cache/*` sub-router. Callers can `.merge(..)` this
/// directly into the main `build_router` output — it is self-stated so
/// it does not need the surrounding `AppState`.
pub fn cache_http_router(cache: Arc<DdbCache>) -> Router {
    let state = CacheHttpState { cache };
    Router::new()
        .route("/api/cache/stats", get(get_stats))
        .route("/api/cache/keys", get(list_keys))
        .route(
            "/api/cache/{key}",
            get(get_value).put(put_value).delete(delete_value),
        )
        .route("/api/cache/{key}/ttl", get(get_ttl))
        .route("/api/cache/{key}/expire", post(post_expire))
        .route(
            "/api/cache/hash/{key}",
            post(hash_set).get(hash_get_all),
        )
        .route("/api/cache/list/{key}/push", post(list_push))
        .route("/api/cache/list/{key}", get(list_range))
        .route(
            "/api/cache/zset/{key}",
            post(zset_add).get(zset_range),
        )
        // Register delete/put aliases redundantly for clarity in tests.
        .route("/api/cache/{key}/delete", delete(delete_value))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// String handlers
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct PutValueBody {
    pub value: Value,
    pub ttl_seconds: Option<u64>,
}

async fn get_value(
    State(state): State<CacheHttpState>,
    Path(key): Path<String>,
) -> impl IntoResponse {
    match state.cache.get(&key) {
        Some(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(v) => (StatusCode::OK, Json(json!({ "key": key, "value": v }))).into_response(),
            Err(_) => (
                StatusCode::OK,
                Json(json!({ "key": key, "value": String::from_utf8_lossy(&bytes).to_string() })),
            )
                .into_response(),
        },
        None => (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response(),
    }
}

async fn put_value(
    State(state): State<CacheHttpState>,
    Path(key): Path<String>,
    Json(body): Json<PutValueBody>,
) -> impl IntoResponse {
    let bytes = serde_json::to_vec(&body.value).unwrap_or_else(|_| b"null".to_vec());
    let ttl = body.ttl_seconds.map(Duration::from_secs);
    state.cache.set(key.clone(), bytes, ttl);
    (StatusCode::OK, Json(json!({ "key": key, "status": "ok" })))
}

async fn delete_value(
    State(state): State<CacheHttpState>,
    Path(key): Path<String>,
) -> impl IntoResponse {
    let removed = state.cache.del(&key);
    (StatusCode::OK, Json(json!({ "key": key, "removed": removed })))
}

async fn get_ttl(
    State(state): State<CacheHttpState>,
    Path(key): Path<String>,
) -> impl IntoResponse {
    let ttl = state.cache.ttl(&key);
    (StatusCode::OK, Json(json!({ "key": key, "ttl_seconds": ttl })))
}

#[derive(Debug, Deserialize)]
pub struct ExpireBody {
    pub ttl_seconds: u64,
}

async fn post_expire(
    State(state): State<CacheHttpState>,
    Path(key): Path<String>,
    Json(body): Json<ExpireBody>,
) -> impl IntoResponse {
    let ok = state.cache.expire(&key, Duration::from_secs(body.ttl_seconds));
    (StatusCode::OK, Json(json!({ "key": key, "applied": ok })))
}

#[derive(Debug, Deserialize)]
pub struct KeysQuery {
    pub pattern: Option<String>,
}

async fn list_keys(
    State(state): State<CacheHttpState>,
    Query(q): Query<KeysQuery>,
) -> impl IntoResponse {
    let pattern = q.pattern.unwrap_or_else(|| "*".into());
    let keys = state.cache.keys(&pattern);
    (StatusCode::OK, Json(json!({ "pattern": pattern, "keys": keys })))
}

// ---------------------------------------------------------------------------
// Hash handlers
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct HashSetBody {
    pub field: String,
    pub value: Value,
}

async fn hash_set(
    State(state): State<CacheHttpState>,
    Path(key): Path<String>,
    Json(body): Json<HashSetBody>,
) -> impl IntoResponse {
    let bytes = serde_json::to_vec(&body.value).unwrap_or_else(|_| b"null".to_vec());
    let added = state.cache.hset(&key, body.field.clone(), bytes);
    (
        StatusCode::OK,
        Json(json!({ "key": key, "field": body.field, "added": added })),
    )
}

async fn hash_get_all(
    State(state): State<CacheHttpState>,
    Path(key): Path<String>,
) -> impl IntoResponse {
    let pairs = state.cache.hgetall(&key);
    let map: serde_json::Map<String, Value> = pairs
        .into_iter()
        .map(|(f, v)| {
            let decoded = serde_json::from_slice::<Value>(&v)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&v).to_string()));
            (f, decoded)
        })
        .collect();
    (StatusCode::OK, Json(json!({ "key": key, "fields": map })))
}

// ---------------------------------------------------------------------------
// List handlers
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ListPushBody {
    pub side: String, // "L" or "R"
    pub values: Vec<Value>,
}

async fn list_push(
    State(state): State<CacheHttpState>,
    Path(key): Path<String>,
    Json(body): Json<ListPushBody>,
) -> impl IntoResponse {
    let left = body.side.eq_ignore_ascii_case("L");
    let mut len = 0usize;
    for v in body.values {
        let bytes = serde_json::to_vec(&v).unwrap_or_else(|_| b"null".to_vec());
        len = if left {
            state.cache.lpush(&key, bytes)
        } else {
            state.cache.rpush(&key, bytes)
        };
    }
    (
        StatusCode::OK,
        Json(json!({ "key": key, "length": len, "side": body.side })),
    )
}

#[derive(Debug, Deserialize)]
pub struct ListRangeQuery {
    pub start: Option<i64>,
    pub stop: Option<i64>,
}

async fn list_range(
    State(state): State<CacheHttpState>,
    Path(key): Path<String>,
    Query(q): Query<ListRangeQuery>,
) -> impl IntoResponse {
    let start = q.start.unwrap_or(0);
    let stop = q.stop.unwrap_or(-1);
    let values: Vec<Value> = state
        .cache
        .lrange(&key, start, stop)
        .into_iter()
        .map(|v| {
            serde_json::from_slice::<Value>(&v)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&v).to_string()))
        })
        .collect();
    (StatusCode::OK, Json(json!({ "key": key, "values": values })))
}

// ---------------------------------------------------------------------------
// ZSet handlers
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ZSetAddBody {
    pub score: f64,
    pub member: String,
}

async fn zset_add(
    State(state): State<CacheHttpState>,
    Path(key): Path<String>,
    Json(body): Json<ZSetAddBody>,
) -> impl IntoResponse {
    let added = state.cache.zadd(&key, body.score, body.member.clone());
    (
        StatusCode::OK,
        Json(json!({ "key": key, "member": body.member, "added": added })),
    )
}

#[derive(Debug, Serialize)]
pub struct ZSetMemberView {
    pub member: String,
    pub score: f64,
}

async fn zset_range(
    State(state): State<CacheHttpState>,
    Path(key): Path<String>,
) -> impl IntoResponse {
    let items: Vec<ZSetMemberView> = state
        .cache
        .zrange(&key, 0, -1)
        .into_iter()
        .map(|(m, s)| ZSetMemberView { member: m, score: s })
        .collect();
    (StatusCode::OK, Json(json!({ "key": key, "members": items })))
}

// ---------------------------------------------------------------------------
// Stats handler
// ---------------------------------------------------------------------------

async fn get_stats(State(state): State<CacheHttpState>) -> impl IntoResponse {
    let stats: DdbCacheStats = state.cache.stats();
    (StatusCode::OK, Json(stats))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    async fn body_json(resp: axum::http::Response<Body>) -> Value {
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn http_put_then_get_roundtrip() {
        let cache = Arc::new(DdbCache::new());
        let app = cache_http_router(cache.clone());

        // PUT /api/cache/foo
        let put_body = json!({ "value": "bar", "ttl_seconds": null });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/cache/foo")
                    .header("content-type", "application/json")
                    .body(Body::from(put_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // GET /api/cache/foo
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/cache/foo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp).await;
        assert_eq!(json["key"], "foo");
        assert_eq!(json["value"], "bar");
    }

    #[tokio::test]
    async fn http_delete_removes_key() {
        let cache = Arc::new(DdbCache::new());
        cache.set("drop", b"hi".to_vec(), None);
        let app = cache_http_router(cache.clone());

        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/cache/drop")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(cache.get("drop").is_none());
    }

    #[tokio::test]
    async fn http_hash_set_and_get_all() {
        let cache = Arc::new(DdbCache::new());
        let app = cache_http_router(cache.clone());

        let body = json!({ "field": "name", "value": "darshan" });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/cache/hash/user:1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/cache/hash/user:1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let json = body_json(resp).await;
        assert_eq!(json["fields"]["name"], "darshan");
    }

    #[tokio::test]
    async fn http_list_push_and_range() {
        let cache = Arc::new(DdbCache::new());
        let app = cache_http_router(cache.clone());

        let body = json!({ "side": "R", "values": ["a", "b", "c"] });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/cache/list/q/push")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/cache/list/q?start=0&stop=-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let json = body_json(resp).await;
        assert_eq!(json["values"], json!(["a", "b", "c"]));
    }

    #[tokio::test]
    async fn http_zset_add_and_range() {
        let cache = Arc::new(DdbCache::new());
        let app = cache_http_router(cache.clone());

        for (score, member) in [(1.0, "a"), (3.0, "c"), (2.0, "b")] {
            let body = json!({ "score": score, "member": member });
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/cache/zset/leader")
                        .header("content-type", "application/json")
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/cache/zset/leader")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let json = body_json(resp).await;
        let members = json["members"].as_array().unwrap();
        assert_eq!(members[0]["member"], "a");
        assert_eq!(members[2]["member"], "c");
    }

    #[tokio::test]
    async fn http_stats_returns_object() {
        let cache = Arc::new(DdbCache::new());
        cache.set("k", b"v".to_vec(), None);
        let app = cache_http_router(cache.clone());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/cache/stats")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let json = body_json(resp).await;
        assert_eq!(json["strings"], 1);
    }

    #[tokio::test]
    async fn http_keys_and_ttl_endpoints() {
        let cache = Arc::new(DdbCache::new());
        cache.set("user:1", b"\"a\"".to_vec(), None);
        cache.set("user:2", b"\"b\"".to_vec(), None);
        cache.set("other", b"\"c\"".to_vec(), None);
        let app = cache_http_router(cache.clone());

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/cache/keys?pattern=user:*")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let json = body_json(resp).await;
        let mut keys: Vec<String> = json["keys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        keys.sort();
        assert_eq!(keys, vec!["user:1".to_string(), "user:2".to_string()]);

        // EXPIRE + TTL round-trip.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/cache/user:1/expire")
                    .header("content-type", "application/json")
                    .body(Body::from(json!({ "ttl_seconds": 120 }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/cache/user:1/ttl")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let json = body_json(resp).await;
        let ttl = json["ttl_seconds"].as_i64().unwrap();
        assert!(ttl > 0 && ttl <= 120);
    }
}
