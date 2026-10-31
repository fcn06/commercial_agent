//! Standalone MCP Runtime and Tool Execution Engine for Commercial B2B Agent
//! Connects to Swarm Gateway for cognition (ChatCompletions, Responses, and Gemini formats)
//! and to Marketplace Trust Gateway for verified execution.

use anyhow::Result;
use configuration::McpRuntimeConfig;
use llm_api::chat::{ChatCompletionRequest, ChatCompletionResponse, Message, ToolChoice};
use llm_api::tools::{FunctionDefinition, FunctionParameters, Tool};
use std::sync::Arc;
use tracing::{info, warn};

use crate::cognitive::{
    CognitiveFormat, ContentPart, CreateResponseRequest, GeminiContent,
    GeminiFunctionResponse, GeminiInteractionRequest, GeminiPart, ResponseItem,
    ResponsesInput, Role, SwarmClient,
};

#[derive(Debug, Clone, Default)]
pub struct SsiAuthenticationData {
    pub x_envelope: Option<String>,
    pub x_instruction: Option<String>,
    pub jwt: Option<String>,
    pub tenant_id: Option<String>,
    pub thread_id: Option<String>,
    pub passport_hash: Option<String>,
}

#[derive(Clone)]
pub struct McpAgent {
    swarm_client: Arc<SwarmClient>,
    config: McpRuntimeConfig,
    trust_gateway_url: String,
    http_client: reqwest::Client,
}

impl McpAgent {
    pub async fn new(
        config: McpRuntimeConfig,
        _api_key_override: Option<String>,
        _nats_client: Option<Arc<async_nats::Client>>,
    ) -> Result<Self> {
        let swarm_client = Arc::new(SwarmClient::from_env_or_config(
            Some(&config.agent_mcp_llm_url),
            Some(&config.agent_mcp_model_id),
            None,
        ));

        let trust_gateway_url = std::env::var("TRUST_GATEWAY_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8081".to_string());

        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_default();

        Ok(Self {
            swarm_client,
            config,
            trust_gateway_url,
            http_client,
        })
    }

    pub fn with_swarm_client(mut self, client: Arc<SwarmClient>) -> Self {
        self.swarm_client = client;
        self
    }

