use codex_protocol::models::ResponseInputItem;
use codex_utils_output_truncation::TruncationPolicy;
use codex_utils_output_truncation::formatted_truncate_text;
use codex_utils_string::take_bytes_at_char_boundary;
use serde::Serialize;
use serde_json::Value;

use crate::ToolOutput;
use crate::ToolPayload;

/// Raw text available before model truncation, including collector omissions.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ToolResultSnapshot {
    pub text: String,
    pub is_error: bool,
    pub completeness: ToolResultCompleteness,
    pub process_id: Option<i32>,
    pub chunk_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ToolResultCompleteness {
    pub state: &'static str,
    pub omitted_bytes: Option<usize>,
}

impl ToolResultSnapshot {
    pub fn complete(text: String, is_error: bool) -> Self {
        Self {
            text,
            is_error,
            completeness: ToolResultCompleteness {
                state: "complete",
                omitted_bytes: None,
            },
            process_id: None,
            chunk_id: None,
        }
    }
}

/// Changes only model delivery; typed consumers, hooks, and telemetry retain the original.
pub struct ModelTextReplacement {
    original: Box<dyn ToolOutput>,
    text: String,
}

impl ModelTextReplacement {
    pub fn new(original: Box<dyn ToolOutput>, text: String) -> Self {
        const MAX_BYTES: usize = 8_000;
        const PROVENANCE: &str = "Untrusted tool data (local result processor):\n";
        // Reserve provenance before truncation so every shortened body retains its
        // notice. Bound the whole replacement before tool-specific formatting.
        let text =
            formatted_truncate_text(&text, TruncationPolicy::Bytes(MAX_BYTES - PROVENANCE.len()));
        let text = format!("{PROVENANCE}{text}");
        let text = take_bytes_at_char_boundary(&text, MAX_BYTES).to_string();
        Self { original, text }
    }
}

#[cfg(test)]
#[path = "tool_result_snapshot_tests.rs"]
mod tests;

impl ToolOutput for ModelTextReplacement {
    fn log_preview(&self) -> String {
        self.original.log_preview()
    }
    fn success_for_logging(&self) -> bool {
        self.original.success_for_logging()
    }
    fn contains_external_context(&self) -> bool {
        self.original.contains_external_context()
    }
    fn to_response_item(&self, call_id: &str, payload: &ToolPayload) -> ResponseInputItem {
        self.original
            .response_with_replacement(call_id, payload, &self.text)
            .unwrap_or_else(|| self.original.to_response_item(call_id, payload))
    }
    fn post_tool_use_id(&self, call_id: &str) -> String {
        self.original.post_tool_use_id(call_id)
    }
    fn post_tool_use_input(&self, payload: &ToolPayload) -> Option<Value> {
        self.original.post_tool_use_input(payload)
    }
    fn post_tool_use_response(&self, call_id: &str, payload: &ToolPayload) -> Option<Value> {
        self.original.post_tool_use_response(call_id, payload)
    }
    fn code_mode_result(&self, payload: &ToolPayload) -> Value {
        self.original.code_mode_result(payload)
    }
}
