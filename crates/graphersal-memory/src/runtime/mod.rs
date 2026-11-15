//! The processes and the store: the daemon (the single writer), the shim each harness starts,
//! their transport and protocol, the stores and their registry and maintenance, the MCP
//! profiles and the product catalog, the engine pin.

pub mod catalog;
pub mod client;
pub mod daemon;
pub mod engine;
pub mod ipc;
pub mod maintenance;
#[cfg(test)]
pub(crate) mod maintenance_tests;
pub mod mcp;
#[cfg(test)]
pub(crate) mod mcp_tests;
pub mod protocol;
pub mod registry;
pub mod shim;
pub mod store;
