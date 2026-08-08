//! Bounded RSS 2.0 and Atom polling support.

/// Conditional, redirect-free HTTP feed client.
pub mod client;
/// Streaming bounded RSS 2.0 and Atom parser.
pub mod parser;
/// Poll-to-atomic-commit orchestration.
pub mod poller;
