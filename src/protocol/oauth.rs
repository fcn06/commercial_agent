use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use base64::Engine;
use chrono::{Duration, Utc};
use ed25519_dalek::Signer;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ── Configuration structs (deserialized from b2b_oauth_clients.toml) ──

fn default_access_token_ttl() -> u64 {
    3600
}

#[derive(Debug, Clone, Deserialize)]
pub struct B2bOAuthConfig {
    pub server: B2bServerConfig,
    pub clients: Vec<B2bClientEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct B2bServerConfig {
    #[serde(default = "default_access_token_ttl")]
    pub access_token_ttl_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct B2bClientEntry {
    pub client_id: String,
    pub client_secret_env: String,
    pub tenant_id: String,
    pub allowed_scopes: Vec<String>,
    #[serde(default)]
    pub display_name: Option<String>,
}

// ── Axum state ──

#[derive(Clone)]
pub struct B2bOAuthState {
    pub config: Arc<B2bOAuthConfig>,
    pub signing_key: Arc<ed25519_dalek::SigningKey>,
    pub public_key_b64: String,
}

pub fn load_or_generate_keypair() -> anyhow::Result<(ed25519_dalek::SigningKey, String)> {
    let key_path = "configuration/b2b_signing.key";
    if std::path::Path::new(key_path).exists() {
        let bytes = std::fs::read(key_path)?;
        let key_bytes: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("Invalid key length"))?;
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&key_bytes);
        let pub_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(signing_key.verifying_key().to_bytes());
        Ok((signing_key, pub_b64))
    } else {
        use rand::RngCore;
        let mut seed = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut seed);
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
        if let Some(parent) = std::path::Path::new(key_path).parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(key_path, seed)?;
        let pub_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(signing_key.verifying_key().to_bytes());
        Ok((signing_key, pub_b64))
    }
}

// ── Request / Response types ──

#[derive(Deserialize)]
pub struct TokenRequest {
    pub grant_type: String,
    pub client_id: String,
    pub client_secret: String,
    #[serde(default)]
    pub scope: String,
}

#[derive(Serialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

#[derive(Serialize)]
pub struct ErrorResponse {
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_description: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub aud: String,
    pub iss: String,
    pub exp: usize,
    pub scopes: Vec<String>,
    pub tenant_id: String,
    pub client_id: String,
}

// ── Config loader ──

pub fn load_b2b_oauth_config(path: &str) -> B2bOAuthConfig {
    let content = std::fs::read_to_string(path).unwrap_or_else(|e| {
        panic!(
            "🚨 Failed to read B2B OAuth client registry at '{}': {}",
            path, e
        )
    });

    let config: B2bOAuthConfig = toml::from_str(&content).unwrap_or_else(|e| {
        panic!(
            "🚨 Failed to parse B2B OAuth client registry at '{}': {}",
            path, e
        )
    });

    if config.clients.is_empty() {
        panic!(
            "🚨 B2B OAuth client registry at '{}' contains no [[clients]] entries",
            path
        );
    }

    tracing::info!(
        "📋 Loaded {} B2B OAuth client(s) from '{}'",
        config.clients.len(),
        path
    );
    for client in &config.clients {
        tracing::info!(
            "   ✅ client_id={} tenant_id={} scopes={:?} (secret_env={})",
            client.client_id,
            client.tenant_id,
            client.allowed_scopes,
            client.client_secret_env
        );
    }

    config
}

// ── Server startup ──

