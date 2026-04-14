// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
//
// Postgres LISTEN/NOTIFY fanout for multi-replica WebSocket subscriptions.
//
// Problem
// -------
// When DarshJDB runs as multiple `ddb-server` replicas behind a load
// balancer, a WebSocket subscription opened against replica A will only
// see events from mutations that happen on replica A — the in-process
// broadcast channel (`tokio::sync::broadcast`) doesn't know about B.
// Without fanout, a client connected to A never learns about triples
// written via B's REST handler.
//
// Solution
// --------
// The triple store's write path already emits
// `pg_notify('ddb_changes', '{tx_id}:{entity_type}')` at commit time.
// This module spawns a dedicated task that:
//
//   1. Opens a `PgListener` (separate from the pool — `LISTEN`
//      connections can't return to the pool while listening).
//   2. LISTENs on the `ddb_changes` channel forever.
//   3. Each notification is parsed and re-published into the local
//      `change_tx` broadcast channel so in-process subscribers
//      (WebSocket sessions, live queries, connectors) get the event as
//      though it had happened locally.
//
// On connection error the task sleeps 1 s and reconnects — the outer
// loop never exits unless the process does.
//
// Prior history: this logic used to live inline in `main.rs`. Extracting
// it to the `cluster` module clarifies its role as part of the
// horizontal-scaling story.

use std::time::Duration;

use sqlx::postgres::PgListener;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use crate::sync::ChangeEvent;

/// Name of the Postgres NOTIFY channel used by the triple store write path.
pub const CHANGE_CHANNEL: &str = "ddb_changes";

/// Spawn the LISTEN loop.
///
/// * `database_url` — full Postgres connection string. A new session is
///   opened for the listener (separate from the main pool).
/// * `change_tx` — the in-process broadcast channel that carries
///   [`ChangeEvent`] to every WebSocket / live-query subscriber on this
///   replica.
///
/// Returns the task's `JoinHandle`. Dropping it aborts the listener.
pub fn spawn(
    database_url: String,
    change_tx: tokio::sync::broadcast::Sender<ChangeEvent>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let mut listener = match PgListener::connect(&database_url).await {
                Ok(l) => l,
                Err(e) => {
                    error!(error = %e, "failed to create PgListener for ddb_changes; retrying in 5s");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
            };
            if let Err(e) = listener.listen(CHANGE_CHANNEL).await {
                error!(error = %e, "failed to LISTEN on ddb_changes channel; retrying in 5s");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
            info!("LISTEN/NOTIFY: subscribed to {CHANGE_CHANNEL} channel");

            loop {
                match listener.recv().await {
                    Ok(notification) => {
                        let payload = notification.payload();
                        let (tx_id, entity_type) = parse_payload(payload);
                        debug!(
                            tx_id,
                            entity_type = ?entity_type,
                            "received {CHANGE_CHANNEL} notification"
                        );
                        let _ = change_tx.send(ChangeEvent {
                            tx_id,
                            entity_ids: vec![],
                            attributes: vec![],
                            entity_type,
                            actor_id: None,
                        });
                    }
                    Err(e) => {
                        warn!(error = %e, "PgListener recv error, reconnecting in 1s");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        break; // re-enter outer loop to rebuild the listener
                    }
                }
            }
        }
    })
}

/// Parse a `ddb_changes` payload of the form `{tx_id}` or `{tx_id}:{entity_type}`.
///
/// Unknown payloads degrade to `tx_id = 0`, `entity_type = None`.
pub fn parse_payload(payload: &str) -> (i64, Option<String>) {
    match payload.split_once(':') {
        Some((tid, etype)) => {
            let tid: i64 = tid.parse().unwrap_or(0);
            (tid, Some(etype.to_string()))
        }
        None => {
            let tid: i64 = payload.parse().unwrap_or(0);
            (tid, None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_payload_with_entity_type() {
        let (tx_id, etype) = parse_payload("42:user");
        assert_eq!(tx_id, 42);
        assert_eq!(etype.as_deref(), Some("user"));
    }

    #[test]
    fn parse_payload_without_entity_type() {
        let (tx_id, etype) = parse_payload("99");
        assert_eq!(tx_id, 99);
        assert_eq!(etype, None);
    }

    #[test]
    fn parse_payload_garbage_is_zero() {
        let (tx_id, etype) = parse_payload("not-a-number");
        assert_eq!(tx_id, 0);
        assert_eq!(etype, None);
    }

    #[test]
    fn parse_payload_garbage_with_colon() {
        let (tx_id, etype) = parse_payload("nope:something");
        assert_eq!(tx_id, 0);
        assert_eq!(etype.as_deref(), Some("something"));
    }
}
