// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
// ddb-cache: crate root.
//
// Slice 10 (Phase 1.3) — Unified DdbCache Layer
// --------------------------------------------
// This crate provides a unified two-tier cache for the DarshJDB engine:
//
//   * `L1Cache` — fast in-memory DashMap tier (from Slice 8)
//   * `L2Cache` — durable Postgres-backed tier  (from Slice 9)
//   * `PubSubEngine` — tokio::sync::broadcast fan-out for keyspace
//     notifications and cross-node cache coherence signals
//   * `DdbCache` — composition of L1 + L2 + pub/sub that implements
//     read-through and write-through semantics and records Prometheus
//     metrics via the `metrics` crate.
//
// Slices 8 and 9 land in parallel; while they are in-flight the local
// copies of `L1Cache` and `L2Cache` below are minimal stubs sufficient
// to (a) build the workspace, (b) validate unified semantics end-to-end,
// and (c) be trivially reconciled against the real implementations when
// all three slices merge.
//
// Author: Darshankumar Joshi

pub mod l1;
pub mod l2;
pub mod pubsub;
pub mod unified;

pub use l1::L1Cache;
pub use l2::L2Cache;
pub use pubsub::{PubSubEngine, PubSubMessage};
pub use unified::DdbCache;
