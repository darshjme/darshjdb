//! Aggregation engine for DarshJDB.
//!
//! Provides GROUP BY, pivot, statistical aggregation, and time-series
//! bucketing over the EAV triple store. Powers view-level summaries,
//! rollup computations, and dashboard widgets.
//!
//! # Architecture
//!
//! The engine translates high-level aggregation queries into CTE-based
//! SQL that first materializes the EAV triples into a columnar form
//! (pivot), then applies PostgreSQL's native aggregate functions. This
//! pushes all heavy computation into the database, avoiding large
//! client-side data transfers.

pub mod chart;
pub mod engine;
pub mod handlers;
pub mod sql_builder;

pub use chart::{ChartBucket, ChartQuery, ChartResult, TimeBucket};
pub use engine::{
    AggFn, AggGroup, AggregateQuery, AggregateResult, Aggregation, AggregationEngine,
    HavingClause, HavingOp,
};
pub use handlers::aggregation_routes;
pub use sql_builder::build_aggregate_sql;
