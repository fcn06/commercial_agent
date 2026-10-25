use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Parameter mapping rule: translates an incoming parameter key to a canonical parameter key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ParameterMapping {
    pub source_param: String,
    pub target_param: String,
    #[serde(default)]
    pub default_value: Option<Value>,
}

/// Tool mapping configuration defining translation rules for a specific external tool call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolMapping {
    pub incoming_tool: String,
    pub canonical_tool: String,
    #[serde(default)]
    pub parameter_mappings: Vec<ParameterMapping>,
    #[serde(default)]
    pub passthrough_unmapped: bool,
}

/// Root configuration for schema translation mappings.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SchemaMappingConfig {
    #[serde(default)]
    pub mappings: Vec<ToolMapping>,
}

/// Semantic IO Mapper middleware: translates heterogeneous external tool schemas
/// (e.g. from Claude MCP, A2A agents, or partner swarms) into mesh canonical tool requests.
#[derive(Clone, Default)]
pub struct IoMapper {
    deterministic_lookup: HashMap<String, ToolMapping>,
}

impl IoMapper {
    pub fn new(config: SchemaMappingConfig) -> Self {
        let mut lookup = HashMap::new();
        for m in config.mappings {
            lookup.insert(m.incoming_tool.clone(), m);
        }
        Self {
            deterministic_lookup: lookup,
        }
    }

    /// Load schema mappings from a TOML file.
    pub fn load_from_file(path: &str) -> Result<Self, anyhow::Error> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read schema mappings at {path}: {e}"))?;
        let config: SchemaMappingConfig = toml::from_str(&content)
            .map_err(|e| anyhow::anyhow!("Failed to parse schema mappings TOML at {path}: {e}"))?;
        Ok(Self::new(config))
    }

    /// Translates an incoming tool name and argument JSON object into the mesh's canonical representation.
    ///
    /// If no mapping is defined, returns the original tool name and arguments unchanged.
    pub fn map_inbound(
        &self,
        incoming_tool: &str,
        incoming_args: &Value,
    ) -> Result<(String, Value), anyhow::Error> {
        if let Some(mapping) = self.deterministic_lookup.get(incoming_tool) {
            let src_obj = match incoming_args.as_object() {
                Some(obj) => obj,
                None => {
                    // Non-object arguments are passed through if no parameter rules exist
                    if mapping.parameter_mappings.is_empty() {
                        return Ok((mapping.canonical_tool.clone(), incoming_args.clone()));
                    }
                    return Err(anyhow::anyhow!(
                        "Incoming arguments for tool '{incoming_tool}' must be a JSON object"
                    ));
                }
            };

            let mut mapped_obj = serde_json::Map::new();

            // Apply explicit parameter translations
            for p in &mapping.parameter_mappings {
                if let Some(val) = src_obj.get(&p.source_param) {
                    mapped_obj.insert(p.target_param.clone(), val.clone());
                } else if let Some(ref def) = p.default_value {
                    mapped_obj.insert(p.target_param.clone(), def.clone());
                }
            }

            // If passthrough_unmapped is enabled, retain unmapped keys
            if mapping.passthrough_unmapped {
                for (k, v) in src_obj {
                    let is_source = mapping
                        .parameter_mappings
                        .iter()
                        .any(|p| &p.source_param == k);
                    if !is_source && !mapped_obj.contains_key(k) {
                        mapped_obj.insert(k.clone(), v.clone());
                    }
                }
            }

            return Ok((mapping.canonical_tool.clone(), Value::Object(mapped_obj)));
        }

        // Pass-through if already in canonical format or unmapped
        Ok((incoming_tool.to_string(), incoming_args.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_io_mapper_deterministic_translation() {
        let config = SchemaMappingConfig {
            mappings: vec![ToolMapping {
                incoming_tool: "google.calendar.create_event".to_string(),
                canonical_tool: "calendar_create_event".to_string(),
                parameter_mappings: vec![
                    ParameterMapping {
                        source_param: "event_title".to_string(),
                        target_param: "title".to_string(),
                        default_value: None,
                    },
                    ParameterMapping {
                        source_param: "start_time".to_string(),
                        target_param: "start_iso".to_string(),
                        default_value: None,
                    },
                    ParameterMapping {
                        source_param: "end_time".to_string(),
                        target_param: "end_iso".to_string(),
                        default_value: None,
                    },
                ],
                passthrough_unmapped: false,
            }],
        };

        let mapper = IoMapper::new(config);

        let incoming = json!({
            "event_title": "Quarterly Review",
            "start_time": "2026-10-15T09:00:00Z",
            "end_time": "2026-10-15T10:00:00Z",
            "internal_tracker_id": "skip_me"
        });

        let (canonical_name, canonical_args) = mapper
            .map_inbound("google.calendar.create_event", &incoming)
            .expect("Mapping failed");

        assert_eq!(canonical_name, "calendar_create_event");
        assert_eq!(canonical_args["title"], "Quarterly Review");
        assert_eq!(canonical_args["start_iso"], "2026-10-15T09:00:00Z");
        assert_eq!(canonical_args["end_iso"], "2026-10-15T10:00:00Z");
        assert!(canonical_args.get("internal_tracker_id").is_none());
    }

    #[test]
    fn test_io_mapper_default_value_injection() {
        let config = SchemaMappingConfig {
            mappings: vec![ToolMapping {
                incoming_tool: "mcp_stripe/charge".to_string(),
                canonical_tool: "stripe_charge_customer".to_string(),
                parameter_mappings: vec![
                    ParameterMapping {
                        source_param: "amount".to_string(),
                        target_param: "amount_cents".to_string(),
                        default_value: None,
                    },
                    ParameterMapping {
                        source_param: "currency".to_string(),
                        target_param: "currency".to_string(),
                        default_value: Some(json!("USD")),
                    },
                ],
                passthrough_unmapped: true,
            }],
        };

        let mapper = IoMapper::new(config);

        let incoming = json!({
            "amount": 2500,
            "description": "Monthly API subscription"
        });

        let (name, args) = mapper
            .map_inbound("mcp_stripe/charge", &incoming)
            .expect("Mapping failed");

        assert_eq!(name, "stripe_charge_customer");
        assert_eq!(args["amount_cents"], 2500);
        assert_eq!(args["currency"], "USD"); // Default value injected
        assert_eq!(args["description"], "Monthly API subscription"); // Unmapped passed through
    }
}
