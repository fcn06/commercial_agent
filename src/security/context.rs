//! Secret & Credential Context Management for Commercial B2B Agent

pub struct SecretString(String);

impl SecretString {
    pub fn new(s: String) -> Self {
        Self(s)
    }

    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

pub fn load_secret(var_name: &str) -> Option<SecretString> {
    if let Ok(val) = std::env::var(var_name) {
        let trimmed = val.trim();
        if !trimmed.is_empty() {
            return Some(SecretString(trimmed.to_string()));
        }
    }

    let _ = dotenvy::dotenv();
    if let Ok(val) = std::env::var(var_name) {
        let trimmed = val.trim();
        if !trimmed.is_empty() {
            return Some(SecretString(trimmed.to_string()));
        }
    }

    None
}
