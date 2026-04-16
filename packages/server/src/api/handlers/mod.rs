//! Handler modules for the DarshJDB REST API.
//!
//! Each sub-module groups related HTTP handlers by domain concern.
//! The parent [`super::rest`] module composes these into a single router.

pub mod admin;
pub mod auth;
pub mod auth_oauth;
pub mod data;
pub mod data_mutation;
pub mod docs;
pub mod events;
pub mod functions;
pub mod graph;
pub mod helpers;
pub mod query;
pub mod schema;
pub mod search;
pub mod storage;
