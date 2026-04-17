use anyhow::Context;
use codex_hooks::schema::{
    HookEventNameWire, HookUniversalOutputWire, SessionStartCommandInput,
    SessionStartCommandOutputWire, SessionStartHookSpecificOutputWire,
};
use std::io::Read;

pub fn run_get_session_context() -> anyhow::Result<()> {
    // Read input from stdin (Codex hook protocol)
    let mut stdin = std::io::stdin();
    let mut buf = String::new();
    stdin
        .read_to_string(&mut buf)
        .context("Failed to read from stdin")?;

    let input: SessionStartCommandInput =
        serde_json::from_str(&buf).context("Failed to parse SessionStartCommandInput")?;

    // Create a current thread runtime to run async code
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async {
        // Bootstrap ilhae to get access to Brain, Settings, etc.
        // We set mock_mode to false explicitly just in case, but bootstrap handles it.
        let bootstrapped = crate::startup_main::bootstrap_ilhae_runtime().await?;

        let deps = crate::session_context_service::SessionPromptContextDeps {
            brain: bootstrapped.brain.clone(),
            settings_store: bootstrapped.settings_store.clone(),
            ilhae_dir: bootstrapped.ilhae_dir.clone(),
            reverse_session_map: None,
            active_session_id: None,
        };

        // Prepare the session context dynamically just like the old native injection did
        let prepared = crate::session_context_service::prepare_session_prompt_context(
            &deps,
            &input.session_id,
            false,
        )
        .await?;

        // Extract the text blocks
        let mut contexts = vec![];
        for block in prepared.prompt_blocks {
            if let agent_client_protocol_schema::ContentBlock::Text(t) = block {
                contexts.push(t.text);
            }
        }

        // 1. Inject Recent Short-Term Memory (Top 5)
        if let Ok(recent) = deps.brain.memory_chunk_list(0, 5) {
            if !recent.is_empty() {
                let mut buf = String::new();
                buf.push_str("### RECENT MEMORY (Short-Term)\n");
                for mem in recent {
                    if let Some(text) = mem.get("text").and_then(|t| t.as_str()) {
                        let snippet: String = text.chars().take(200).collect();
                        buf.push_str(&format!("- {}...\n", snippet.replace("\n", " ")));
                    }
                }
                contexts.push(buf);
            }
        }

        // 2. Inject Relevant Knowledge Artifacts (LLM Wiki) based on cwd
        if let Ok(wiki) = deps.brain.ki_search(&input.cwd, 5) {
            if let Some(arr) = wiki.as_array() {
                if !arr.is_empty() {
                    let mut buf = String::new();
                    buf.push_str("### RELEVANT KNOWLEDGE (LLM Wiki)\n");
                    buf.push_str("Formal knowledge artifacts related to your context. Use `read_artifact` or `knowledge_search` to read them in full if needed.\n");
                    for ki in arr {
                        if let (Some(id), Some(title), Some(summary)) = (
                            ki.get("id").and_then(|v| v.as_str()),
                            ki.get("title").and_then(|v| v.as_str()),
                            ki.get("summary").and_then(|v| v.as_str()),
                        ) {
                            buf.push_str(&format!("- [{}] {}: {}\n", id, title, summary.replace("\n", " ")));
                        }
                    }
                    contexts.push(buf);
                }
            }
        }

        let combined_context = if contexts.is_empty() {
            None
        } else {
            Some(contexts.join("\n\n"))
        };

        // Construct the output JSON expected by Codex hook
        let out = SessionStartCommandOutputWire {
            universal: HookUniversalOutputWire {
                r#continue: true,
                stop_reason: None,
                suppress_output: false,
                system_message: None,
            },
            hook_specific_output: Some(SessionStartHookSpecificOutputWire {
                hook_event_name: HookEventNameWire::SessionStart,
                additional_context: combined_context,
            }),
        };

        let json_out = serde_json::to_string(&out)?;
        println!("{}", json_out);

        Ok::<_, anyhow::Error>(())
    })?;

    Ok(())
}
