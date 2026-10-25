//! Cognitive Ingress Domain (Tier 1: Swarm Gateway)
//!
//! Provides multi-format cognitive interaction with Swarm Gateway:
//! - Format 1: Stateless OpenAI Chat Completions (ChatCompletionRequest / Response)
//! - Format 2: Stateful Open Responses (CreateResponseRequest / ResponseObject with session chaining)
//! - Format 3: Google Gemini Format (GeminiInteractionRequest / GeminiContent)

pub mod client;

#[allow(unused_imports)]
pub use client::{
    CognitiveFormat, ContentPart, CreateResponseRequest, GeminiContent, GeminiFunctionCall,
    GeminiFunctionResponse, GeminiInlineData, GeminiInteractionRequest, GeminiPart, ResponseItem,
    ResponseObject, ResponseUsage, ResponsesInput, Role, SwarmClient,
};
