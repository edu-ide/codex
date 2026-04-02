//! TurnAccumulator — unified buffer accumulation, patch generation, and DB
//! persistence logic shared by both Solo (RelayProxy) and Team (leader_loop)
//! response processing paths.
//!
//! Extracts the common `merge_stream_chunk`, `append_*_block`, buffer mutation,
//! `assistant_turn_patch` construction, and `to_db_record()` logic that was
//! previously duplicated between `relay_proxy.rs` and `leader_loop.rs`.

use serde_json::json;

use crate::AssistantContentBlock;

// ─── Stream chunk merging ───────────────────────────────────────────────

/// Merge streamed chunk text into an accumulated buffer.
///
/// Handles both delta chunks and snapshot-style chunks safely:
/// - duplicate chunk: ignore
/// - snapshot chunk (incoming already includes existing): replace
/// - partial overlap: append only non-overlapping tail
pub fn merge_stream_chunk(existing: &mut String, incoming: &str) {
    if incoming.is_empty() {
        return;
    }
    if existing.is_empty() {
        existing.push_str(incoming);
        return;
    }
    if existing.ends_with(incoming) {
        return;
    }
    if incoming.starts_with(existing.as_str()) || incoming.contains(existing.as_str()) {
        existing.clear();
        existing.push_str(incoming);
        return;
    }

    let mut overlap = 0usize;
    for idx in incoming
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(incoming.len()))
    {
        if idx == 0 {
            continue;
        }
        if existing.ends_with(&incoming[..idx]) {
            overlap = idx;
        }
    }

    existing.push_str(&incoming[overlap..]);
}

// ─── ContentBlock helpers ───────────────────────────────────────────────

/// Append or extend a Text content block. Merges with the last block if it's
/// already a Text block; otherwise pushes a new one.
pub fn append_text_block(blocks: &mut Vec<AssistantContentBlock>, incoming: &str) {
    if incoming.is_empty() {
        return;
    }
    // If last block is Text, merge into it (normal streaming case)
    if let Some(AssistantContentBlock::Text { text }) = blocks.last_mut() {
        merge_stream_chunk(text, incoming);
        return;
    }
    // Last block is NOT Text (e.g. ToolCalls or Thinking).
    // If incoming is a snapshot that includes text from previous Text blocks,
    // strip the overlapping prefix to avoid duplicate text rendering.
    let mut new_text = incoming.to_string();
    // Collect all previous text from Text blocks
    let mut prev_text = String::new();
    for block in blocks.iter() {
        if let AssistantContentBlock::Text { text } = block {
            prev_text.push_str(text);
        }
    }
    if !prev_text.is_empty() && new_text.starts_with(&prev_text) {
        // Snapshot includes previous text — strip it
        new_text = new_text[prev_text.len()..].to_string();
        if new_text.is_empty() {
            return; // Nothing new after stripping snapshot overlap
        }
    }
    blocks.push(AssistantContentBlock::Text { text: new_text });
}

/// Append or extend a Thinking content block.
pub fn append_thinking_block(blocks: &mut Vec<AssistantContentBlock>, incoming: &str) {
    if incoming.is_empty() {
        return;
    }
    if let Some(AssistantContentBlock::Thinking { text }) = blocks.last_mut() {
        merge_stream_chunk(text, incoming);
        return;
    }
    blocks.push(AssistantContentBlock::Thinking {
        text: incoming.to_string(),
    });
}

/// Append a tool call ID to the latest ToolCalls block, or create one.
pub fn append_tool_call_block(blocks: &mut Vec<AssistantContentBlock>, tool_call_id: String) {
    if tool_call_id.trim().is_empty() {
        return;
    }
    if let Some(AssistantContentBlock::ToolCalls { tool_call_ids }) = blocks.last_mut() {
        if !tool_call_ids.contains(&tool_call_id) {
            tool_call_ids.push(tool_call_id);
        }
        return;
    }
    blocks.push(AssistantContentBlock::ToolCalls {
        tool_call_ids: vec![tool_call_id],
    });
}

