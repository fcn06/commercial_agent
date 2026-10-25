use async_trait::async_trait;
use base64::Engine;
use ed25519_dalek::Verifier;
use serde_json::{Map, Value};
use sha2::Digest;
use std::sync::Arc;
use uuid::Uuid;

pub mod mapping;
pub mod policy;
pub mod skill;

#[allow(unused_imports)]
pub use mapping::IoMapper;
#[allow(unused_imports)]
pub use policy::fetch_b2b_prompt;
pub use skill::Skill;

use crate::cognitive::SwarmClient;
use crate::gateway::{McpAgent, SsiAuthenticationData};
use agent_core::business_logic::agent::Agent;
use agent_core::business_logic::mcp_runtime::McpRuntimeDetails;
use agent_core::business_logic::services::{
    DiscoveryService, EvaluationService, MemoryService, WorkflowServiceApi,
};
use agent_models::execution::execution_result::ExecutionResult;
use configuration::AgentConfig;
use llm_api::chat::ChatCompletionRequest;
use llm_api::chat::Message as LlmMessage;

#[derive(Clone)]
pub struct B2bIdentityAgent {
    swarm_client: Arc<SwarmClient>,
    mcp_agent: Option<Arc<McpAgent>>,
    system_message: String,
    nats_client: Option<async_nats::Client>,
    http_client: reqwest::Client,
    did_web_cache: Option<async_nats::jetstream::kv::Store>,
    io_mapper: Arc<mapping::IoMapper>,
}

