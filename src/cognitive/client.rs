//! Client for Swarm Gateway (Tier 1 Cognitive Ingress)
//! Supports three cognitive formats:
//! 1. ChatCompletionRequest (stateless OpenAI wire format)
//! 2. Responses (stateful Open Responses standard with session chaining)
//! 3. Gemini format (Google Gemini native interaction format)

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use tracing::{debug, error, warn};

use llm_api::chat::{ChatCompletionRequest, ChatCompletionResponse};
use llm_api::tools::Tool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CognitiveFormat {
    #[default]
    ChatCompletions,
    Responses,
    Gemini,
}

impl FromStr for CognitiveFormat {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().trim() {
            "responses" | "open_responses" | "stateful" => Ok(CognitiveFormat::Responses),
            "gemini" | "google" => Ok(CognitiveFormat::Gemini),
            _ => Ok(CognitiveFormat::ChatCompletions),
        }
    }
}

impl std::fmt::Display for CognitiveFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CognitiveFormat::ChatCompletions => write!(f, "chat_completions"),
            CognitiveFormat::Responses => write!(f, "responses"),
            CognitiveFormat::Gemini => write!(f, "gemini"),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Format 2: Stateful Open Responses Schema
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseItem {
    Message {
        id: String,
        role: Role,
        content: Vec<ContentPart>,
    },
    Reasoning {
        id: String,
        thought_process: String,
        signature: Option<String>,
    },
    FunctionCall {
        id: String,
        call_id: String,
        name: String,
        arguments: String,
    },
    FunctionCallOutput {
        id: String,
        call_id: String,
        output: String,
        is_error: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text { text: String },
    Image { media_type: String, data_base64: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ResponsesInput {
    Text(String),
    Items(Vec<ResponseItem>),
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CreateResponseRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub input: Option<ResponsesInput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_response_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResponseObject {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub output: Vec<ResponseItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<ResponseUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ResponseUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub total_tokens: u32,
}

// ═══════════════════════════════════════════════════════════════════════════
// Format 3: Google Gemini Format Schema
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GeminiInteractionRequest {
    pub contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_interaction_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GeminiContent {
    pub role: String,
    pub parts: Vec<GeminiPart>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
#[serde(untagged)]
pub enum GeminiPart {
    Text { text: String },
    InlineData { inline_data: GeminiInlineData },
    FunctionCall { function_call: GeminiFunctionCall },
    FunctionResponse { function_response: GeminiFunctionResponse },
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GeminiInlineData {
    pub mime_type: String,
    pub data: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GeminiFunctionCall {
    pub name: String,
    pub args: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GeminiFunctionResponse {
    pub name: String,
    pub response: serde_json::Value,
}

// ═══════════════════════════════════════════════════════════════════════════
// Swarm Gateway Client
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Clone)]
pub struct SwarmClient {
    pub gateway_url: String,
    pub model_id: String,
    pub gateway_token: Option<String>,
    pub format: CognitiveFormat,
    http_client: reqwest::Client,
}

impl SwarmClient {
    pub fn new(
        gateway_url: String,
        model_id: String,
        gateway_token: Option<String>,
        format: CognitiveFormat,
    ) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .unwrap_or_default();

        Self {
            gateway_url: gateway_url.trim_end_matches('/').to_string(),
            model_id,
            gateway_token,
            format,
            http_client,
        }
    }

    /// Resolves Swarm client from environment variables with fallbacks
    pub fn from_env_or_config(
        configured_url: Option<&str>,
        configured_model: Option<&str>,
        configured_format: Option<&str>,
    ) -> Self {
        let gateway_url = std::env::var("SWARM_GATEWAY_URL")
            .ok()
            .or_else(|| configured_url.map(|s| s.to_string()))
            .unwrap_or_else(|| "http://127.0.0.1:8080".to_string());

        let model_id = std::env::var("SWARM_MODEL_ID")
            .or_else(|_| std::env::var("AGENT_MODEL_ID"))
            .ok()
            .or_else(|| configured_model.map(|s| s.to_string()))
            .unwrap_or_else(|| "openai/gpt-oss-20b".to_string());

        let gateway_token = std::env::var("SWARM_GATEWAY_TOKEN")
            .or_else(|_| std::env::var("SWARM_API_KEY"))
            .ok()
            .filter(|s| !s.trim().is_empty());

        let format_str = std::env::var("SWARM_COGNITIVE_FORMAT")
            .ok()
            .or_else(|| configured_format.map(|s| s.to_string()))
            .unwrap_or_else(|| "chat_completions".to_string());

        let format = CognitiveFormat::from_str(&format_str).unwrap_or_default();

        Self::new(gateway_url, model_id, gateway_token, format)
    }

    /// Performs a health check against Swarm Gateway (GET /health)
    pub async fn check_health(&self) -> Result<bool> {
        let url = format!("{}/health", self.gateway_url);
        let mut req = self.http_client.get(&url);
        if let Some(token) = &self.gateway_token {
            req = req.bearer_auth(token);
        }

        match req.send().await {
            Ok(res) if res.status().is_success() => Ok(true),
            Ok(res) => {
                warn!("⚠️ Swarm Gateway health check returned status: {}", res.status());
                Ok(false)
            }
            Err(e) => {
                warn!("⚠️ Swarm Gateway connection failed ({}): {}", url, e);
                Ok(false)
            }
        }
    }

    /// 1. Execute Stateless OpenAI Chat Completion (POST /v1/chat/completions)
    pub async fn call_chat_completions(
        &self,
        request: &ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse> {
        let url = format!("{}/v1/chat/completions", self.gateway_url);
        debug!("Dispatching ChatCompletion to Swarm Gateway: {}", url);

        let mut req = self.http_client.post(&url).json(request);
        if let Some(token) = &self.gateway_token {
            req = req.bearer_auth(token);
        }

        let res = req
            .send()
            .await
            .with_context(|| format!("Failed to connect to Swarm Gateway at {}", url))?;

        if !res.status().is_success() {
            let status = res.status();
            let err_body = res.text().await.unwrap_or_default();
            error!("Swarm Gateway ChatCompletions error [{}]: {}", status, err_body);
            anyhow::bail!("Swarm Gateway returned {}: {}", status, err_body);
        }

        let parsed: ChatCompletionResponse = res
            .json()
            .await
            .context("Failed to deserialize ChatCompletionResponse from Swarm Gateway")?;

        Ok(parsed)
    }

    /// 2. Execute Stateful Open Responses Turn (POST /v1/responses)
    pub async fn call_responses(
        &self,
        request: &CreateResponseRequest,
    ) -> Result<ResponseObject> {
        let url = format!("{}/v1/responses", self.gateway_url);
        debug!("Dispatching Open Response turn to Swarm Gateway: {}", url);

        let mut req = self.http_client.post(&url).json(request);
        if let Some(token) = &self.gateway_token {
            req = req.bearer_auth(token);
        }

        let res = req
            .send()
            .await
            .with_context(|| format!("Failed to connect to Swarm Gateway at {}", url))?;

        if !res.status().is_success() {
            let status = res.status();
            let err_body = res.text().await.unwrap_or_default();
            error!("Swarm Gateway Responses error [{}]: {}", status, err_body);
            anyhow::bail!("Swarm Gateway returned {}: {}", status, err_body);
        }

        let parsed: ResponseObject = res
            .json()
            .await
            .context("Failed to deserialize ResponseObject from Swarm Gateway")?;

        Ok(parsed)
    }

    /// 3. Execute Google Gemini Interaction
    /// Routes through Swarm Gateway /v1/responses translating to ResponseItems
    pub async fn call_gemini(
        &self,
        request: &GeminiInteractionRequest,
        _tools: Option<Vec<Tool>>,
    ) -> Result<ResponseObject> {
        let mut items = Vec::new();
        for content in &request.contents {
            let role = match content.role.as_str() {
                "model" => Role::Assistant,
                _ => Role::User,
            };

            for part in &content.parts {
                match part {
                    GeminiPart::Text { text } => {
                        items.push(ResponseItem::Message {
                            id: format!("msg_{}", uuid::Uuid::new_v4()),
                            role: role.clone(),
                            content: vec![ContentPart::Text { text: text.clone() }],
                        });
                    }
                    GeminiPart::FunctionCall { function_call } => {
                        items.push(ResponseItem::FunctionCall {
                            id: format!("call_{}", uuid::Uuid::new_v4()),
                            call_id: format!("fcall_{}", uuid::Uuid::new_v4()),
                            name: function_call.name.clone(),
                            arguments: function_call.args.to_string(),
                        });
                    }
                    GeminiPart::FunctionResponse { function_response } => {
                        items.push(ResponseItem::FunctionCallOutput {
                            id: format!("out_{}", uuid::Uuid::new_v4()),
                            call_id: function_response.name.clone(),
                            output: function_response.response.to_string(),
                            is_error: false,
                        });
                    }
                    GeminiPart::InlineData { inline_data } => {
                        items.push(ResponseItem::Message {
                            id: format!("msg_{}", uuid::Uuid::new_v4()),
                            role: role.clone(),
                            content: vec![ContentPart::Image {
                                media_type: inline_data.mime_type.clone(),
                                data_base64: inline_data.data.clone(),
                            }],
                        });
                    }
                }
            }
        }

        let resp_req = CreateResponseRequest {
            model: Some(self.model_id.clone()),
            input: Some(ResponsesInput::Items(items)),
            previous_response_id: request.previous_interaction_id.clone(),
            stream: Some(false),
            temperature: Some(0.0),
            max_output_tokens: Some(4096),
        };

        self.call_responses(&resp_req).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cognitive_format_parsing() {
        assert_eq!(
            CognitiveFormat::from_str("chat_completions").unwrap(),
            CognitiveFormat::ChatCompletions
        );
        assert_eq!(
            CognitiveFormat::from_str("responses").unwrap(),
            CognitiveFormat::Responses
        );
        assert_eq!(
            CognitiveFormat::from_str("stateful").unwrap(),
            CognitiveFormat::Responses
        );
        assert_eq!(
            CognitiveFormat::from_str("gemini").unwrap(),
            CognitiveFormat::Gemini
        );
        assert_eq!(
            CognitiveFormat::from_str("unknown").unwrap(),
            CognitiveFormat::ChatCompletions
        );
    }

    #[test]
    fn test_open_responses_serialization() {
        let items = vec![
            ResponseItem::Message {
                id: "msg_1".to_string(),
                role: Role::User,
                content: vec![ContentPart::Text {
                    text: "What is the weather?".to_string(),
                }],
            },
            ResponseItem::FunctionCall {
                id: "call_1".to_string(),
                call_id: "fcall_1".to_string(),
                name: "claw_weather".to_string(),
                arguments: "{\"city\":\"Zagreb\"}".to_string(),
            },
        ];

        let req = CreateResponseRequest {
            model: Some("openai/gpt-oss-20b".to_string()),
            input: Some(ResponsesInput::Items(items)),
            previous_response_id: Some("resp_prev_123".to_string()),
            stream: Some(false),
            temperature: Some(0.0),
            max_output_tokens: Some(1024),
        };

        let json = serde_json::to_string(&req).expect("Serialization failed");
        assert!(json.contains("claw_weather"));
        assert!(json.contains("resp_prev_123"));
        assert!(json.contains("function_call"));
    }

    #[test]
    fn test_gemini_interaction_serialization() {
        let req = GeminiInteractionRequest {
            contents: vec![GeminiContent {
                role: "user".to_string(),
                parts: vec![
                    GeminiPart::Text {
                        text: "Weather query".to_string(),
                    },
                    GeminiPart::FunctionResponse {
                        function_response: GeminiFunctionResponse {
                            name: "claw_weather".to_string(),
                            response: serde_json::json!({ "temperature": 18, "city": "Zagreb" }),
                        },
                    },
                ],
            }],
            previous_interaction_id: Some("interact_123".to_string()),
        };

        let json = serde_json::to_string(&req).expect("Gemini Serialization failed");
        assert!(json.contains("claw_weather"));
        assert!(json.contains("Zagreb"));
        assert!(json.contains("previousInteractionId"));
    }
}

