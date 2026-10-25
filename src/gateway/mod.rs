//! Execution Egress Domain (Tier 2: Marketplace Trust Gateway)
//!
//! Manages tool discovery and sandboxed policy-governed execution against
//! the Marketplace Trust Gateway.

pub mod mcp_runtime;

pub use mcp_runtime::{McpAgent, SsiAuthenticationData};