#[async_trait]
impl Agent for B2bIdentityAgent {
    async fn new(
        agent_config: AgentConfig,
        _agent_api_key: String,
        mcp_runtime_details: Option<McpRuntimeDetails>,
        _evaluation_service: Option<Arc<dyn EvaluationService>>,
        _memory_service: Option<Arc<dyn MemoryService>>,
        _discovery_service: Option<Arc<dyn DiscoveryService>>,
        _workflow_service: Option<Arc<dyn WorkflowServiceApi>>,
    ) -> anyhow::Result<Self> {
        let model_id = agent_config.agent_model_id();
        let system_message = agent_config
            .agent_system_prompt()
            .expect("agent_system_prompt not found in config");

        let swarm_client = Arc::new(SwarmClient::from_env_or_config(
            Some(&agent_config.agent_llm_url()),
            Some(&model_id),
            None,
        ));

        let nats_url =
            std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".to_string());
        let mut nats_options =
            if let Some(seed) = crate::security::load_secret("NATS_NKEY_SEED") {
                async_nats::ConnectOptions::with_nkey(seed.expose_secret().to_string())
            } else {
                async_nats::ConnectOptions::new()
            };
        let timeout_secs = std::env::var("NATS_REQUEST_TIMEOUT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(25);
        nats_options = nats_options
            .request_timeout(Some(std::time::Duration::from_secs(timeout_secs)))
            .retry_on_initial_connect()
            .max_reconnects(60) // Try for up to ~30 minutes with backoff
            .reconnect_delay_callback(|attempts| {
                // Exponential backoff capped at 30 seconds
                let delay = std::cmp::min(2u64.pow(attempts as u32), 30);
                std::time::Duration::from_secs(delay)
            })
            .event_callback(|event| async move {
                match event {
                    async_nats::Event::Disconnected => {
                        tracing::error!("🚨 NATS client disconnected!");
                    }
                    async_nats::Event::Connected => {
                        tracing::info!("✅ NATS client connected");
                    }
                    async_nats::Event::SlowConsumer(sc) => {
                        tracing::warn!("⚠️ NATS client slow consumer detected on subject: {}", sc);
                    }
                    _ => {}
                }
            });
        let nats_client = async_nats::connect_with_options(&nats_url, nats_options)
            .await
            .ok();

        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_default();

        let mut did_web_cache = None;
        if let Some(ref nc) = nats_client {
            let jetstream = async_nats::jetstream::new(nc.clone());
            did_web_cache = match jetstream.get_key_value("did_web_cache").await {
                Ok(kv) => Some(kv),
                Err(_) => {
                    jetstream
                        .create_key_value(async_nats::jetstream::kv::Config {
                            bucket: "did_web_cache".to_string(),
                            history: 1,
                            max_age: std::time::Duration::from_secs(3600), // 1 hour TTL
                            ..Default::default()
                        })
                        .await
                        .ok()
                }
            };
        }

        let mcp_agent = if let Some(details) = mcp_runtime_details {
            Some(Arc::new(
                McpAgent::new(
                    details.config,
                    Some(details.api_key),
                    nats_client.clone().map(Arc::new),
                )
                .await?
                .with_swarm_client(swarm_client.clone()),
            ))
        } else if let Some(path) = agent_config.agent_mcp_config_path() {
            let agent_mcp_config =
                configuration::McpRuntimeConfig::load_agent_config(path.as_str())
                    .map_err(|e| anyhow::anyhow!("Error loading MCP config: {}", e))?;
            Some(Arc::new(
                McpAgent::new(agent_mcp_config, None, nats_client.clone().map(Arc::new))
                    .await?
                    .with_swarm_client(swarm_client.clone()),
            ))
        } else {
            None
        };

        let io_mappings_path = std::env::var("B2B_SCHEMA_MAPPINGS_PATH")
            .unwrap_or_else(|_| "configuration/b2b_schema_mappings.toml".to_string());
        let io_mapper = mapping::IoMapper::load_from_file(&io_mappings_path)
            .unwrap_or_else(|_| {
                tracing::info!("Using default empty IO mapper (pass-through)");
                mapping::IoMapper::default()
            });
        let io_mapper = Arc::new(io_mapper);

        Ok(Self {
            swarm_client,
            mcp_agent,
            system_message,
            nats_client,
            http_client,
            did_web_cache,
            io_mapper,
        })
    }

    async fn handle_request(
        &self,
        request: LlmMessage,
        metadata: Option<Map<String, Value>>,
    ) -> anyhow::Result<ExecutionResult> {
        let request_id = Uuid::new_v4().to_string();
        let thread_id = metadata
            .as_ref()
            .and_then(|m| m.get("thread_id").or_else(|| m.get("conversation_id")))
            .and_then(|v| v.as_str())
            .unwrap_or(request_id.as_str())
            .to_string();

        // Check if external caller provided explicit external tool invocation in metadata
        if let Some((tool_name, args)) = metadata.as_ref().and_then(|meta| {
            let t = meta.get("external_tool_name").and_then(|v| v.as_str())?;
            let a = meta.get("tool_arguments")?;
            Some((t, a))
        }) && let Ok((canonical_tool, mapped_args)) = self.io_mapper.map_inbound(tool_name, args)
        {
            tracing::info!(
                "🔄 IO Mapper: Mapped external tool '{tool_name}' -> '{canonical_tool}' with translated args: {:?}",
                mapped_args
            );
        }

        let ssi_auth = match self.extract_authentication_metadata(&metadata).await {
            Ok(auth) => auth,
            Err(e) => {
                tracing::error!("Authentication failed: {}", e);
                return Err(anyhow::anyhow!("Authentication failed: {}", e));
            }
        };

        let response = if let Some(ref agent) = self.mcp_agent {
            let mut b2b_prompt_override = if !self.system_message.is_empty() {
                Some(self.system_message.clone())
            } else {
                None
            };

            // B2B Dynamic Prompt Injection
            let b2b_sender_info = metadata.as_ref().and_then(|meta| {
                let tenant = meta.get("tenant_id").and_then(|v| v.as_str())?;
                let sender = meta.get("sender_did").and_then(|v| v.as_str())?;
                let nc = self.nats_client.as_ref()?;
                if tenant != sender {
                    Some((tenant, sender, nc))
                } else {
                    None
                }
            });

            if let Some((tenant, sender, nc)) = b2b_sender_info {
                tracing::info!(
                    "🌐 Detected external B2B caller ({}). Fetching DLP policy...",
                    sender
                );
                let mut combined_prompt = if let Some(base) = b2b_prompt_override {
                    format!("{}\n\n", base)
                } else {
                    String::new()
                };
                combined_prompt.push_str(&format!(
                    "### Negotiated Interaction Contract Protocol (NICP) Rules:\n\
                    1. You are the host/service provider agent. The external caller is \"{sender}\". In all contract and reputation operations, \"{sender}\" IS your counterparty.\n\
                    2. When the caller asks about reputation or eligibility, inspect their reputation by calling `reputation_inspect_counterparty(counterparty_did: \"{sender}\")`.\n\
                    3. If the caller presents a third-party peer ExecutionReceipt, call `receipt_present_and_store(action: \"verify\", receipt: ...)` to verify and vault it.\n\
                    4. When drafting or amending contract terms, use `contract_propose_or_amend(contract_id, counterparty_did: \"{sender}\", capabilities, max_amount_minor, currency, settlement_terms, cancellation_notice_hours)`. In your response, ALWAYS explicitly output the drafted `contract_id` and `canonical_hash`.\n\
                    5. When the counterparty provides their attestation signature for the canonical hash, immediately call `contract_verify_and_activate(contract_id, counterparty_attestation)` to execute the 9-step activation ceremony.\n\
                    6. You MUST NOT claim that a contract is authorized or active until `contract_verify_and_activate` succeeds.\n\
                    7. Never represent a proposed or accepted contract as executable until activation succeeds.\n\
                    8. Once active, execute requested tasks within contract limits and retrieve the signed `ExecutionReceipt` using `receipt_present_and_store`.\n\n"
                ));
                if let Some(b2b_prompt) = policy::fetch_b2b_prompt(nc, tenant, sender).await {
                    tracing::info!("🔒 Injecting strict B2B Semantic Filter prompt.");
                    combined_prompt.push_str(&b2b_prompt);
                }
                b2b_prompt_override = Some(combined_prompt);
            }

            agent
                .run_agent_internal(request.clone(), Some(ssi_auth), b2b_prompt_override)
                .await
        } else {
            let messages = vec![
                LlmMessage {
                    role: "system".to_string(),
                    content: Some(self.system_message.clone()),
                    tool_call_id: None,
                    tool_calls: None,
                },
                request.clone(),
            ];
            let req_payload = ChatCompletionRequest {
                model: self.swarm_client.model_id.clone(),
                messages,
                temperature: Some(0.0),
                max_tokens: Some(4096),
                top_p: Some(1.0),
                stop: None,
                stream: Some(false),
                tools: None,
                tool_choice: None,
            };
            let resp = self.swarm_client.call_chat_completions(&req_payload).await;
            resp.map(|r| {
                r.choices.into_iter().next().map(|c| LlmMessage {
                    role: c.message.role,
                    content: c.message.content,
                    tool_calls: c.message.tool_calls,
                    tool_call_id: None,
                })
            })
        };

        let response_msg = response
            .map_err(|e| anyhow::anyhow!("Execution layer error: {}", e))?
            .ok_or_else(|| anyhow::anyhow!("Received None from execution layer"))?;

        let llm_content = response_msg
            .content
            .unwrap_or_else(|| "Empty result".to_string());

        let output_value = match serde_json::from_str::<Value>(&llm_content) {
            Ok(json_val) => json_val,
            Err(_) => Value::String(llm_content),
        };

        Ok(ExecutionResult {
            request_id,
            conversation_id: thread_id,
            success: true,
            output: output_value,
        })
    }
}

impl B2bIdentityAgent {
    /// Overrides the base system instructions with an Anthropic/Claude-style Markdown skill.
    pub fn with_skill(mut self, skill: Skill) -> Self {
        self.system_message = skill.instructions;
        self
    }