/// Extract `toolCallId` (or `tool_call_id`) from a JSON tool call value.
pub fn extract_tool_call_id(value: &serde_json::Value) -> Option<String> {
    value
        .get("toolCallId")
        .or_else(|| value.get("tool_call_id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

// ─── TurnAccumulator ────────────────────────────────────────────────────

/// Unified turn accumulator used by both Solo and Team response paths.
/// Tracks streamed content, thinking, tool calls, and content blocks in
/// arrival order for a single assistant turn.
#[derive(Clone)]
pub struct TurnAccumulator {
    pub session_id: String,
    pub agent_id: String,
    pub turn_seq: u64,
    pub patch_seq: u64,
    pub content: String,
    pub thinking: String,
    pub tool_calls: Vec<serde_json::Value>,
    pub content_blocks: Vec<AssistantContentBlock>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub duration_ms: i64,
    pub start_time: Option<std::time::Instant>,
    pub db_message_id: Option<i64>,
    pub model_id: String,
    pub cost_usd: f64,
}

impl TurnAccumulator {
    /// Create a new accumulator for a turn.
    pub fn new(session_id: String, agent_id: String, turn_seq: u64) -> Self {
        Self {
            session_id,
            agent_id,
            turn_seq,
            patch_seq: 0,
            content: String::new(),
            thinking: String::new(),
            tool_calls: Vec::new(),
            content_blocks: Vec::new(),
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            duration_ms: 0,
            start_time: Some(std::time::Instant::now()),
            db_message_id: None,
            model_id: String::new(),
            cost_usd: 0.0,
        }
    }

    /// Append text content and update content_blocks.
    pub fn append_text(&mut self, text: &str) {
        merge_stream_chunk(&mut self.content, text);
        append_text_block(&mut self.content_blocks, text);
    }

    /// Append thinking content and update content_blocks.
    pub fn append_thinking(&mut self, text: &str) {
        merge_stream_chunk(&mut self.thinking, text);
        append_thinking_block(&mut self.content_blocks, text);
    }

    /// Push a new tool call (or update existing by ID).
    pub fn push_tool_call(&mut self, val: serde_json::Value, tool_call_id: Option<String>) {
        let mut inserted = false;
        if let Some(ref uid) = tool_call_id {
            if let Some(existing) = self
                .tool_calls
                .iter_mut()
                .find(|tc| tc.get("toolCallId").and_then(|v| v.as_str()) == Some(uid.as_str()))
            {
                *existing = val;
            } else {
                self.tool_calls.push(val);
                inserted = true;
            }
        } else {
            self.tool_calls.push(val);
            inserted = true;
        }
        if inserted {
            if let Some(uid) = tool_call_id {
                append_tool_call_block(&mut self.content_blocks, uid);
            }
        }
    }

    /// Merge an update into an existing tool call entry.
    pub fn merge_tool_call_update(&mut self, val: serde_json::Value, tool_call_id: Option<String>) {
        if let Some(uid) = tool_call_id {
            let mut inserted = false;
            if let Some(existing) = self
                .tool_calls
                .iter_mut()
                .find(|tc| tc.get("toolCallId").and_then(|v| v.as_str()) == Some(&uid))
            {
                if let (Some(existing_obj), Some(update_obj)) =
                    (existing.as_object_mut(), val.as_object())
                {
                    for (k, v) in update_obj {
                        if !v.is_null() {
                            existing_obj.insert(k.clone(), v.clone());
                        }
                    }
                }
            } else {
                self.tool_calls.push(val);
                inserted = true;
            }
            if inserted {
                append_tool_call_block(&mut self.content_blocks, uid);
            }
        }
    }

    /// Update token stats.
    #[allow(dead_code)]
    pub fn update_tokens(&mut self, input: i64, output: i64, total: i64) {
        self.input_tokens = input;
        self.output_tokens = output;
        self.total_tokens = total;
    }

    /// Bump patch sequence and update duration.
    pub fn advance_patch(&mut self) {
        self.patch_seq = self.patch_seq.saturating_add(1);
        if let Some(start) = self.start_time {
            self.duration_ms = start.elapsed().as_millis() as i64;
        }
    }

    /// Build the `ilhae/assistant_turn_patch` JSON payload.
    pub fn to_patch(&self) -> serde_json::Value {
        let mut patch = json!({
            "sessionId": self.session_id,
            "agentId": self.agent_id,
            "turnSeq": self.turn_seq,
            "patchSeq": self.patch_seq,
            "content": self.content,
            "thinking": self.thinking,
            "toolCalls": self.tool_calls,
            "contentBlocks": self.content_blocks,
            "input_tokens": self.input_tokens,
            "output_tokens": self.output_tokens,
            "total_tokens": self.total_tokens,
            "durationMs": self.duration_ms,
        });
        if !self.model_id.is_empty() {
            patch["modelId"] = json!(self.model_id);
        }
        patch
    }

    /// Return serialized fields for DB persistence.
    pub fn to_db_fields(&self) -> DbMessageFields {
        let tool_calls_json = if self.tool_calls.is_empty() {
            String::new()
        } else {
            serde_json::to_string(&self.tool_calls).unwrap_or_default()
        };
        let content_blocks_json = if self.content_blocks.is_empty() {
            String::new()
        } else {
            serde_json::to_string(&self.content_blocks).unwrap_or_default()
        };
        DbMessageFields {
            content: self.content.clone(),
            thinking: self.thinking.clone(),
            tool_calls: tool_calls_json,
            content_blocks: content_blocks_json,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            total_tokens: self.total_tokens,
            duration_ms: self.duration_ms,
        }
    }

    /// Whether this accumulator has any renderable content.
    pub fn has_content(&self) -> bool {
        !self.content.is_empty() || !self.tool_calls.is_empty()
    }

    /// Reset for a new turn — clears content/thinking/tool_calls/content_blocks,
    /// bumps turn_seq, resets patch_seq.
    pub fn reset_for_new_turn(&mut self) {
        self.turn_seq = self.turn_seq.saturating_add(1);
        self.patch_seq = 0;
        self.content.clear();
        self.thinking.clear();
        self.tool_calls.clear();
        self.content_blocks.clear();
        self.input_tokens = 0;
        self.output_tokens = 0;
        self.total_tokens = 0;
        self.duration_ms = 0;
        self.start_time = Some(std::time::Instant::now());
    }
}

/// Serialized DB message fields ready for `add_full_message_with_blocks`.
pub struct DbMessageFields {
    pub content: String,
    pub thinking: String,
    pub tool_calls: String,
    pub content_blocks: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub duration_ms: i64,
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_stream_chunk_basic() {
        let mut buf = String::new();
        merge_stream_chunk(&mut buf, "hello");
        assert_eq!(buf, "hello");
        merge_stream_chunk(&mut buf, " world");
        assert_eq!(buf, "hello world");
    }

    #[test]
    fn merge_stream_chunk_duplicate() {
        let mut buf = "hello world".to_string();
        merge_stream_chunk(&mut buf, " world");
        assert_eq!(buf, "hello world"); // no duplicate
    }

    #[test]
    fn merge_stream_chunk_snapshot() {
        let mut buf = "hello".to_string();
        merge_stream_chunk(&mut buf, "hello world");
        assert_eq!(buf, "hello world"); // snapshot replace
    }

    #[test]
    fn accumulator_content_blocks_order() {
        let mut acc = TurnAccumulator::new("s1".into(), "agent".into(), 1);
        acc.append_thinking("think first");
        acc.append_text("then text");
        acc.push_tool_call(json!({"toolCallId": "tc1"}), Some("tc1".into()));
        acc.append_text("more text");

        assert_eq!(acc.content_blocks.len(), 4);
        matches!(
            &acc.content_blocks[0],
            AssistantContentBlock::Thinking { .. }
        );
        matches!(&acc.content_blocks[1], AssistantContentBlock::Text { .. });
        matches!(
            &acc.content_blocks[2],
            AssistantContentBlock::ToolCalls { .. }
        );
        matches!(&acc.content_blocks[3], AssistantContentBlock::Text { .. });
    }

    #[test]
    fn accumulator_patch_format() {
        let mut acc = TurnAccumulator::new("s1".into(), "leader".into(), 1);
        acc.append_text("hello");
        acc.advance_patch();
        let patch = acc.to_patch();
        assert_eq!(patch["sessionId"], "s1");
        assert_eq!(patch["agentId"], "leader");
        assert_eq!(patch["content"], "hello");
        assert_eq!(patch["patchSeq"], 1);
    }

    #[test]
    fn accumulator_db_fields() {
        let mut acc = TurnAccumulator::new("s1".into(), "agent".into(), 1);
        acc.append_text("content");
        acc.append_thinking("thought");
        acc.push_tool_call(
            json!({"toolCallId": "tc1", "name": "test"}),
            Some("tc1".into()),
        );

        let fields = acc.to_db_fields();
        assert_eq!(fields.content, "content");
        assert_eq!(fields.thinking, "thought");
        assert!(!fields.tool_calls.is_empty());
        assert!(!fields.content_blocks.is_empty());
    }

    #[test]
    fn tool_call_dedup() {
        let mut acc = TurnAccumulator::new("s1".into(), "agent".into(), 1);
        acc.push_tool_call(
            json!({"toolCallId": "tc1", "status": "running"}),
            Some("tc1".into()),
        );
        acc.push_tool_call(
            json!({"toolCallId": "tc1", "status": "done"}),
            Some("tc1".into()),
        );
        assert_eq!(acc.tool_calls.len(), 1);
        assert_eq!(acc.tool_calls[0]["status"], "done");
    }

    #[test]
    fn merge_tool_call_update_partial() {
        let mut acc = TurnAccumulator::new("s1".into(), "agent".into(), 1);
        acc.push_tool_call(
            json!({"toolCallId": "tc1", "status": "running", "name": "test"}),
            Some("tc1".into()),
        );
        acc.merge_tool_call_update(
            json!({"toolCallId": "tc1", "status": "done"}),
            Some("tc1".into()),
        );
        assert_eq!(acc.tool_calls.len(), 1);
        assert_eq!(acc.tool_calls[0]["status"], "done");
        assert_eq!(acc.tool_calls[0]["name"], "test"); // preserved
    }

    // Tests moved from relay_proxy.rs
    #[test]
    fn append_text_block_merges() {
        let mut blocks = Vec::new();
        append_text_block(&mut blocks, "hello");
        append_text_block(&mut blocks, " world");
        assert_eq!(blocks.len(), 1);
        if let AssistantContentBlock::Text { text } = &blocks[0] {
            assert_eq!(text, "hello world");
        }
    }

    #[test]
    fn append_thinking_then_text_separate() {
        let mut blocks = Vec::new();
        append_thinking_block(&mut blocks, "thinking...");
        append_text_block(&mut blocks, "response");
        assert_eq!(blocks.len(), 2);
    }

    #[test]
    fn append_tool_call_block_dedup() {
        let mut blocks = Vec::new();
        append_tool_call_block(&mut blocks, "tool-a".into());
        append_tool_call_block(&mut blocks, "tool-a".into());
        append_tool_call_block(&mut blocks, "tool-b".into());
        assert_eq!(blocks.len(), 1);
        if let AssistantContentBlock::ToolCalls { tool_call_ids } = &blocks[0] {
            assert_eq!(tool_call_ids, &["tool-a", "tool-b"]);
        }
    }
}