    /// Fetches tools from the Trust Gateway via MCP HTTP protocol (tools/list)
    pub async fn fetch_tools(&self) -> Vec<Tool> {
        let url = format!(
            "{}/v1/mcp/messages",
            self.trust_gateway_url.trim_end_matches('/')
        );
        let rpc_req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {}
        });

        match self.http_client.post(&url).json(&rpc_req).send().await {
            Ok(res) => {
                if let Ok(json) = res.json::<serde_json::Value>().await {
                    if let Some(tools_array) = json
                        .get("result")
                        .and_then(|r| r.get("tools"))
                        .and_then(|t| t.as_array())
                    {
                        let mut tools = Vec::new();
                        for item in tools_array {
                            let name = item
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let description = item
                                .get("description")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let schema = item
                                .get("inputSchema")
                                .or_else(|| item.get("input_schema"))
                                .cloned()
                                .unwrap_or_else(|| serde_json::json!({"type": "object"}));

                            let properties =
                                schema.get("properties").cloned().unwrap_or_else(|| {
                                    serde_json::Value::Object(serde_json::Map::new())
                                });

                            if !name.is_empty() {
                                tools.push(Tool {
                                    r#type: "function".to_string(),
                                    function: FunctionDefinition {
                                        name,
                                        description,
                                        parameters: FunctionParameters {
                                            r#type: "object".to_string(),
                                            properties,
                                            required: None,
                                        },
                                    },
                                });
                            }
                        }
                        info!("📦 Loaded {} tools from Trust Gateway", tools.len());
                        return tools;
                    }
                }
            }
            Err(e) => {
                warn!(
                    "⚠️ Failed to fetch tools from Trust Gateway ({}): {}",
                    url, e
                );
            }
        }
        vec![]
    }

    /// Executes a tool call against the Trust Gateway (tools/call)
    pub async fn execute_tool(
        &self,
        tool_name: &str,
        arguments_json: &str,
        auth_data: Option<&SsiAuthenticationData>,
    ) -> Result<String> {
        let url = format!(
            "{}/v1/mcp/messages",
            self.trust_gateway_url.trim_end_matches('/')
        );
        let tenant = auth_data
            .and_then(|a| a.tenant_id.as_deref())
            .unwrap_or("company-beta");
        let jwt = auth_data.and_then(|a| a.jwt.as_deref()).unwrap_or("");

        let parsed_args: serde_json::Value =
            serde_json::from_str(arguments_json).unwrap_or_else(|_| serde_json::json!({}));

        let rpc_req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": tool_name,
                "arguments": parsed_args
            }
        });

        let trust_api_key = std::env::var("TRUST_GATEWAY_API_KEY")
            .or_else(|_| std::env::var("TRUST_GATEWAY_TOKEN"))
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "sk_dev_local".to_string());

        let mut req = self.http_client.post(&url);

        if !trust_api_key.is_empty() {
            req = req
                .header("authorization", format!("Bearer {}", trust_api_key))
                .header("x-tenant-id", tenant);
        } else {
            req = req.header("x-tenant-id", tenant);
            if !jwt.is_empty() {
                req = req.header("authorization", format!("Bearer {}", jwt));
            }
        }

        let res = req.json(&rpc_req).send().await?;
        let res_json: serde_json::Value = res.json().await?;

        if let Some(content) = res_json
            .get("result")
            .and_then(|r| r.get("content"))
            .and_then(|c| c.as_array())
        {
            let mut out = String::new();
            for c in content {
                if let Some(text) = c.get("text").and_then(|t| t.as_str()) {
                    out.push_str(text);
                }
            }
            Ok(out)
        } else if let Some(err) = res_json.get("error") {
            Ok(format!("Error: {}", err))
        } else {
            Ok(res_json.to_string())
        }
    }

    /// Main agent execution loop dispatching to the configured CognitiveFormat
    pub async fn run_agent_internal(
        &self,
        user_message: Message,
        auth_data: Option<SsiAuthenticationData>,
        system_prompt_override: Option<String>,
    ) -> Result<Option<Message>> {
        let tools = self.fetch_tools().await;
        let system_prompt =
            system_prompt_override.unwrap_or_else(|| self.config.agent_mcp_system_prompt.clone());

        match self.swarm_client.format {
            CognitiveFormat::ChatCompletions => {
                self.run_chat_completions_loop(user_message, auth_data, system_prompt, tools)
                    .await
            }
            CognitiveFormat::Responses => {
                self.run_responses_loop(user_message, auth_data, system_prompt, tools)
                    .await
            }
            CognitiveFormat::Gemini => {
                self.run_gemini_loop(user_message, auth_data, system_prompt, tools)
                    .await
            }
        }
    }

    /// Format 1: Stateless OpenAI Chat Completions Loop (POST /v1/chat/completions)
    async fn run_chat_completions_loop(
        &self,
        user_message: Message,
        auth_data: Option<SsiAuthenticationData>,
        system_prompt: String,
        tools: Vec<Tool>,
    ) -> Result<Option<Message>> {
        let mut messages = vec![
            Message {
                role: "system".to_string(),
                content: Some(system_prompt),
                tool_calls: None,
                tool_call_id: None,
            },
            user_message,
        ];

        let max_loops = self.config.agent_mcp_max_loops.max(4);

        for loop_idx in 0..max_loops {
            info!(
                "Agent Loop (ChatCompletions): {}/{} - State: Thinking via Swarm Gateway",
                loop_idx + 1,
                max_loops
            );

            let has_tools = !tools.is_empty();
            let tool_choice = if has_tools {
                Some(ToolChoice::String(
                    self.config.agent_mcp_tool_choice_auto.clone(),
                ))
            } else {
                None
            };
            let tools_opt = if has_tools {
                Some(tools.clone())
            } else {
                None
            };

            let req_payload = ChatCompletionRequest {
                model: self.swarm_client.model_id.clone(),
                messages: messages.clone(),
                temperature: Some(0.0),
                max_tokens: Some(4096),
                top_p: Some(1.0),
                stop: None,
                stream: Some(false),
                tools: tools_opt,
                tool_choice,
            };

            let resp: ChatCompletionResponse = match self
                .swarm_client
                .call_chat_completions(&req_payload)
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    warn!("⚠️ LLM thinking step failed: {}", e);
                    return Ok(Some(Message {
                        role: "assistant".to_string(),
                        content: Some("I encountered a temporary error while processing your request. Please try again shortly.".to_string()),
                        tool_calls: None,
                        tool_call_id: None,
                    }));
                }
            };

            let choice = resp.choices.first().cloned();
            if choice.is_none() {
                break;
            }
            let choice = choice.unwrap();

            if let Some(tool_calls) = choice
                .message
                .tool_calls
                .as_ref()
                .filter(|tc| !tc.is_empty())
            {
                info!("Agent requested {} tool call(s)", tool_calls.len());
                messages.push(Message {
                    role: choice.message.role.clone(),
                    content: choice.message.content.clone(),
                    tool_calls: choice.message.tool_calls.clone(),
                    tool_call_id: None,
                });

                for call in tool_calls {
                    info!(
                        "Executing tool '{}' against Trust Gateway...",
                        call.function.name
                    );
                    let result_str = self
                        .execute_tool(
                            &call.function.name,
                            &call.function.arguments,
                            auth_data.as_ref(),
                        )
                        .await
                        .unwrap_or_else(|e| format!("Execution error: {}", e));

                    messages.push(Message {
                        role: "tool".to_string(),
                        content: Some(result_str),
                        tool_calls: None,
                        tool_call_id: Some(call.id.clone()),
                    });
                }
            } else {
                info!(
                    "Agent finished thinking (stop reason: {:?})",
                    choice.finish_reason
                );
                return Ok(Some(Message {
                    role: choice.message.role,
                    content: choice.message.content,
                    tool_calls: None,
                    tool_call_id: None,
                }));
            }
        }

        Ok(messages
            .into_iter()
            .filter(|m| m.role == "assistant")
            .last())
    }

    /// Format 2: Stateful Open Responses Loop (POST /v1/responses)
    async fn run_responses_loop(
        &self,
        user_message: Message,
        auth_data: Option<SsiAuthenticationData>,
        system_prompt: String,
        tools: Vec<Tool>,
    ) -> Result<Option<Message>> {
        let max_loops = self.config.agent_mcp_max_loops.max(4);
        let mut previous_response_id: Option<String> = None;

        let mut full_system_prompt = system_prompt;
        if !tools.is_empty() {
            full_system_prompt.push_str("\n\n### Available Tools (invoked via function calls):\n");
            for t in &tools {
                full_system_prompt.push_str(&format!(
                    "- `{}`: {}\n  Parameters: {}\n",
                    t.function.name,
                    t.function.description,
                    serde_json::to_string(&t.function.parameters.properties).unwrap_or_default()
                ));
            }
        }

        let user_text = user_message.content.unwrap_or_default();
        let initial_items = vec![
            ResponseItem::Message {
                id: format!("sys_{}", uuid::Uuid::new_v4()),
                role: Role::System,
                content: vec![ContentPart::Text {
                    text: full_system_prompt,
                }],
            },
            ResponseItem::Message {
                id: format!("usr_{}", uuid::Uuid::new_v4()),
                role: Role::User,
                content: vec![ContentPart::Text { text: user_text }],
            },
        ];

        let mut next_input = ResponsesInput::Items(initial_items);

        for loop_idx in 0..max_loops {
            info!(
                "Agent Loop (Open Responses): {}/{} - Session: {:?}",
                loop_idx + 1,
                max_loops,
                previous_response_id
            );

            let req = CreateResponseRequest {
                model: Some(self.swarm_client.model_id.clone()),
                input: Some(next_input),
                previous_response_id: previous_response_id.clone(),
                stream: Some(false),
                temperature: Some(0.0),
                max_output_tokens: Some(4096),
            };

            let resp = match self.swarm_client.call_responses(&req).await {
                Ok(r) => r,
                Err(e) => {
                    warn!("⚠️ Swarm Gateway Responses turn failed: {}", e);
                    return Ok(Some(Message {
                        role: "assistant".to_string(),
                        content: Some("I encountered a temporary error while processing your request. Please try again shortly.".to_string()),
                        tool_calls: None,
                        tool_call_id: None,
                    }));
                }
            };

            previous_response_id = Some(resp.id.clone());

            let mut tool_calls: Vec<(String, String, String)> = Vec::new();
            let mut final_content = None;

            for item in resp.output {
                match item {
                    ResponseItem::FunctionCall {
                        call_id,
                        name,
                        arguments,
                        ..
                    } => {
                        tool_calls.push((call_id, name, arguments));
                    }
                    ResponseItem::Message {
                        role: Role::Assistant,
                        content,
                        ..
                    } => {
                        for part in content {
                            if let ContentPart::Text { text } = part {
                                final_content = Some(text);
                            }
                        }
                    }
                    _ => {}
                }
            }

            if !tool_calls.is_empty() {
                info!("Responses turn requested {} function call(s)", tool_calls.len());
                let mut tool_output_items = Vec::new();

                for (call_id, name, arguments) in tool_calls {
                    info!("Executing function '{}' via Trust Gateway...", name);
                    let result_str = self
                        .execute_tool(&name, &arguments, auth_data.as_ref())
                        .await
                        .unwrap_or_else(|e| format!("Execution error: {}", e));

                    tool_output_items.push(ResponseItem::FunctionCallOutput {
                        id: format!("out_{}", uuid::Uuid::new_v4()),
                        call_id,
                        output: result_str,
                        is_error: false,
                    });
                }

                next_input = ResponsesInput::Items(tool_output_items);
            } else {
                info!("Responses turn finished thinking");
                return Ok(Some(Message {
                    role: "assistant".to_string(),
                    content: final_content.or_else(|| Some("Completed".to_string())),
                    tool_calls: None,
                    tool_call_id: None,
                }));
            }
        }

        Ok(Some(Message {
            role: "assistant".to_string(),
            content: Some("Max iterations reached".to_string()),
            tool_calls: None,
            tool_call_id: None,
        }))
    }

    /// Format 3: Google Gemini Format Loop
    async fn run_gemini_loop(
        &self,
        user_message: Message,
        auth_data: Option<SsiAuthenticationData>,
        system_prompt: String,
        tools: Vec<Tool>,
    ) -> Result<Option<Message>> {
        let max_loops = self.config.agent_mcp_max_loops.max(4);
        let user_text = user_message.content.unwrap_or_default();
        let prompt_combined = format!("{}\n\n{}", system_prompt, user_text);

        let mut contents = vec![GeminiContent {
            role: "user".to_string(),
            parts: vec![GeminiPart::Text {
                text: prompt_combined,
            }],
        }];

        let mut previous_interaction_id: Option<String> = None;

        for loop_idx in 0..max_loops {
            info!(
                "Agent Loop (Gemini Format): {}/{} - State: Thinking via Swarm Gateway",
                loop_idx + 1,
                max_loops
            );

            let gemini_req = GeminiInteractionRequest {
                contents: contents.clone(),
                previous_interaction_id: previous_interaction_id.clone(),
            };

            let resp = match self
                .swarm_client
                .call_gemini(&gemini_req, Some(tools.clone()))
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    warn!("⚠️ Gemini interaction turn failed: {}", e);
                    return Ok(Some(Message {
                        role: "assistant".to_string(),
                        content: Some("I encountered a temporary error while processing your request. Please try again shortly.".to_string()),
                        tool_calls: None,
                        tool_call_id: None,
                    }));
                }
            };

            previous_interaction_id = Some(resp.id.clone());

            let mut tool_calls: Vec<(String, String, String)> = Vec::new();
            let mut text_response = None;

            for item in resp.output {
                match item {
                    ResponseItem::FunctionCall {
                        call_id,
                        name,
                        arguments,
                        ..
                    } => {
                        tool_calls.push((call_id, name, arguments));
                    }
                    ResponseItem::Message {
                        role: Role::Assistant,
                        content,
                        ..
                    } => {
                        for part in content {
                            if let ContentPart::Text { text } = part {
                                text_response = Some(text);
                            }
                        }
                    }
                    _ => {}
                }
            }

            if !tool_calls.is_empty() {
                info!("Gemini turn returned {} function call(s)", tool_calls.len());
                let mut response_parts = Vec::new();

                for (_call_id, name, arguments) in tool_calls {
                    info!("Executing Gemini tool '{}' via Trust Gateway...", name);
                    let result_str = self
                        .execute_tool(&name, &arguments, auth_data.as_ref())
                        .await
                        .unwrap_or_else(|e| format!("Execution error: {}", e));

                    let response_json = serde_json::from_str::<serde_json::Value>(&result_str)
                        .unwrap_or_else(|_| serde_json::json!({ "result": result_str }));

                    response_parts.push(GeminiPart::FunctionResponse {
                        function_response: GeminiFunctionResponse {
                            name,
                            response: response_json,
                        },
                    });
                }

                contents.push(GeminiContent {
                    role: "user".to_string(),
                    parts: response_parts,
                });
            } else {
                info!("Gemini turn finished thinking");
                return Ok(Some(Message {
                    role: "assistant".to_string(),
                    content: text_response.or_else(|| Some("Completed".to_string())),
                    tool_calls: None,
                    tool_call_id: None,
                }));
            }
        }

        Ok(Some(Message {
            role: "assistant".to_string(),
            content: Some("Max iterations reached".to_string()),
            tool_calls: None,
            tool_call_id: None,
        }))
    }
}