    async fn verify_ssi_jwt(&self, jwt: &str) -> anyhow::Result<serde_json::Value> {
        let parts: Vec<&str> = jwt.split('.').collect();
        if parts.len() != 3 {
            return Err(anyhow::anyhow!("Invalid JWT format"));
        }

        let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(parts[1])
            .map_err(|e| anyhow::anyhow!("Payload B64 decode failed: {}", e))?;
        let payload_str = String::from_utf8(payload_bytes)
            .map_err(|e| anyhow::anyhow!("Payload UTF8 decode failed: {}", e))?;
        let payload: serde_json::Value = serde_json::from_str(&payload_str)
            .map_err(|e| anyhow::anyhow!("Payload JSON parse failed: {}", e))?;

        let iss = payload
            .get("iss")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing iss claim"))?;

        if !iss.starts_with("did:twin:z") || iss.len() < 11 {
            return Err(anyhow::anyhow!("Invalid issuer DID format"));
        }

        let hex_part = &iss[10..];
        let pub_key_bytes = hex::decode(hex_part)
            .map_err(|e| anyhow::anyhow!("Hex decode of public key failed: {}", e))?;
        let pub_key_arr: [u8; 32] = pub_key_bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("Invalid public key length"))?;

        let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(&pub_key_arr)
            .map_err(|e| anyhow::anyhow!("Invalid VerifyingKey: {}", e))?;

