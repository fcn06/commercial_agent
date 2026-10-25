//! Northbound Protocol Ingress Domain (A2A JSON-RPC 2.0 & OAuth2 Server)

pub mod oauth;
pub mod server;

#[allow(unused_imports)]
pub use oauth::{
    load_b2b_oauth_config, load_or_generate_keypair, start_oauth_server, B2bOAuthConfig,
    B2bOAuthState,
};
#[allow(unused_imports)]
pub use server::{AuthConfig, SecureAgentServer};
