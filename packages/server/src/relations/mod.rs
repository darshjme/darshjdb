//! Relational field system for DarshJDB.
//!
//! Implements Link, Lookup, and Rollup fields on top of the EAV triple
//! store's `ValueType::Reference`. These are the relational primitives
//! that make DarshJDB a full BaaS — analogous to Teable's linked-record
//! system but built over triples rather than foreign-key columns.
//!
//! # Architecture
//!
//! - **Link** fields create bidirectional reference triples between entities.
//!   OneToOne and OneToMany links store references directly on the entity.
//!   ManyToMany links use junction entities (`link:{uuid}`) with `link/source`
//!   and `link/target` reference triples.
//!
//! - **Lookup** fields traverse a link and pull field values from the linked
//!   record(s), producing a single value or array depending on cardinality.
//!
//! - **Rollup** fields traverse a link, collect a target field's values,
//!   and apply an aggregation function (Count, Sum, Average, Min, Max, etc.).
//!
//! - **Cascade** handles cleanup when linked records are mutated or deleted,
//!   invalidating dependent lookups and rollups and emitting change events.

pub mod cascade;
pub mod handlers;
pub mod link;
pub mod lookup;
pub mod rollup;

pub use cascade::{cascade_delete, cascade_update, CascadeEvent};
pub use link::{LinkConfig, Relationship, add_link, create_link, get_linked, remove_link};
pub use lookup::{LookupConfig, LookupField, resolve_lookup};
pub use rollup::{RollupConfig, RollupFn, compute_rollup};