        let message = format!("{}.{}", parts[0], parts[1]);
        let sig_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(parts[2])
            .map_err(|e| anyhow::anyhow!("Signature B64 decode failed: {}", e))?;
        let sig_arr: [u8; 64] = sig_bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("Invalid signature length"))?;
        let signature = ed25519_dalek::Signature::from_bytes(&sig_arr);

        verifying_key
            .verify(message.as_bytes(), &signature)
            .map_err(|e| anyhow::anyhow!("SSI Session JWT signature verification failed: {}", e))?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        if payload
            .get("exp")
            .and_then(|v| v.as_u64())
            .is_some_and(|exp| now > exp)
        {
            return Err(anyhow::anyhow!("SSI Session JWT expired"));
        }

        Ok(payload)
    }

    async fn verify_oauth2_jwt(
        &self,
        jwt: &str,
        jwks_url: &str,
    ) -> anyhow::Result<serde_json::Value> {
        let parts: Vec<&str> = jwt.split('.').collect();
        if parts.len() != 3 {
            return Err(anyhow::anyhow!("Invalid JWT format"));
        }

        let header_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(parts[0])
            .map_err(|e| anyhow::anyhow!("Header B64 decode failed: {}", e))?;
        let header: serde_json::Value = serde_json::from_slice(&header_bytes)
            .map_err(|e| anyhow::anyhow!("Header JSON parse failed: {}", e))?;

        let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(parts[1])
            .map_err(|e| anyhow::anyhow!("Payload B64 decode failed: {}", e))?;
        let payload: serde_json::Value = serde_json::from_slice(&payload_bytes)
            .map_err(|e| anyhow::anyhow!("Payload JSON parse failed: {}", e))?;

        let jwks_res = self
            .http_client
            .get(jwks_url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to fetch JWKS from {}: {}", jwks_url, e))?;
        let jwks: serde_json::Value = jwks_res
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to parse JWKS JSON from {}: {}", jwks_url, e))?;

        let keys = jwks
            .get("keys")
            .and_then(|k| k.as_array())
            .ok_or_else(|| anyhow::anyhow!("Invalid JWKS format at {}", jwks_url))?;

        let target_kid = header.get("kid").and_then(|k| k.as_str());
        let key_entry = if let Some(kid) = target_kid {
            keys.iter()
                .find(|k| k.get("kid").and_then(|id| id.as_str()) == Some(kid))
                .ok_or_else(|| anyhow::anyhow!("Key ID '{}' not found in JWKS", kid))?
        } else {
            keys.first()
                .ok_or_else(|| anyhow::anyhow!("JWKS contains no keys"))?
        };

        let x_str = key_entry
            .get("x")
            .and_then(|x| x.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing x field in JWK"))?;

        let pub_key_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(x_str)
            .map_err(|e| anyhow::anyhow!("Failed to decode JWK x field: {}", e))?;
        let pub_key_arr: [u8; 32] = pub_key_bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("Invalid public key length in JWK"))?;

        let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(&pub_key_arr)
            .map_err(|e| anyhow::anyhow!("Invalid VerifyingKey: {}", e))?;

        let message = format!("{}.{}", parts[0], parts[1]);
        let sig_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(parts[2])
            .map_err(|e| anyhow::anyhow!("Signature B64 decode failed: {}", e))?;
        let sig_arr: [u8; 64] = sig_bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("Invalid signature length"))?;
        let signature = ed25519_dalek::Signature::from_bytes(&sig_arr);

        verifying_key
            .verify(message.as_bytes(), &signature)
            .map_err(|e| anyhow::anyhow!("OAuth2 JWT signature verification failed: {}", e))?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        if payload
            .get("exp")
            .and_then(|v| v.as_u64())
            .is_some_and(|exp| now > exp)
        {
            return Err(anyhow::anyhow!("OAuth2 JWT expired"));
        }

        Ok(payload)
    }

    async fn extract_authentication_metadata(
        &self,
        metadata: &Option<Map<String, Value>>,
    ) -> anyhow::Result<SsiAuthenticationData> {
        let meta = match metadata {
            Some(m) => m,
            None => return Err(anyhow::anyhow!("Missing metadata for authentication")),
        };

        let jwt_str = meta.get("agent_jwt").and_then(|v| v.as_str());
        let envelope = meta.get("x_envelope").and_then(|v| v.as_str());
        let instruction = meta.get("x_instruction").and_then(|v| v.as_str());

        let mut final_tenant = meta
            .get("tenant_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let passport_hash = meta
            .get("passport_hash")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // 1. Check for VP token (envelope)
        if let Some(env) = envelope {
            // Verify JWS envelope signature and extract payload
            let verified_payload = crate::security::verify_signed(env)
                .map_err(|e| anyhow::anyhow!("VP envelope signature verification failed: {}", e))?;

            // Parse payload as JSON to extract instruction hash and VP
            let plain_payload: serde_json::Value = serde_json::from_str(&verified_payload)
                .map_err(|e| anyhow::anyhow!("Invalid VP envelope payload JSON: {}", e))?;

            let original_hash = plain_payload["body"]["instruction_hash"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing instruction_hash in DIDComm body"))?;

            // Verify Instruction Binding
            let instr = instruction
                .ok_or_else(|| anyhow::anyhow!("Missing instruction for VP verification"))?;
            let mut hasher = sha2::Sha256::new();
            hasher.update(instr.as_bytes());
            let current_hash = hex::encode(hasher.finalize());
            if current_hash != original_hash {
                return Err(anyhow::anyhow!(
                    "Instruction tampering detected! Hash mismatch."
                ));
            }

            // Extract VP token
            let vp_token = plain_payload["body"]["vp"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing vp in DIDComm body"))?;

            // Verify the VP JWT token signature using trust_auth with shared client and NATS cache
            let vp_claims = crate::security::verify_presentation(
                vp_token,
                &self.http_client,
                self.did_web_cache.as_ref(),
            )
            .await
            .map_err(|e| anyhow::anyhow!("VP token verification failed: {}", e))?;

            tracing::info!(
                "🔒 B2B Agent: Authenticated via verified VP-token for agent: {}",
                vp_claims.agent_did
            );

            if final_tenant.is_none() {
                final_tenant = Some(vp_claims.tenant_id);
            }
        } else if let Some(jwt) = jwt_str {
            // 2. Determine JWT type by inspecting payload
            let parts: Vec<&str> = jwt.split('.').collect();
            if parts.len() != 3 {
                return Err(anyhow::anyhow!("Invalid JWT format"));
            }
            let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(parts[1])
                .map_err(|e| anyhow::anyhow!("Payload B64 decode failed: {}", e))?;
            let payload: serde_json::Value = serde_json::from_slice(&payload_bytes)
                .map_err(|e| anyhow::anyhow!("Payload JSON parse failed: {}", e))?;

            let iss = payload.get("iss").and_then(|v| v.as_str()).unwrap_or("");

            if iss.starts_with("did:twin:z") {
                // Verified user Session JWT
                let ssi_payload = self.verify_ssi_jwt(jwt).await?;
                tracing::info!("🔒 B2B Agent: Authenticated via user SSI Session JWT");
                if final_tenant.is_none() {
                    final_tenant = ssi_payload
                        .get("tenant_id")
                        .and_then(|t| t.as_str())
                        .map(|t| t.to_string());
                }

                // UCAN Verification checks
                if let Some(scope) = ssi_payload.get("scope").and_then(|s| s.as_array()) {
                    let scopes: Vec<&str> = scope.iter().filter_map(|s| s.as_str()).collect();
                    if !scopes.contains(&"mcp:execute") {
                        return Err(anyhow::anyhow!(
                            "Unauthorized SSI JWT: missing 'mcp:execute' scope"
                        ));
                    }
                }
            } else {
                // Verified B2B OAuth2 token from external Identity Server
                let jwks_url = std::env::var("B2B_JWKS_URL").unwrap_or_else(|_| {
                    let identity_server = std::env::var("B2B_IDENTITY_SERVER_URL")
                        .unwrap_or_else(|_| "http://127.0.0.1:4012".to_string());
                    format!(
                        "{}/.well-known/jwks.json",
                        identity_server.trim_end_matches('/')
                    )
                });
                let oauth_payload = self.verify_oauth2_jwt(jwt, &jwks_url).await?;
                tracing::info!(
                    "🔒 B2B Agent: Authenticated via OAuth2 token issued by Identity Server ({})",
                    jwks_url
                );
                if let Some(t) = oauth_payload.get("tenant_id").and_then(|t| t.as_str()) {
                    final_tenant = Some(t.to_string());
                }
            }
        } else {
            return Err(anyhow::anyhow!(
                "B2B Agent requires either a VP envelope or an OAuth2 Bearer token"
            ));
        }

        Ok(SsiAuthenticationData {
            x_envelope: envelope.map(|s| s.to_string()),
            x_instruction: instruction.map(|s| s.to_string()),
            jwt: jwt_str.map(|s| s.to_string()),
            tenant_id: final_tenant,
            thread_id: None,
            passport_hash,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::Signer;

    #[tokio::test]
    async fn test_verify_ssi_jwt_valid() {
        use rand::RngCore;
        let mut seed = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut seed);
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
        let pub_hex = hex::encode(signing_key.verifying_key().to_bytes());
        let user_did = format!("did:twin:z{}", pub_hex);

        let claims = serde_json::json!({
            "iss": user_did,
            "sub": "did:twin:ztest-agent",
            "scope": ["mcp:execute"],
            "tenant_id": "tenant_abc",
            "exp": (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() + 60)
        });

        let header = serde_json::json!({
            "alg": "EdDSA",
            "typ": "JWT"
        });

        let header_b64 =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(header.to_string());
        let payload_b64 =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string());
        let message = format!("{}.{}", header_b64, payload_b64);

        let signature = signing_key.sign(message.as_bytes());
        let sig_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature.to_bytes());
        let token = format!("{}.{}", message, sig_b64);

        let agent = B2bIdentityAgent {
            swarm_client: Arc::new(SwarmClient::new(
                "http://localhost:8080".to_string(),
                "test-model".to_string(),
                None,
                crate::cognitive::CognitiveFormat::ChatCompletions,
            )),
            mcp_agent: None,
            system_message: "Hello".to_string(),
            nats_client: None,
            http_client: reqwest::Client::new(),
            did_web_cache: None,
            io_mapper: Arc::new(mapping::IoMapper::default()),
        };

        let decoded_payload = agent.verify_ssi_jwt(&token).await.unwrap();
        assert_eq!(decoded_payload["tenant_id"], "tenant_abc");
    }
}
