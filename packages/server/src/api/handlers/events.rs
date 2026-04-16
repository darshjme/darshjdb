//! SSE subscription and pub/sub event handlers.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::Response;
use serde::Deserialize;
use serde_json::Value;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

use crate::api::error::ApiError;
use crate::api::rest::AppState;
use crate::sync::pubsub::PubSubEvent;

use super::helpers::{extract_bearer_token, negotiate_response};

// ---------------------------------------------------------------------------
// SSE subscribe (live queries)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct SubscribeParams {
    q: String,
}

/// `GET /api/subscribe?q=...` -- Server-Sent Events for live query updates.
pub async fn subscribe(
    State(state): State<AppState>,
    Query(params): Query<SubscribeParams>,
    headers: HeaderMap,
) -> Result<Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let _token = extract_bearer_token(&headers)?;

    if params.q.is_empty() {
        return Err(ApiError::bad_request("Query parameter 'q' is required"));
    }

    let rx = state.sse_tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|msg| match msg {
        Ok(payload) => {
            let data = serde_json::to_string(&payload.data).unwrap_or_default();
            Some(Ok(Event::default()
                .event("update")
                .data(data)
                .id(payload.tx_id.to_string())))
        }
        Err(_) => None,
    });

    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

// ---------------------------------------------------------------------------
// Pub/Sub SSE
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct EventsParams {
    channel: String,
}

/// `GET /api/events?channel=entity:users:*` -- Server-Sent Events for pub/sub.
pub async fn events_sse(
    State(state): State<AppState>,
    Query(params): Query<EventsParams>,
    headers: HeaderMap,
) -> Result<Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let _token = extract_bearer_token(&headers)?;

    if params.channel.is_empty() {
        return Err(ApiError::bad_request(
            "Query parameter 'channel' is required",
        ));
    }

    let pattern = crate::sync::pubsub::ChannelPattern::parse(&params.channel);
    let rx = state.pubsub.subscribe_events();

    let stream = BroadcastStream::new(rx).filter_map(move |msg| match msg {
        Ok(event) => {
            if pattern.matches(&event.channel) {
                let data = serde_json::to_string(&event).unwrap_or_default();
                Some(Ok(Event::default()
                    .event("pub-event")
                    .data(data)
                    .id(event.tx_id.to_string())))
            } else {
                None
            }
        }
        Err(_) => None,
    });

    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

// ---------------------------------------------------------------------------
// Publish
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct PublishRequest {
    channel: String,
    event: String,
    #[serde(default)]
    payload: Option<Value>,
}

/// `POST /api/events/publish` -- Publish a custom event to a channel.
pub async fn events_publish(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> Result<Response, ApiError> {
    let _token = extract_bearer_token(&headers)?;

    let req: PublishRequest = serde_json::from_str(&body)
        .map_err(|e| ApiError::bad_request(format!("invalid request body: {e}")))?;

    if req.channel.is_empty() {
        return Err(ApiError::bad_request("'channel' is required"));
    }
    if req.event.is_empty() {
        return Err(ApiError::bad_request("'event' is required"));
    }

    let pub_event = PubSubEvent {
        channel: req.channel.clone(),
        event: req.event.clone(),
        entity_type: None,
        entity_id: None,
        changed: vec![],
        tx_id: 0,
        payload: req.payload,
    };

    let receivers = state.pubsub.publish(pub_event);

    let response = serde_json::json!({
        "ok": true,
        "channel": req.channel,
        "event": req.event,
        "receivers": receivers,
    });

    Ok(negotiate_response(&headers, &response))
}
