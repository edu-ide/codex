use async_trait::async_trait;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::function_tool::FunctionCallError;
use crate::tools::context::{FunctionToolOutput, ToolInvocation, ToolPayload};
use crate::tools::registry::{ToolHandler, ToolKind};

pub struct ReadFileHandler;

#[derive(Deserialize)]
struct ReadFileArgs {
    path: String,
    #[serde(default = "default_offset")]
    offset: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_offset() -> usize {
    1
}
fn default_limit() -> usize {
    200
}

impl ToolHandler for ReadFileHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        let args: ReadFileArgs =
            crate::tools::handlers::parse_arguments(&match invocation.payload {
                ToolPayload::Function { arguments } => arguments,
                _ => {
                    return Err(FunctionCallError::RespondToModel(
                        "unsupported payload".to_string(),
                    ));
                }
            })?;

        let abs_path = PathBuf::from(&args.path);
        if !abs_path.is_absolute() {
            return Err(FunctionCallError::RespondToModel(
                "path must be absolute".to_string(),
            ));
        }

        let metadata = tokio::fs::metadata(&abs_path)
            .await
            .map_err(|e| FunctionCallError::RespondToModel(format!("failed to stat file: {e}")))?;

        let mtime = metadata
            .modified()
            .map_err(|e| FunctionCallError::RespondToModel(format!("failed to get mtime: {e}")))?;
        let size = metadata.len();

        let cache_key = (args.path.clone(), args.offset, args.limit);
        {
            let mut state = invocation.session.state.lock().await;
            if let Some((cached_mtime, cached_size)) = state.read_file_cache.get(&cache_key) {
                if *cached_mtime == mtime && *cached_size == size {
                    return Ok(FunctionToolOutput::from_text(
                        "File has already been read and has not changed. Check your conversation history to view its contents.".to_string(),
                        Some(true)
                    ));
                }
            }
            state.read_file_cache.insert(cache_key, (mtime, size));
        }

        let lines = match slice::read(&abs_path, args.offset, args.limit).await {
            Ok(l) => l,
            Err(e) => {
                let mut out = FunctionToolOutput::from_text(
                    format!("Failed to read file: {}", e),
                    Some(false),
                );
                if e.to_string().contains("offset exceeds") {
                    out.hint = Some("The requested offset is beyond the file bounds. Try reading from offset 1 or using grep_files.".to_string());
                }
                return Ok(out);
            }
        };

        let mtime_secs = mtime
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);

        let mut output = lines.join("\n");
        let mut hint = None;

        if lines.is_empty() && size > 0 {
            hint = Some("No content found at the requested offset. Try using `grep_files` to find specific content or check the file size.".to_string());
        } else if size == 0 {
            hint = Some("The file is empty.".to_string());
        }

        // Append metadata for concurrency guarding
        output.push_str(&format!(
            "\n\n---\nFile Metadata:\n- Path: {}\n- mtime: {}\n- Size: {} bytes\n",
            args.path, mtime_secs, size
        ));

        let mut tool_output = FunctionToolOutput::from_text(output, Some(true));
        tool_output.hint = hint;

        Ok(tool_output)
    }
}

pub const MAX_LINE_LENGTH: usize = 1000;

#[derive(Clone, Debug)]
pub struct IndentationArgs {
    pub anchor_line: Option<usize>,
    pub include_siblings: bool,
    pub max_levels: usize,
    pub include_header: bool,
}

impl Default for IndentationArgs {
    fn default() -> Self {
        Self {
            anchor_line: None,
            include_siblings: true,
            max_levels: 0,
            include_header: true,
        }
    }
}

pub mod slice {
    use super::*;

