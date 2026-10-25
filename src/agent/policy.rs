use async_nats::Client;
use tracing::{info, warn};

/// Fetches the dynamic B2B semantic filter prompt for a given external DID from the NATS KV store.
pub async fn fetch_b2b_prompt(
    nats_client: &Client,
    tenant_id: &str,
    sender_did: &str,
) -> Option<String> {
    info!(
        "🔎 Checking B2B DLP policy for external caller: {} on tenant: {}",
        sender_did, tenant_id
    );

    match async_nats::jetstream::new(nats_client.clone())
        .get_key_value(&format!("KV_{}_B2B_POLICIES", tenant_id.replace(":", "_")))
        .await
    {
        Ok(kv) => {
            let safe_sender_did = sender_did.replace(":", "_");
            match kv.get(&safe_sender_did).await {
                Ok(Some(entry)) => {
                    if let Ok(prompt) = String::from_utf8(entry.to_vec()) {
                        info!("✅ Found B2B semantic filter policy for {}", sender_did);
                        return Some(prompt);
                    }
                }
                Ok(None) => {
                    info!(
                        "ℹ️ No specific B2B policy found for {}; using fallback.",
                        sender_did
                    );
                }
                Err(e) => {
                    warn!(
                        "⚠️ Failed to fetch B2B policy key {} (sanitized: {}): {}",
                        sender_did, safe_sender_did, e
                    );
                }
            }
        }
        Err(e) => {
            warn!("⚠️ B2B Policies bucket not found or inaccessible: {}", e);
        }
    }
    None
}