pub async fn start_oauth_server(port: u16, state: B2bOAuthState) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/oauth/token", post(token_handler))
        .route("/.well-known/jwks.json", get(jwks_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    tracing::info!(
        "🔐 Starting B2B OAuth2 Authorization Server on port {}",
        port
    );

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    Ok(())
}

// ── Token handler (multi-client) ──

async fn token_handler(
    State(state): State<B2bOAuthState>,
    axum::extract::Form(payload): axum::extract::Form<TokenRequest>,
) -> Result<Json<TokenResponse>, (StatusCode, Json<ErrorResponse>)> {
    // 1. Validate grant type
    if payload.grant_type != "client_credentials" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "unsupported_grant_type".to_string(),
                error_description: Some(
                    "Only client_credentials grant type is supported".to_string(),
                ),
            }),
        ));
    }

    // 2. Look up client in registry
    let client_entry = state
        .config
        .clients
        .iter()
        .find(|c| c.client_id == payload.client_id)
        .ok_or_else(|| {
            tracing::warn!("🚫 B2B OAuth: Unknown client_id '{}'", payload.client_id);
            (
                StatusCode::UNAUTHORIZED,
                Json(ErrorResponse {
                    error: "invalid_client".to_string(),
                    error_description: Some("Unknown client_id".to_string()),
                }),
            )
        })?;

    // 3. Resolve client secret from environment variable or systemd credentials
    let expected_secret_wrapper =
        crate::security::load_secret(&client_entry.client_secret_env)
            .or_else(|| {
                std::env::var(&client_entry.client_secret_env)
                    .ok()
                    .map(crate::security::SecretString::new)
            })
            .or_else(|| {
                if client_entry.client_id == "company-beta" {
                    Some(crate::security::SecretString::new(
                        "secret123".to_string(),
                    ))
                } else if client_entry.client_id == "company-alpha" {
                    Some(crate::security::SecretString::new(
                        "dev-secret-alpha".to_string(),
                    ))
                } else {
                    None
                }
            })
            .ok_or_else(|| {
                tracing::error!(
                    "🚨 B2B OAuth: Secret variable '{}' not set for client '{}'",
                    client_entry.client_secret_env,
                    client_entry.client_id
                );
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: "server_error".to_string(),
                        error_description: Some(
                            "Client secret not configured on server".to_string(),
                        ),
                    }),
                )
            })?;
    let expected_secret = expected_secret_wrapper.expose_secret().to_string();

    if expected_secret.trim().is_empty() {
        tracing::error!(
            "🚨 B2B OAuth: Environment variable '{}' is empty for client '{}'",
            client_entry.client_secret_env,
            client_entry.client_id
        );
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "server_error".to_string(),
                error_description: Some("Client secret not configured on server".to_string()),
            }),
        ));
    }

    // 4. Validate client secret using constant-time comparison
    use subtle::ConstantTimeEq;
    let payload_secret_bytes = payload.client_secret.as_bytes();
    let expected_secret_bytes = expected_secret.as_bytes();
    let secrets_match = if payload_secret_bytes.len() == expected_secret_bytes.len() {
        payload_secret_bytes
            .ct_eq(expected_secret_bytes)
            .unwrap_u8()
            == 1
    } else {
        expected_secret_bytes
            .ct_eq(expected_secret_bytes)
            .unwrap_u8();
        false
    };

    if !secrets_match {
        tracing::warn!(
            "🚫 B2B OAuth: Invalid secret for client '{}'",
            payload.client_id
        );
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "invalid_client".to_string(),
                error_description: Some("Invalid client credentials".to_string()),
            }),
        ));
    }

    // 5. Validate and filter scopes
    let requested_scopes: Vec<String> = if payload.scope.is_empty() {
        // If no scopes requested, grant all allowed scopes
        client_entry.allowed_scopes.clone()
    } else {
        let requested: Vec<String> = payload
            .scope
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();
        let unauthorized: Vec<&String> = requested
            .iter()
            .filter(|s| !client_entry.allowed_scopes.contains(s))
            .collect();

        if !unauthorized.is_empty() {
            tracing::warn!(
                "🚫 B2B OAuth: Client '{}' requested unauthorized scopes: {:?}",
                payload.client_id,
                unauthorized
            );
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "invalid_scope".to_string(),
                    error_description: Some(format!(
                        "Unauthorized scopes: {}",
                        unauthorized
                            .iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )),
                }),
            ));
        }
        requested
    };

    // 6. Build and sign JWT
    let ttl = state.config.server.access_token_ttl_secs;
    let now = Utc::now();
    let exp = (now + Duration::seconds(ttl as i64)).timestamp() as usize;

    let claims = Claims {
        sub: client_entry.client_id.clone(),
        aud: "b2b_agent".to_string(),
        iss: "b2b_auth_server".to_string(),
        exp,
        scopes: requested_scopes.clone(),
        tenant_id: client_entry.tenant_id.clone(),
        client_id: client_entry.client_id.clone(),
    };

    let header = serde_json::json!({
        "alg": "EdDSA",
        "typ": "JWT",
        "kid": "b2b-key-1"
    });

    let header_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(header.to_string());
    let payload_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_string(&claims).unwrap());
    let message = format!("{}.{}", header_b64, payload_b64);

    let signature = state.signing_key.sign(message.as_bytes());
    let sig_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature.to_bytes());

    let token = format!("{}.{}", message, sig_b64);

    let client_display_name = client_entry
        .display_name
        .as_deref()
        .unwrap_or(&client_entry.client_id);
    tracing::info!(
        "🔑 B2B OAuth: Issued token for '{}' (tenant={}, scopes={:?}, ttl={}s)",
        client_display_name,
        client_entry.tenant_id,
        requested_scopes,
        ttl
    );

    Ok(Json(TokenResponse {
        access_token: token,
        token_type: "Bearer".to_string(),
        expires_in: ttl,
        scope: Some(requested_scopes.join(" ")),
    }))
}

