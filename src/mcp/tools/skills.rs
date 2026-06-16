use crate::mcp::{AppContext, McpTool, McpToolResult};
use anyhow::Result;
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::sync::Arc;

pub fn create_skill_tool() -> McpTool {
    McpTool {
        name: "create_skill".to_string(),
        description:
            "Create a new skill (SKILL.md file) for reusable procedures. Skills allow the agent to automate recurring task patterns. The skill is saved to <data_dir>/skills/<category>/SKILL.md and will be available for future sessions."
                .to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Skill name (lowercase, hyphens/underscores, max 64 chars)"
                },
                "description": {
                    "type": "string",
                    "description": "Brief description of what the skill does"
                },
                "content": {
                    "type": "string",
                    "description": "Full markdown body of the skill (steps, verification, etc.)"
                },
                "category": {
                    "type": "string",
                    "description": "Optional category for organizing (e.g., 'devops', 'data-science'). Default: 'general'"
                }
            },
            "required": ["name", "description", "content"]
        }),
        handler: Arc::new(|args: Value, ctx: AppContext| -> Result<McpToolResult> {
            // Extract arguments
            let name = args["name"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing required argument: 'name'"))?;
            let description = args["description"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing required argument: 'description'"))?;
            let content = args["content"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing required argument: 'content'"))?;
            let category = args["category"].as_str().unwrap_or("general");

            // Validate name: non-empty
            if name.is_empty() {
                anyhow::bail!("Skill name must not be empty");
            }
            // Validate name: max 64 chars
            if name.len() > 64 {
                anyhow::bail!(
                    "Skill name must be 64 characters or less (got {})",
                    name.len()
                );
            }
            // Validate name: lowercase alphanumeric, hyphens, underscores
            if !name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
            {
                anyhow::bail!(
                    "Skill name must match pattern: lowercase alphanumeric, hyphens, underscores only"
                );
            }

            // Validate description: non-empty
            if description.is_empty() {
                anyhow::bail!("Skill description must not be empty");
            }

            // Validate content: non-empty
            if content.is_empty() {
                anyhow::bail!("Skill content must not be empty");
            }

            // Normalize name (lowercase, spaces -> hyphens)
            let normalized_name = name.to_lowercase().replace(' ', "-");

            // Build file path: <data_dir>/skills/<category>/SKILL.md
            let skill_dir = Path::new(&ctx.data_dir).join("skills").join(category);
            let skill_path = skill_dir.join("SKILL.md");

            // Check if SKILL.md already exists (don't overwrite)
            if skill_path.exists() {
                anyhow::bail!(
                    "Skill '{}' already exists at {}. Use a different name or category.",
                    normalized_name,
                    skill_path.display()
                );
            }

            // Create the directory if it doesn't exist
            fs::create_dir_all(&skill_dir).map_err(|e| {
                anyhow::anyhow!(
                    "Failed to create skill directory '{}': {}",
                    skill_dir.display(),
                    e
                )
            })?;

            // Build the full file content with YAML frontmatter + body
            let file_content = format!(
                "---\nname: {}\ndescription: {}\ncategory: {}\n---\n\n{}",
                normalized_name, description, category, content
            );

            // Write the file
            let safe_path_str = skill_path.to_string_lossy().to_string();
            fs::write(&skill_path, &file_content).map_err(|e| {
                anyhow::anyhow!("Failed to write skill file '{}': {}", safe_path_str, e)
            })?;

            Ok(McpToolResult {
                call_id: String::new(),
                content: format!(
                    "Skill '{}' created successfully at {}",
                    normalized_name, safe_path_str
                ),
                is_error: false,
            })
        }),
    }
}
