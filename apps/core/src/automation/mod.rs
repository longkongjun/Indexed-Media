//! Durable automation source configuration and execution boundaries.

/// Download completion signal dispatch into fixed reconcile events.
pub mod completion;
/// Authenticated automation event query and command routes.
pub mod event_routes;
/// Durable automation event state, payload, lease, and command persistence.
pub mod event_store;
/// Strict source configuration inputs and redacted projections.
pub mod model;
/// Authenticated automation source management routes.
pub mod routes;
/// Bounded RSS/Atom protocol adapter and polling orchestration.
pub mod rss;
/// Startup recovery and bounded automation worker loop.
pub mod runtime;
/// Source configuration orchestration.
pub mod service;
/// Encrypted, versioned source persistence.
pub mod source_store;
/// Signed inbound webhook verification and durable acceptance.
pub mod webhook;
/// Fixed-action automation event executor.
pub mod worker;