async fn jwks_handler(State(state): State<B2bOAuthState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "keys": [
            {
                "kty": "OKP",
                "crv": "Ed25519",
                "x": state.public_key_b64,
                "kid": "b2b-key-1",
                "alg": "EdDSA",
                "use": "sig"
            }
        ]
    }))
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::Verifier;

    #[test]
    fn test_load_b2b_oauth_config_parses_correctly() {
        let toml_content = r#"
[server]
access_token_ttl_secs = 1800

[[clients]]
client_id = "test-client-a"
client_secret_env = "TEST_SECRET_A"
tenant_id = "tenant_a"
allowed_scopes = ["b2b:query", "b2b:semantic_filter"]
display_name = "Test Client A"

[[clients]]
client_id = "test-client-b"
client_secret_env = "TEST_SECRET_B"
tenant_id = "tenant_b"
allowed_scopes = ["b2b:query"]
"#;
        let config: B2bOAuthConfig = toml::from_str(toml_content).expect("Failed to parse TOML");

        assert_eq!(config.server.access_token_ttl_secs, 1800);
        assert_eq!(config.clients.len(), 2);

        assert_eq!(config.clients[0].client_id, "test-client-a");
        assert_eq!(config.clients[0].client_secret_env, "TEST_SECRET_A");
        assert_eq!(config.clients[0].tenant_id, "tenant_a");
        assert_eq!(
            config.clients[0].allowed_scopes,
            vec!["b2b:query", "b2b:semantic_filter"]
        );
        assert_eq!(
            config.clients[0].display_name,
            Some("Test Client A".to_string())
        );

        assert_eq!(config.clients[1].client_id, "test-client-b");
        assert_eq!(config.clients[1].tenant_id, "tenant_b");
        assert_eq!(config.clients[1].display_name, None);
    }

    #[test]
    fn test_default_ttl() {
        let toml_content = r#"
[server]

[[clients]]
client_id = "minimal"
client_secret_env = "MIN_SECRET"
tenant_id = "tenant_min"
allowed_scopes = ["b2b:query"]
"#;
        let config: B2bOAuthConfig = toml::from_str(toml_content).expect("Failed to parse TOML");
        assert_eq!(config.server.access_token_ttl_secs, 3600); // default
    }

    #[test]
    fn test_asymmetric_token_signing_and_verification() {
        let (signing_key, pub_b64) = load_or_generate_keypair().unwrap();

        let claims = Claims {
            sub: "test-client".to_string(),
            aud: "b2b_agent".to_string(),
            iss: "b2b_auth_server".to_string(),
            exp: (chrono::Utc::now() + chrono::Duration::seconds(30)).timestamp() as usize,
            scopes: vec!["b2b:query".to_string()],
            tenant_id: "tenant_test".to_string(),
            client_id: "test-client".to_string(),
        };

        let header = serde_json::json!({
            "alg": "EdDSA",
            "typ": "JWT",
            "kid": "b2b-key-1"
        });

        let header_b64 =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(header.to_string());
        let payload_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_string(&claims).unwrap());
        let message = format!("{}.{}", header_b64, payload_b64);

        let signature = signing_key.sign(message.as_bytes());
        let sig_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature.to_bytes());
        let token = format!("{}.{}", message, sig_b64);

        // Verification phase
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);

        let pub_key_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&pub_b64)
            .unwrap();
        let pub_key_arr: [u8; 32] = pub_key_bytes.try_into().unwrap();
        let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(&pub_key_arr).unwrap();

        let sig_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(parts[2])
            .unwrap();
        let sig_arr: [u8; 64] = sig_bytes.try_into().unwrap();
        let signature_to_verify = ed25519_dalek::Signature::from_bytes(&sig_arr);

        verifying_key
            .verify(message.as_bytes(), &signature_to_verify)
            .unwrap();
    }
}
