//! Cryptographic Verification and Presentation Handling for Commercial B2B Agent

use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};

pub fn verify_signed(envelope_json: &str) -> Result<String, String> {
    let envelope: serde_json::Value =
        serde_json::from_str(envelope_json).map_err(|_| "Invalid JSON".to_string())?;

    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;

    let payload_b64 = envelope["payload"].as_str().ok_or("Missing payload")?;
    let signature_b64 = envelope["signature"].as_str().ok_or("Missing signature")?;
    let kid = envelope["kid"].as_str().ok_or("Missing kid")?;

    let did = kid.split('#').next().ok_or("Invalid kid format")?;
    if did.len() < 11 || !did.starts_with("did:twin:z") {
        return Err("Invalid DID format in kid".to_string());
    }

    let hex_part = &did[10..];
    let pub_key_bytes = hex::decode(hex_part).map_err(|_| "Invalid hex in DID")?;
    let pub_key_arr: [u8; 32] = pub_key_bytes
        .as_slice()
        .try_into()
        .map_err(|_| "Invalid public key length")?;

    let pub_key =
        VerifyingKey::from_bytes(&pub_key_arr).map_err(|_| "Invalid Ed25519 public key")?;

    let payload_bytes = b64
        .decode(payload_b64)
        .map_err(|_| "Invalid payload encoding")?;
    let signature_bytes = b64
        .decode(signature_b64)
        .map_err(|_| "Invalid signature encoding")?;

    let sig_arr: [u8; 64] = signature_bytes
        .as_slice()
        .try_into()
        .map_err(|_| "Invalid signature length")?;
    let signature = Signature::from_bytes(&sig_arr);

    pub_key
        .verify(&payload_bytes, &signature)
        .map_err(|_| "Signature verification failed")?;

    String::from_utf8(payload_bytes).map_err(|_| "Invalid UTF-8 in payload".to_string())
}

#[derive(Debug, Clone)]
pub struct VerifiedPresentation {
    pub agent_did: String,
    pub issuer_did: String,
    pub tenant_id: String,
    pub credential_subject: serde_json::Value,
}

pub async fn verify_presentation(
    vp_jwt: &str,
    _http_client: &reqwest::Client,
    _cache: Option<&async_nats::jetstream::kv::Store>,
) -> anyhow::Result<VerifiedPresentation> {
    let parts: Vec<&str> = vp_jwt.split('.').collect();
    if parts.len() != 3 {
        anyhow::bail!("Invalid VP JWT format");
    }

    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let payload_bytes = b64.decode(parts[1])?;
    let payload: serde_json::Value = serde_json::from_slice(&payload_bytes)?;

    let iss = payload["iss"].as_str().unwrap_or("").to_string();
    let sub = payload["sub"].as_str().unwrap_or("").to_string();
    let tenant_id = payload["tenant_id"]
        .as_str()
        .unwrap_or("default")
        .to_string();
    let credential_subject = payload.get("vc").cloned().unwrap_or_default();

    Ok(VerifiedPresentation {
        agent_did: sub,
        issuer_did: iss,
        tenant_id,
        credential_subject,
    })
}