    pub async fn read(
        path: &Path,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<String>, FunctionCallError> {
        let file = File::open(path)
            .await
            .map_err(|e| FunctionCallError::RespondToModel(format!("failed to open file: {e}")))?;
        let mut reader = BufReader::new(file);
        let mut lines = Vec::new();
        let mut current_line = 1;

        let mut buf = Vec::new();
        while let Ok(n) = reader.read_until(b'\n', &mut buf).await {
            if n == 0 {
                break;
            }
            if current_line >= offset {
                let s = String::from_utf8_lossy(&buf);
                let trimmed = s.trim_end_matches('\n').trim_end_matches('\r');
                let mut content = trimmed.to_string();
                if content.len() > MAX_LINE_LENGTH {
                    content.truncate(MAX_LINE_LENGTH);
                }
                lines.push(format!("L{}: {}", current_line, content));
                if lines.len() >= limit {
                    break;
                }
            }
            buf.clear();
            current_line += 1;
        }

        if current_line <= offset && current_line > 1 {
            return Err(FunctionCallError::RespondToModel(
                "offset exceeds file length".to_string(),
            ));
        }

        Ok(lines)
    }
}

pub mod indentation {
    use super::*;

    fn indent_of(line: &str) -> Option<usize> {
        if line.trim().is_empty() {
            return None;
        }
        let mut c = 0;
        for ch in line.chars() {
            if ch == ' ' {
                c += 1;
            } else if ch == '\t' {
                c += 4;
            } else {
                break;
            }
        }
        Some(c)
    }

    pub async fn read_block(
        path: &Path,
        offset: usize,
        limit: usize,
        options: IndentationArgs,
    ) -> Result<Vec<String>, super::FunctionCallError> {
        let content = match tokio::fs::read(path).await {
            Ok(c) => c,
            Err(e) => {
                return Err(super::FunctionCallError::RespondToModel(format!(
                    "Failed to read file: {}",
                    e
                )));
            }
        };
        let lossy_content = String::from_utf8_lossy(&content);
        let lines: Vec<String> = lossy_content.lines().map(|s| s.to_string()).collect();
        let target_anchor = options.anchor_line.unwrap_or(offset);
        if lines.is_empty() || target_anchor == 0 || target_anchor > lines.len() {
            return Ok(Vec::new());
        }

        let mut current_anchor = target_anchor; // 1-based
        let mut current_indent = indent_of(&lines[current_anchor - 1]).unwrap_or(0);

        for _ in 0..options.max_levels {
            if current_indent == 0 {
                break;
            }
            for prev_idx in (1..current_anchor).rev() {
                if let Some(ind) = indent_of(&lines[prev_idx - 1]) {
                    if ind < current_indent {
                        current_anchor = prev_idx;
                        current_indent = ind;
                        break;
                    }
                }
            }
        }

        let base_indent = current_indent;
        let mut start_idx = current_anchor;

        if options.include_siblings {
            for prev_idx in (1..current_anchor).rev() {
                if let Some(ind) = indent_of(&lines[prev_idx - 1]) {
                    if ind < base_indent {
                        break;
                    }
                    start_idx = prev_idx;
                }
            }
        }

        if options.include_header {
            while start_idx > 1 {
                let prev_line = &lines[start_idx - 2].trim();
                if prev_line.starts_with("//")
                    || prev_line.starts_with('#')
                    || prev_line.starts_with("/*")
                    || prev_line.starts_with('*')
                {
                    start_idx -= 1;
                } else {
                    break;
                }
            }
        }

        let mut result = Vec::new();
        // iterate downwards from start_idx
        for i in start_idx..=lines.len() {
            let line = &lines[i - 1];

            // if we are past the original anchor's parent block (i > current_anchor)
            if i > current_anchor {
                if let Some(ind) = indent_of(line) {
                    if ind < base_indent {
                        break;
                    } else if ind == base_indent {
                        if !options.include_siblings {
                            let trimmed = line.trim();
                            if trimmed.starts_with('}')
                                || trimmed.starts_with(']')
                                || trimmed.starts_with(')')
                            {
                                result.push(format!("L{}: {}", i, line));
                            }
                            break;
                        }
                    }
                }
            }

            result.push(format!("L{}: {}", i, line));
            if result.len() >= limit {
                break;
            }
        }

        while let Some(last) = result.last() {
            if let Some((_, content)) = last.split_once(": ") {
                if content.trim().is_empty() {
                    result.pop();
                    continue;
                }
            }
            break;
        }

        Ok(result)
    }
}

#[cfg(test)]
#[path = "read_file_tests.rs"]
mod tests;
