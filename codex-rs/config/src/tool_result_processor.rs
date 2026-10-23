use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

/// Optional local command that can replace model-visible tool text.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(try_from = "UncheckedConfig")]
pub struct ToolResultProcessorConfig {
    pub command: Vec<String>,
    #[schemars(default = "default_timeout", range(min = 1, max = 30000))]
    pub timeout_ms: u64,
    #[schemars(default = "default_input_limit", range(min = 1, max = 67108864))]
    pub max_input_bytes: usize,
    #[schemars(default = "default_output_limit", range(min = 1, max = 262144))]
    pub max_output_bytes: usize,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UncheckedConfig {
    command: Vec<String>,
    #[serde(default = "default_timeout")]
    #[schemars(range(min = 1, max = 30000))]
    timeout_ms: u64,
    #[serde(default = "default_input_limit")]
    #[schemars(range(min = 1, max = 67108864))]
    max_input_bytes: usize,
    #[serde(default = "default_output_limit")]
    #[schemars(range(min = 1, max = 262144))]
    max_output_bytes: usize,
}

const fn default_timeout() -> u64 {
    2_000
}
const fn default_input_limit() -> usize {
    32 * 1024 * 1024
}
const fn default_output_limit() -> usize {
    64 * 1024
}

impl ToolResultProcessorConfig {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.command.is_empty()
            || self.command[0].is_empty()
            || self.command.len() > 64
            || self
                .command
                .iter()
                .any(|arg| arg.contains('\0') || arg.len() > 4096)
        {
            return Err("tool result processor requires a bounded, nonempty command argv");
        }
        if !(1..=30_000).contains(&self.timeout_ms)
            || !(1..=64 * 1024 * 1024).contains(&self.max_input_bytes)
            || !(1..=256 * 1024).contains(&self.max_output_bytes)
        {
            return Err("tool result processor limits are outside the supported bounds");
        }
        Ok(())
    }
}

impl TryFrom<UncheckedConfig> for ToolResultProcessorConfig {
    type Error = &'static str;

    fn try_from(value: UncheckedConfig) -> Result<Self, Self::Error> {
        let config = Self {
            command: value.command,
            timeout_ms: value.timeout_ms,
            max_input_bytes: value.max_input_bytes,
            max_output_bytes: value.max_output_bytes,
        };
        config.validate()?;
        Ok(config)
    }
}

#[cfg(test)]
#[path = "tool_result_processor_tests.rs"]
mod tests;
