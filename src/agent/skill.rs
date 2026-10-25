//! Standalone Markdown Skill Definition for Commercial B2B Agent

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillFrontmatter {
    pub name: String,
    #[serde(default)]
    pub display_name: Option<String>,
    pub description: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    #[serde(default)]
    pub examples: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub frontmatter: SkillFrontmatter,
    pub instructions: String,
}

impl Skill {
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path_ref = path.as_ref();
        let content = fs::read_to_string(path_ref)
            .with_context(|| format!("Failed to read skill file at {:?}", path_ref))?;
        Self::from_markdown(&content)
            .with_context(|| format!("Failed to parse skill file at {:?}", path_ref))
    }

    pub fn from_markdown(content: &str) -> Result<Self> {
        let trimmed = content.trim();
        if !trimmed.starts_with("---") {
            bail!("Skill file must begin with YAML frontmatter starting with '---'");
        }

        let rest = &trimmed[3..];
        let end_idx = match rest.find("\n---") {
            Some(idx) => idx,
            None => bail!("Malformed skill file: missing closing '---' frontmatter delimiter"),
        };

        let frontmatter_str = rest[..end_idx].trim();
        let frontmatter: SkillFrontmatter = serde_yaml::from_str(frontmatter_str)
            .with_context(|| "Failed to deserialize skill YAML frontmatter")?;

        let instructions_raw = &rest[end_idx + 4..];
        let instructions = instructions_raw.trim().to_string();

        Ok(Self {
            frontmatter,
            instructions,
        })
    }

    pub fn id(&self) -> &str {
        &self.frontmatter.name
    }

    pub fn name(&self) -> &str {
        self.frontmatter
            .display_name
            .as_deref()
            .unwrap_or(&self.frontmatter.name)
    }

    pub fn description(&self) -> &str {
        &self.frontmatter.description
    }

    pub fn tags(&self) -> Vec<String> {
        self.frontmatter.tags.clone().unwrap_or_default()
    }

    pub fn examples(&self) -> Vec<String> {
        self.frontmatter.examples.clone().unwrap_or_default()
    }
}
