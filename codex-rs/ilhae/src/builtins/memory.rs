#[macro_export]
macro_rules! register_memory_tools {
    ($builder:expr, $brain_service:expr, $bt_settings:expr) => {{
        use $crate::{
            MemoryReadInput, MemoryWriteInput, MemoryToolDreamAnalyzeInput,
            MemoryToolDreamApplyInput, MemoryToolDreamPreviewInput, MemoryToolExtractInput,
            MemoryToolForgetInput, MemoryToolListInput, MemoryToolPinInput,
            MemoryToolPromoteInput, MemoryToolSearchInput, MemoryToolStatsInput,
            MemoryToolStoreInput,
        };

        $builder
            .tool_fn(
                "memory_read",
                "Read agent context. Sections: system, identity, soul, user (from files), memory (from pinned DB items), daily, project, or all.",
                {
                    let bts = $bt_settings.clone();
                    let brain = $brain_service.clone();
                    async move |input: MemoryReadInput, _cx| {
                        $crate::check_tool_enabled!(bts, "memory_read");
                        let text = if input.section == "memory" {
                            brain.memory_pinned_text()
                        } else {
                            $crate::memory_provider::read_section(&input.section)
                                .unwrap_or_else(|e| e)
                        };
                        Ok::<String, sacp::Error>(if text.is_empty() { "(empty)".to_string() } else { text })
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "memory_write",
                "Update agent context. For identity/soul/user/system: writes to file. For memory: stores as pinned in DB.",
                {
                    let bts = $bt_settings.clone();
                    let brain = $brain_service.clone();
                    async move |input: MemoryWriteInput, _cx| {
                        $crate::check_tool_enabled!(bts, "memory_write");
                        if input.section == "memory" {
                            match brain.memory_chunk_store_pinned(&input.content, "agent", true) {
                                Ok(id) => Ok::<String, sacp::Error>(format!("✅ Stored as pinned memory (id={})", id)),
                                Err(e) => Err(sacp::Error::internal_error().data(e.to_string())),
                            }
                        } else {
                            $crate::memory_provider::write_section(&input.section, &input.content)
                                .map_err(|e| sacp::Error::invalid_request().data(e))
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "memory_search",
                "Search long-term memory using BM25 full-text search. Returns ranked memory chunks matching the query.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    async move |input: MemoryToolSearchInput, _cx| {
                        $crate::check_tool_enabled!(bts, "memory_search");
                        match brain.memory_search(&input.query, input.limit, None, None) {
                            Ok(chunks) => {
                                let text = serde_json::to_string_pretty(&chunks)
                                    .unwrap_or("[]".to_string());
                                Ok::<String, sacp::Error>(text)
                            }
                            Err(e) => Err(sacp::Error::internal_error().data(e.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "memory_store",
                "Store a new piece of knowledge in long-term memory. Use this to remember important facts, decisions, or user preferences.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    async move |input: MemoryToolStoreInput, _cx| {
                        $crate::check_tool_enabled!(bts, "memory_store");
                        match brain.memory_chunk_store(&input.text, &input.source) {
                            Ok(id) => Ok::<String, sacp::Error>(format!("✅ Stored memory chunk (id={})", id)),
                            Err(e) => Err(sacp::Error::internal_error().data(e.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "memory_forget",
                "Delete a memory chunk by ID. Use this to remove outdated or incorrect knowledge.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    async move |input: MemoryToolForgetInput, _cx| {
                        $crate::check_tool_enabled!(bts, "memory_forget");
                        match brain.memory_chunk_forget(input.chunk_id) {
                            Ok(true) => Ok::<String, sacp::Error>(format!("✅ Deleted memory chunk {}", input.chunk_id)),
                            Ok(false) => Ok::<String, sacp::Error>(format!("⚠️ Chunk {} not found", input.chunk_id)),
                            Err(e) => Err(sacp::Error::internal_error().data(e.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "memory_list",
                "List memory chunks with pagination. Returns stored memories ordered by recency.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    async move |input: MemoryToolListInput, _cx| {
                        $crate::check_tool_enabled!(bts, "memory_list");
                        match brain.memory_chunk_list(input.offset, input.limit) {
                            Ok(chunks) => {
                                let text = serde_json::to_string_pretty(&chunks)
                                    .unwrap_or("[]".to_string());
                                Ok::<String, sacp::Error>(text)
                            }
                            Err(e) => Err(sacp::Error::internal_error().data(e.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "memory_stats",
                "Get memory store statistics: total chunks, indexed files, DB size, last index time.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    async move |_input: MemoryToolStatsInput, _cx| {
                        $crate::check_tool_enabled!(bts, "memory_stats");
                        match brain.memory_chunk_stats() {
                            Ok(stats) => {
                                let text = serde_json::to_string_pretty(&stats)
                                    .unwrap_or("{}".to_string());
                                Ok::<String, sacp::Error>(text)
                            }
                            Err(e) => Err(sacp::Error::internal_error().data(e.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "memory_pin",
                "Pin or unpin a memory chunk. Pinned memories are always injected into the agent's context.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    async move |input: MemoryToolPinInput, _cx| {
                        $crate::check_tool_enabled!(bts, "memory_pin");
                        match brain.memory_chunk_set_pinned(input.chunk_id, input.pinned) {
                            Ok(true) => Ok::<String, sacp::Error>(format!("✅ Chunk {} {}", input.chunk_id, if input.pinned { "pinned" } else { "unpinned" })),
                            Ok(false) => Ok::<String, sacp::Error>(format!("⚠️ Chunk {} not found", input.chunk_id)),
                            Err(e) => Err(sacp::Error::internal_error().data(e.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "memory_promote",
                "Promote a session artifact into a durable knowledge item. Requires self-improvement mode.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    async move |input: MemoryToolPromoteInput, _cx| {
                        $crate::check_tool_enabled!(bts, "memory_promote");
                        if !bts.get().agent.self_improvement_enabled {
                            return Err(sacp::Error::invalid_request().data(
                                "Self-improvement is disabled. Enable it in the active ilhae profile.".to_string(),
                            ));
                        }
                        match brain.memory_promote(
                            &input.session_id,
                            &input.artifact_type,
                            &input.ki_id,
                            &input.title,
                            input.tags,
                        ) {
                            Ok(result) => Ok::<String, sacp::Error>(
                                serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string())
                            ),
                            Err(e) => Err(sacp::Error::internal_error().data(e.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "memory_extract",
                "Extract a knowledge item into a graph/vault note. Requires self-improvement mode.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    async move |input: MemoryToolExtractInput, _cx| {
                        $crate::check_tool_enabled!(bts, "memory_extract");
                        if !bts.get().agent.self_improvement_enabled {
                            return Err(sacp::Error::invalid_request().data(
                                "Self-improvement is disabled. Enable it in the active ilhae profile.".to_string(),
                            ));
                        }
                        match brain.memory_extract(
                            &input.ki_id,
                            input.namespace.as_deref(),
                            input.vault_path.as_deref(),
                        ) {
                            Ok(result) => Ok::<String, sacp::Error>(
                                serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string())
                            ),
                            Err(e) => Err(sacp::Error::internal_error().data(e.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "memory_dream_preview",
                "Preview pending dream groups that are ready for summarization. Requires self-improvement mode.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    async move |input: MemoryToolDreamPreviewInput, _cx| {
                        $crate::check_tool_enabled!(bts, "memory_dream_preview");
                        if !bts.get().agent.self_improvement_enabled {
                            return Err(sacp::Error::invalid_request().data(
                                "Self-improvement is disabled. Enable it in the active ilhae profile.".to_string(),
                            ));
                        }
                        match brain.memory_dream_preview(input.limit) {
                            Ok(result) => Ok::<String, sacp::Error>(
                                serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string())
                            ),
                            Err(e) => Err(sacp::Error::internal_error().data(e.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "memory_dream_analyze",
                "Analyze pending dream groups within a directory scope. Requires self-improvement mode.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    async move |input: MemoryToolDreamAnalyzeInput, _cx| {
                        $crate::check_tool_enabled!(bts, "memory_dream_analyze");
                        if !bts.get().agent.self_improvement_enabled {
                            return Err(sacp::Error::invalid_request().data(
                                "Self-improvement is disabled. Enable it in the active ilhae profile.".to_string(),
                            ));
                        }
                        match brain.memory_dream_analyze(std::path::Path::new(&input.dir), input.limit) {
                            Ok(result) => Ok::<String, sacp::Error>(
                                serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string())
                            ),
                            Err(e) => Err(sacp::Error::internal_error().data(e.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "memory_dream_ignored_preview",
                "Preview ignored dream groups that may need re-review. Requires self-improvement mode.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    async move |input: MemoryToolDreamPreviewInput, _cx| {
                        $crate::check_tool_enabled!(bts, "memory_dream_ignored_preview");
                        if !bts.get().agent.self_improvement_enabled {
                            return Err(sacp::Error::invalid_request().data(
                                "Self-improvement is disabled. Enable it in the active ilhae profile.".to_string(),
                            ));
                        }
                        match brain.memory_dream_ignored_preview(input.limit) {
                            Ok(result) => Ok::<String, sacp::Error>(
                                serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string())
                            ),
                            Err(e) => Err(sacp::Error::internal_error().data(e.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "memory_dream_summarize",
                "Mark dream chunk groups as summarized after consolidation. Requires self-improvement mode.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    async move |input: MemoryToolDreamApplyInput, _cx| {
                        $crate::check_tool_enabled!(bts, "memory_dream_summarize");
                        if !bts.get().agent.self_improvement_enabled {
                            return Err(sacp::Error::invalid_request().data(
                                "Self-improvement is disabled. Enable it in the active ilhae profile.".to_string(),
                            ));
                        }
                        match brain.memory_dream_summarize(&input.ids) {
                            Ok(result) => Ok::<String, sacp::Error>(
                                serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string())
                            ),
                            Err(e) => Err(sacp::Error::internal_error().data(e.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "memory_dream_ignore",
                "Mark dream chunk groups as ignored. Requires self-improvement mode.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    async move |input: MemoryToolDreamApplyInput, _cx| {
                        $crate::check_tool_enabled!(bts, "memory_dream_ignore");
                        if !bts.get().agent.self_improvement_enabled {
                            return Err(sacp::Error::invalid_request().data(
                                "Self-improvement is disabled. Enable it in the active ilhae profile.".to_string(),
                            ));
                        }
                        match brain.memory_dream_ignore(&input.ids) {
                            Ok(result) => Ok::<String, sacp::Error>(
                                serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string())
                            ),
                            Err(e) => Err(sacp::Error::internal_error().data(e.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
            .tool_fn(
                "memory_dream_requeue",
                "Move ignored dream chunk groups back to pending review. Requires self-improvement mode.",
                {
                    let brain = $brain_service.clone();
                    let bts = $bt_settings.clone();
                    async move |input: MemoryToolDreamApplyInput, _cx| {
                        $crate::check_tool_enabled!(bts, "memory_dream_requeue");
                        if !bts.get().agent.self_improvement_enabled {
                            return Err(sacp::Error::invalid_request().data(
                                "Self-improvement is disabled. Enable it in the active ilhae profile.".to_string(),
                            ));
                        }
                        match brain.memory_dream_requeue(&input.ids) {
                            Ok(result) => Ok::<String, sacp::Error>(
                                serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string())
                            ),
                            Err(e) => Err(sacp::Error::internal_error().data(e.to_string())),
                        }
                    }
                },
                sacp::tool_fn!(),
            )
    }};
}
