use anyhow::{anyhow, Context, Result};
use codex_protocol::models::{FunctionCallOutputBody, FunctionCallOutputContentItem};
use codex_protocol::protocol::{ContentItem, ResponseItem, RolloutItem};
use codex_state::{StateRuntime, ThreadMetadata};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use chrono::{DateTime, Utc};
use codex_protocol::ThreadId;

pub async fn run_export(
    state_runtime: &StateRuntime,
    session_id: Option<String>,
    last: bool,
    output_path: Option<PathBuf>,
) -> Result<()> {
    let thread = if let Some(id_str) = session_id {
        let id = id_str.parse().context("Failed to parse session ID as UUID")?;
        state_runtime
            .get_thread(id)
            .await?
            .ok_or_else(|| anyhow!("Session not found: {}", id_str))?
    } else if last {
        let threads = state_runtime
            .list_threads(
                1,
                None,
                codex_state::SortKey::UpdatedAt,
                &[],
                None,
                false,
                None,
            )
            .await?;
        threads
            .items
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("No sessions found"))?
    } else {
        return Err(anyhow!("Please specify --session-id or --last"));
    };

    let rollout_path = thread.rollout_path;
    if !rollout_path.exists() {
        return Err(anyhow!("Rollout file not found: {}", rollout_path.display()));
    }

    if let Some(path) = output_path {
        export_to_markdown(&rollout_path, &path, thread.id, thread.title, thread.created_at).await?;
    } else {
        // Output to stdout
        let mut output = std::io::stdout();
        export_to_writer(&rollout_path, &mut output, thread.id, thread.title, thread.created_at)?;
    }

    Ok(())
}

pub async fn export_to_markdown(
    rollout_path: &Path,
    export_path: &Path,
    thread_id: ThreadId,
    title: String,
    created_at: DateTime<Utc>,
) -> Result<()> {
    let mut file = File::create(export_path).context("Failed to create output file")?;
    export_to_writer(rollout_path, &mut file, thread_id, title, created_at)
}

fn export_to_writer<W: Write>(
    rollout_path: &Path,
    output: &mut W,
    thread_id: ThreadId,
    title: String,
    created_at: DateTime<Utc>,
) -> Result<()> {
    let file = File::open(rollout_path).context("Failed to open rollout file")?;
    let reader = BufReader::new(file);

    writeln!(output, "# Codex Session Export")?;
    writeln!(output, "- **Session ID:** `{}`", thread_id)?;
    writeln!(output, "- **Title:** {}", title)?;
    writeln!(output, "- **Date:** {}", created_at)?;
    writeln!(output, "")?;

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let item: RolloutItem = match serde_json::from_str(&line) {
            Ok(item) => item,
            Err(_) => continue,
        };

        if let RolloutItem::ResponseItem(response) = item {
            match response {
                ResponseItem::Message { role, content, .. } => {
                    writeln!(output, "## `{}`", role)?;
                    for content_item in content {
                        match content_item {
                            ContentItem::InputText { text } | ContentItem::OutputText { text } => {
                                writeln!(output, "{}", text)?;
                            }
                            ContentItem::InputImage { image_url } => {
                                writeln!(output, "![Image]({})", image_url)?;
                            }
                        }
                    }
                    writeln!(output, "")?;
                }
                ResponseItem::FunctionCall {
                    name,
                    arguments,
                    namespace,
                    ..
                } => {
                    let full_name = if let Some(ns) = namespace {
                        format!("{}/{}", ns, name)
                    } else {
                        name
                    };
                    writeln!(output, "### 🛠 Tool Call: `{}`", full_name)?;
                    writeln!(output, "```json\n{}\n```", arguments)?;
                    writeln!(output, "")?;
                }
                ResponseItem::FunctionCallOutput {
                    output: payload, ..
                } => {
                    writeln!(output, "### 🔙 Tool Result")?;
                    writeln!(output, "<details><summary>Output</summary>\n")?;
                    match payload.body {
                        FunctionCallOutputBody::Text(text) => {
                            writeln!(output, "{}", text)?;
                        }
                        FunctionCallOutputBody::ContentItems(items) => {
                            for item in items {
                                match item {
                                    FunctionCallOutputContentItem::Text { text } => {
                                        writeln!(output, "{}", text)?;
                                    }
                                    FunctionCallOutputContentItem::Image { image_url, .. } => {
                                        writeln!(output, "![Image]({})", image_url)?;
                                    }
                                }
                            }
                        }
                    }
                    writeln!(output, "\n</details>\n")?;
                }
                _ => {}
            }
        }
    }

    Ok(())
}
