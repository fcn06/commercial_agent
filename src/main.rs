#![deny(clippy::disallowed_methods)]

use clap::Parser;
use configuration::{setup_logging, AgentConfig};
use std::env;

mod agent;
mod cognitive;
mod gateway;
mod protocol;
mod security;

use agent::skill::Skill;
use agent::B2bIdentityAgent;
use cognitive::SwarmClient;
use protocol::oauth;
use protocol::server::{AuthConfig, SecureAgentServer};
use security::context;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(long, default_value = "configuration/agent_identity_config.toml")]
    config_file: String,
    #[clap(long, default_value = "info")]
    log_level: String,
    #[clap(long)]
    skill_file: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();
    let args = Args::parse();

    setup_logging(&args.log_level);
    tracing::info!("🚀 Starting B2B Semantic Filter Agent (Professional Edition)");

    let mut identity_agent_config =
        AgentConfig::load_agent_config(&args.config_file).expect("Incorrect Agent config file");

    if let Ok(port) = env::var("B2B_AGENT_PORT") {
        identity_agent_config.agent_http_endpoint = format!("http://127.0.0.1:{}", port);
        tracing::info!(
            "Overriding agent HTTP endpoint to {}",
            identity_agent_config.agent_http_endpoint
        );
    }

    let agent_api_key = context::load_secret("SWARM_GATEWAY_TOKEN")
        .or_else(|| context::load_secret("SWARM_API_KEY"))
        .or_else(|| context::load_secret("LLM_A2A_API_KEY_B2B"))
        .or_else(|| context::load_secret("LLM_A2A_API_KEY"))
        .map(|s| s.expose_secret().to_string())
        .or_else(|| env::var("SWARM_GATEWAY_TOKEN").ok())
        .or_else(|| env::var("SWARM_API_KEY").ok())
        .unwrap_or_default();

    // Check connectivity to Swarm Gateway (Tier 1 Cognitive Ingress)
    let swarm_probe = SwarmClient::from_env_or_config(
        Some(&identity_agent_config.agent_llm_url()),
        Some(&identity_agent_config.agent_model_id()),
        None,
    );
    match swarm_probe.check_health().await {
        Ok(true) => tracing::info!(
            "✅ Swarm Gateway connected: {} (Cognitive format: {})",
            swarm_probe.gateway_url,
            swarm_probe.format
        ),
        Ok(false) => tracing::warn!(
            "⚠️ Swarm Gateway health probe returned non-200 from {}",
            swarm_probe.gateway_url
        ),
        Err(e) => tracing::warn!(
            "⚠️ Swarm Gateway unreachable at {}: {}. Will retry during requests.",
            swarm_probe.gateway_url,
            e
        ),
    }

    // Resolve Markdown skill file if provided or found
    let skill_file_path = args
        .skill_file
        .or_else(|| env::var("B2B_SKILL_PATH").ok())
        .or_else(|| {
            let default_path = "configuration/skills/default_skill.md";
            if std::path::Path::new(default_path).exists() {
                Some(default_path.to_string())
            } else {
                None
            }
        });

    let loaded_skill = if let Some(ref path) = skill_file_path {
        match Skill::from_file(path) {
            Ok(skill) => {
                tracing::info!(
                    "📖 Loaded Markdown skill '{}' ({}) from {}",
                    skill.name(),
                    skill.id(),
                    path
                );
                Some(skill)
            }
            Err(e) => {
                tracing::warn!(
                    "⚠️ Failed to load skill from {}: {}. Proceeding with default config.",
                    path,
                    e
                );
                None
            }
        }
    } else {
        None
    };

    use agent_core::business_logic::agent::Agent;
    let mut b2b_agent = B2bIdentityAgent::new(
        identity_agent_config.clone(),
        agent_api_key,
        None,
        None,
        None,
        None,
        None,
    )
    .await?;

    if let Some(ref skill) = loaded_skill {
        b2b_agent = b2b_agent.with_skill(skill.clone());
    }

    let enable_embedded_oauth: bool = env::var("B2B_ENABLE_EMBEDDED_OAUTH")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    if enable_embedded_oauth {
        let b2b_oauth_port: u16 = env::var("B2B_OAUTH_PORT")
            .unwrap_or_else(|_| "4012".to_string())
            .parse()
            .unwrap_or(4012);

        let b2b_oauth_config_path = env::var("B2B_OAUTH_CONFIG_PATH")
            .unwrap_or_else(|_| "configuration/b2b_oauth_clients.toml".to_string());
        let b2b_oauth_config = oauth::load_b2b_oauth_config(&b2b_oauth_config_path);

        let (signing_key, public_key_b64) =
            oauth::load_or_generate_keypair().expect("Failed to load or generate B2B signing keypair");

        let oauth_state = oauth::B2bOAuthState {
            config: std::sync::Arc::new(b2b_oauth_config),
            signing_key: std::sync::Arc::new(signing_key),
            public_key_b64,
        };

        oauth::start_oauth_server(b2b_oauth_port, oauth_state).await?;
        tracing::info!("🔑 Embedded fallback OAuth2 server listening on port {}", b2b_oauth_port);
    } else {
        let identity_url = env::var("B2B_IDENTITY_SERVER_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:3075".to_string());
        tracing::info!(
            "🔑 Embedded OAuth2 server (:4012) disabled. Using sovereign Identity Server at {}",
            identity_url
        );
    }

    let auth_config = AuthConfig::None;

    let mut authenticated_agent_server = SecureAgentServer::<B2bIdentityAgent>::new(
        identity_agent_config.clone(),
        b2b_agent,
        auth_config,
        None,
        Some("a2a.v1.*.b2b.send".to_string()),
    )
    .await?;

    if let Some(skill) = loaded_skill {
        authenticated_agent_server = authenticated_agent_server.with_skill(skill);
    }

    tracing::info!("🌐 Starting HTTP server for B2B Identity Agent...");
    authenticated_agent_server.start_http().await?;

    Ok(())
}

pub fn load_b2b_secret() -> String {
    if let Some(sec) = context::load_secret("B2B_JWT_SECRET") {
        let val = sec.expose_secret().to_string();
        if !val.trim().is_empty() {
            return val;
        }
    }

    let _ = dotenvy::dotenv();
    if let Ok(sec) = std::env::var("B2B_JWT_SECRET") {
        let trimmed = sec.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    panic!("🚨 B2B JWT secret is not configured!");
}
