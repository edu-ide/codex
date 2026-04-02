use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum CommandCategory {
    Model,
    FileOps,
    Search,
    System,
    Session,
    Knowledge,
    Skill,
    Experimental,
    Mcp,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
pub struct CommandMeta {
    pub name: String,
    pub help_text: String,
    pub usage_example: Option<String>,
    pub is_experimental: bool,
    pub is_visible: bool,
    pub available_during_task: bool,
    pub category: CommandCategory,
    // agentskills.io compatible metadata
    pub tags: Option<Vec<String>>,
    pub linked_files: Option<Vec<String>>,
    pub version: Option<String>,
    pub compatibility: Option<String>,
}
