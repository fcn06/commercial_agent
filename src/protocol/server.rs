//! Standalone A2A Agent Server for Commercial B2B Agent

use a2a_rs::adapter::{
    BearerTokenAuthenticator, DefaultRequestProcessor, HttpServer, InMemoryTaskStorage,
    NoopPushNotificationSender, SimpleAgentInfo,
};
use agent_core::business_logic::agent::Agent;
use agent_core::business_logic::services::DiscoveryService;
use agent_core::server::agent_handler::AgentHandler;
use anyhow::Result;
use configuration::AgentConfig;
use std::sync::Arc;

use crate::agent::skill::Skill;

#[derive(Debug, Clone)]
pub enum AuthConfig {
    None,
    BearerToken { tokens: Vec<String>, format: String },
}

pub struct SecureAgentServer<T: Agent> {
    config: AgentConfig,
    agent: T,
    auth: AuthConfig,
    discovery_service: Option<Arc<dyn DiscoveryService>>,
    nats_subject: Option<String>,
    skills: Vec<Skill>,
}

impl<T: Agent> SecureAgentServer<T> {
    pub async fn new(
        agent_config: AgentConfig,
        agent: T,
        auth: AuthConfig,
        discovery_service: Option<Arc<dyn DiscoveryService>>,
        nats_subject: Option<String>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            config: agent_config,
            agent,
            auth,
            discovery_service,
            nats_subject,
            skills: Vec::new(),
        })
    }

    pub fn with_skill(mut self, skill: Skill) -> Self {
        self.skills.push(skill);
        self
    }

    pub fn with_skills(mut self, skills: Vec<Skill>) -> Self {
        self.skills = skills;
        self
    }

    fn create_in_memory_storage(&self) -> InMemoryTaskStorage {
        tracing::info!("Using in-memory storage");
        let push_sender = NoopPushNotificationSender;
        InMemoryTaskStorage::with_push_sender(push_sender)
    }

    pub async fn start_http(&self) -> Result<(), Box<dyn std::error::Error>> {
        let storage = self.create_in_memory_storage();
        let message_handler = AgentHandler::<T>::with_storage(self.agent.clone(), storage.clone());

        let agent_http_endpoint = format!("{}", self.config.agent_http_endpoint());
        let simple_agent_info =
            SimpleAgentInfo::new(self.config.agent_name(), agent_http_endpoint.clone());

        let processor = DefaultRequestProcessor::with_handler(message_handler, simple_agent_info);

        let doc_url = self
            .config
            .agent_doc_url()
            .unwrap_or_else(|| "/docs".to_string());

        let mut agent_info =
            SimpleAgentInfo::new(self.config.agent_name(), agent_http_endpoint.clone())
                .with_description(self.config.agent_description())
                .with_documentation_url(doc_url)
                .with_streaming();

        if !self.skills.is_empty() {
            for skill in &self.skills {
                agent_info = agent_info.add_comprehensive_skill(
                    skill.id().to_string(),
                    skill.name().to_string(),
                    Some(skill.description().to_string()),
                    Some(skill.tags()),
                    Some(skill.examples()),
                    Some(vec!["text".to_string(), "data".to_string()]),
                    Some(vec!["text".to_string(), "data".to_string()]),
                );
            }
        } else {
            agent_info = agent_info.add_comprehensive_skill(
                self.config.agent_skill_id(),
                self.config.agent_skill_name(),
                Some(self.config.agent_skill_description()),
                Some(self.config.agent_tags()),
                Some(self.config.agent_examples()),
                Some(vec!["text".to_string(), "data".to_string()]),
                Some(vec!["text".to_string(), "data".to_string()]),
            );
        }

        let bind_address = agent_http_endpoint.clone().replace("http://", "");

        println!(
            "🌐 Starting HTTP a2a agent server {} on {}",
            self.config.agent_name(),
            self.config.agent_http_endpoint()
        );
        println!(
            "📋 Agent card: {}/agent-card",
            self.config.agent_http_endpoint(),
        );
        println!("🛠️  Skills: {}/skills", self.config.agent_http_endpoint());
        println!("💾 Storage: In-memory (non-persistent)");

        // Spawn NATS listener if subject is configured
        if let Some(ref subject) = self.nats_subject {
            let nats_url =
                std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".to_string());
            let local_http_endpoint = self.config.agent_http_endpoint().to_string();
            let subject_clone = subject.clone();
            let agent_name = self.config.agent_name().to_lowercase();

            tokio::spawn(async move {
                let mut backoff = std::time::Duration::from_secs(1);
                loop {
                    tracing::info!(
                        "🔄 {} attempting NATS connection to {}",
                        agent_name,
                        nats_url
                    );
                    match async_nats::connect(&nats_url).await {
                        Ok(nats_client) => {
                            tracing::info!("✅ {} connected to NATS at {}", agent_name, nats_url);
                            match nats_client.subscribe(subject_clone.clone()).await {
                                Ok(mut subscriber) => {
                                    use futures::StreamExt;
                                    tracing::info!(
                                        "📬 {} NATS listener subscribed to {}",
                                        agent_name,
                                        subject_clone
                                    );
                                    let client = reqwest::Client::new();
                                    while let Some(msg) = subscriber.next().await {
                                        let client_inner = client.clone();
                                        let endpoint = local_http_endpoint.clone();
                                        let payload = msg.payload.to_vec();
                                        tokio::spawn(async move {
                                            let _ = client_inner
                                                .post(&endpoint)
                                                .header("Content-Type", "application/json")
                                                .body(payload)
                                                .send()
                                                .await;
                                        });
                                    }
                                }
                                Err(e) => {
                                    tracing::error!("❌ Failed to subscribe to NATS: {}", e);
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!("❌ Failed to connect to NATS: {}", e);
                        }
                    }
                    tokio::time::sleep(backoff).await;
                    backoff = std::cmp::min(backoff * 2, std::time::Duration::from_secs(30));
                }
            });
        }

        match &self.auth {
            AuthConfig::None => {
                println!("🔓 Authentication: None (public access)");
                let server = HttpServer::new(processor, agent_info, bind_address);
                server
                    .start()
                    .await
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
            }
            AuthConfig::BearerToken { tokens, .. } => {
                println!(
                    "🔐 Authentication: Bearer token ({} token(s))",
                    tokens.len()
                );
                let authenticator = BearerTokenAuthenticator::new(tokens.clone());
                let server =
                    HttpServer::with_auth(processor, agent_info, bind_address, authenticator);
                server
                    .start()
                    .await
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
            }
        }
    }
}
