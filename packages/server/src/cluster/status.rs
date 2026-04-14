// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
//
// `/cluster/status` endpoint.
//
// Exposes per-replica node identity, uptime, and the set of singleton
// background tasks this node currently leads. Mounted at the top level
// (next to `/health`) rather than under `/api` so load balancers and
// operator scripts can probe it without authenticating.
//
// The response is stable JSON suitable for both human consumption
// (`curl | jq`) and machine polling (Prometheus textfile exporter, etc.).

use std::sync::Arc;

use axum::{Json, Router, extract::State, routing::get};
use serde::{Deserialize, Serialize};

use super::{ClusterState, NodeId};

/// JSON body returned by `GET /cluster/status`.
#[derive(Debug, Serialize, Deserialize)]
pub struct ClusterStatusResponse {
    /// Stable node UUID (random at process startup).
    pub node_id: String,
    /// Seconds since the process started.
    pub uptime_secs: u64,
    /// Names of singleton background tasks for which THIS replica
    /// currently holds the advisory lock. Example: `["anchor_writer",
    /// "expiry_sweeper"]`.
    pub leader_for: Vec<String>,
    /// Process version (`CARGO_PKG_VERSION`).
    pub version: String,
}

#[derive(Clone)]
pub struct ClusterStatusState {
    pub node_id: Arc<NodeId>,
    pub cluster_state: ClusterState,
}

/// Build the `/cluster/status` router.
pub fn router(state: ClusterStatusState) -> Router {
    Router::new()
        .route("/cluster/status", get(handler))
        .with_state(state)
}

async fn handler(
    State(state): State<ClusterStatusState>,
) -> Json<ClusterStatusResponse> {
    let leader_for = state
        .cluster_state
        .leader_for()
        .await
        .into_iter()
        .map(|s| s.to_string())
        .collect();
    Json(ClusterStatusResponse {
        node_id: state.node_id.uuid().to_string(),
        uptime_secs: state.node_id.uptime_secs(),
        leader_for,
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn status_endpoint_returns_node_identity() {
        let node = Arc::new(NodeId::new());
        let state = ClusterState::new();
        state.mark_leader("anchor_writer").await;
        state.mark_leader("expiry_sweeper").await;

        let app = router(ClusterStatusState {
            node_id: node.clone(),
            cluster_state: state,
        });

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/cluster/status")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let parsed: ClusterStatusResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.node_id, node.uuid().to_string());
        assert_eq!(
            parsed.leader_for,
            vec!["anchor_writer".to_string(), "expiry_sweeper".to_string()]
        );
        let _ = parsed.uptime_secs;
    }
}
