use async_trait::async_trait;
use serde::Deserialize;
use std::path::Path;
use tokio::process::Command as TokioCommand;

use crate::function_tool::FunctionCallError;
use crate::tools::context::{FunctionToolOutput, ToolInvocation, ToolPayload};
use crate::tools::registry::{ToolHandler, ToolKind};

pub struct GrepSearchHandler;

#[derive(Deserialize)]
struct GrepArgs {
    query: String,
    path: String,
    glob: Option<String>,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    50
}

impl ToolHandler for GrepSearchHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Self::Output, FunctionCallError> {
        let args: GrepArgs = crate::tools::handlers::parse_arguments(&match invocation.payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "unsupported payload".to_string(),
                ));
            }
        })?;

        let base_dir = crate::tools::handlers::resolve_workdir_base_path("{}", Path::new("."))?;
        let abs_path = base_dir.join(&args.path);

        if !abs_path.is_absolute() {
            return Err(FunctionCallError::RespondToModel(
                "path must be absolute".to_string(),
            ));
        }

        let results = run_rg_search(
            &args.query,
            args.glob.as_deref(),
            &abs_path,
            args.limit,
            &base_dir,
        )
        .await?;

        let (text, hint) = if results.is_empty() {
            (
                "No Matches".to_string(),
                Some("Consider broadening your search query or checking a different directory. You can also use `list_dir` to inspect the contents of the target directory.".to_string()),
            )
        } else {
            (results.join("\n"), None)
        };

        let mut output = FunctionToolOutput::from_text(text, Some(true));
        output.hint = hint;

        Ok(output)
    }
}

pub fn parse_results(stdout: &[u8], limit: usize) -> Vec<String> {
    let output = String::from_utf8_lossy(stdout);
    let mut lines: Vec<String> = output
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.to_string())
        .collect();

    if lines.len() > limit {
        lines.truncate(limit);
    }
    lines
}

pub async fn run_rg_search(
    query: &str,
    glob_filter: Option<&str>,
    target_path: &Path,
    limit: usize,
    _base_dir: &Path,
) -> Result<Vec<String>, FunctionCallError> {
    let mut cmd = TokioCommand::new("rg");
    cmd.arg("-l")
        .arg("--color=never")
        .arg("--hidden")
        .arg("--smart-case");

    if let Some(glob) = glob_filter {
        cmd.arg("-g").arg(glob);
    }

    cmd.arg(query).arg(target_path);

    let output = cmd
        .output()
        .await
        .map_err(|e| FunctionCallError::RespondToModel(format!("failed to run rg: {e}")))?;

    if !output.status.success() && output.stdout.is_empty() {
        // rg returns 1 if no matches found
        return Ok(Vec::new());
    }

    Ok(parse_results(&output.stdout, limit))
}

#[cfg(test)]
#[path = "grep_files_tests.rs"]
mod tests;
