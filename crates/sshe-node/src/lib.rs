//! Node lifecycle, peer RPC and scheduled probe history.
//!
//! Layers, lowest first: `transport` (Endpoint, RPC, timeouts) → `probe` →
//! `dispatch` (one request against a `Node`) → `server` / `client` → `daemon`.
mod client;
pub mod config;
mod daemon;
mod dispatch;
mod error;
mod history;
mod identity;
mod ipc;
mod layout;
mod probe;
mod scheduler;
mod server;
mod transport;

pub use client::invoke;
pub use daemon::daemon;
pub use error::{Error, Result};

/// This node's public identity, derived from the configured key.
pub fn endpoint_id(config: &config::Config) -> Result<iroh::EndpointId> {
    Ok(identity::load_key(&config.identity)?.public())
}

#[cfg(test)]
mod tests;
