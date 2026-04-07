//! REST API layer for DarshJDB.
//!
//! Exposes the full DarshJDB feature set over HTTP using Axum:
//! authentication, data queries and mutations, server-side functions,
//! file storage, real-time subscriptions (SSE), admin introspection,
//! and auto-generated OpenAPI documentation.
//!
//! # Route Overview
//!
//! ```text
//! /api/auth/*          Authentication (signup, signin, OAuth, etc.)
//! /api/query           DarshJQL over HTTP
//! /api/mutate          Transaction submission
//! /api/data/:entity    REST-style CRUD
//! /api/fn/:name        Server-side function invocation
//! /api/storage/*       File upload, download, deletion
//! /api/subscribe       Server-Sent Events for live queries
//! /api/admin/*         Schema, functions, sessions introspection
//! /api/openapi.json    OpenAPI 3.1 specification
//! /api/docs            Scalar API documentation viewer
//! ```
//!
//! # Content Negotiation
//!
//! Clients may request MessagePack (`Accept: application/msgpack`) or
//! JSON (`Accept: application/json`, the default). All request bodies
//! are decoded the same way via the `Content-Type` header.
//!
//! # Rate Limiting
//!
//! Every response includes `X-RateLimit-Limit`, `X-RateLimit-Remaining`,
//! and `X-RateLimit-Reset` headers.

pub mod batch;
pub mod docs;
pub mod error;
pub mod openapi;
pub mod pool_stats;
pub mod rest;
pub mod sdk_types;
pub mod ws;

pub use error::ApiError;
pub use pool_stats::PoolStats;
pub use rest::build_router;
pub use ws::{WsState, ws_routes};
