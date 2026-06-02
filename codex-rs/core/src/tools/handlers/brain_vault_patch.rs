use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::brain_vault_patch_spec::BRAIN_VAULT_PATCH_TOOL_NAME;
use crate::tools::handlers::brain_vault_patch_spec::create_brain_vault_patch_tool;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use codex_protocol::items::FileChangeItem;
use codex_protocol::items::TurnItem;
use codex_protocol::protocol::FileChange;
use codex_protocol::protocol::PatchApplyStatus;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use serde::Deserialize;
use serde_json::Value;
use sha1::Digest;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::OnceLock;

const DEFAULT_ACTION: &str = "record";
static BRAIN_VAULT_WRITE_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

pub struct BrainVaultPatchHandler;

#[derive(Debug, Deserialize)]
struct BrainVaultPatchArgs {
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    vault: Option<String>,
    #[serde(default)]
    location: Option<String>,
    #[serde(default, alias = "loopPhase")]
    loop_phase: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default, alias = "baseHash")]
    base_hash: Option<String>,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

struct VaultRecordTarget {
    path: PathBuf,
    display_path: PathBuf,
    resource_uri: String,
}

struct PreparedRecordWrite {
    base_hash: String,
    new_hash: String,
    next_content: String,
    changes: HashMap<PathBuf, FileChange>,
}

#[async_trait::async_trait]
impl ToolExecutor<ToolInvocation> for BrainVaultPatchHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(BRAIN_VAULT_PATCH_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_brain_vault_patch_tool()
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let ToolInvocation {
            session,
            payload,
            turn,
            call_id,
            ..
        } = invocation;
        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "brain_vault_patch handler received unsupported payload".to_string(),
                ));
            }
        };

        let args: BrainVaultPatchArgs = parse_arguments(&arguments)?;
        #[allow(deprecated)]
        let fallback_cwd = turn.cwd.to_path_buf();
        let cwd = turn
            .environments
            .primary()
            .map(|environment| environment.cwd.to_path_buf())
            .unwrap_or(fallback_cwd);
        let record = build_record(&args);
        let target = vault_record_target(&cwd, &args)?;
        let file_call_id = format!("{call_id}-file");
        let write = match prepare_record_write(&target, &record, args.base_hash.as_deref()).await {
            Ok(write) => write,
            Err(err) => return Err(err),
        };
        let file_started_item = TurnItem::FileChange(FileChangeItem {
            id: file_call_id.clone(),
            changes: write.changes.clone(),
            status: None,
            auto_approved: Some(true),
            stdout: None,
            stderr: None,
        });
        session
            .emit_turn_item_started(&turn, &file_started_item)
            .await;

        if let Err(err) = commit_record_write(&target, &write).await {
            let file_completed_item = TurnItem::FileChange(FileChangeItem {
                id: file_call_id,
                changes: write.changes,
                status: Some(PatchApplyStatus::Failed),
                auto_approved: None,
                stdout: None,
                stderr: Some(err.to_string()),
            });
            session
                .emit_turn_item_completed(&turn, file_completed_item)
                .await;
            return Err(err);
        }
        let output_text = format!(
            "Brain/Wiki record appended\nresourceUri: {}\npath: {}\nbaseHash: {}\nnewHash: {}",
            target.resource_uri,
            target.path.display(),
            write.base_hash,
            write.new_hash
        );
        let file_completed_item = TurnItem::FileChange(FileChangeItem {
            id: file_call_id,
            changes: write.changes,
            status: Some(PatchApplyStatus::Completed),
            auto_approved: None,
            stdout: Some(output_text.clone()),
            stderr: Some(String::new()),
        });
        session
            .emit_turn_item_completed(&turn, file_completed_item)
            .await;

        Ok(boxed_tool_output(FunctionToolOutput::from_text(
            output_text,
            Some(true),
        )))
    }
}

impl CoreToolRuntime for BrainVaultPatchHandler {}

fn vault_record_target(
    cwd: &Path,
    args: &BrainVaultPatchArgs,
) -> Result<VaultRecordTarget, FunctionCallError> {
    let scope = normalized_value(args.scope.as_deref(), "project");
    let vault = normalized_value(args.vault.as_deref(), "brain");
    let location = normalized_value(args.location.as_deref(), "local");
    let phase = sanitize_segment(
        args.loop_phase
            .as_deref()
            .or(args.action.as_deref())
            .unwrap_or(DEFAULT_ACTION),
    );
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();

    let root = match (scope.as_str(), vault.as_str(), location.as_str()) {
        ("global", "wiki", _) => home_vault_root(".ilhae/wiki")?,
        ("global", _, _) => home_vault_root(".ilhae/brain")?,
        ("project", "wiki", "docs") => cwd.join("docs/wiki"),
        ("project", "wiki", _) => cwd.join(".ilhae/wiki"),
        ("project", _, _) => cwd.join(".ilhae/brain"),
        _ => cwd.join(".ilhae/brain"),
    };

    let relative_path = PathBuf::from("goal-loops").join(format!("{date}-{phase}.md"));
    let path = root.join(&relative_path);
    let display_path = path
        .strip_prefix(cwd)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| path.clone());
    let resource_uri = vault_resource_uri(&scope, &vault, &location, &relative_path);

    Ok(VaultRecordTarget {
        path,
        display_path,
        resource_uri,
    })
}

fn vault_resource_uri(scope: &str, vault: &str, location: &str, relative_path: &Path) -> String {
    let vault_segment = match (vault, location) {
        ("wiki", "docs") => "wiki-docs",
        ("wiki", _) => "wiki",
        _ => "brain",
    };
    let relative = relative_path
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    format!("brain://vault/{scope}/{vault_segment}/{relative}")
}

fn home_vault_root(relative: &str) -> Result<PathBuf, FunctionCallError> {
    let Some(home) = dirs::home_dir() else {
        return Err(FunctionCallError::RespondToModel(
            "cannot resolve home directory for global Brain/Wiki vault".to_string(),
        ));
    };
    Ok(home.join(relative))
}

fn normalized_value(value: Option<&str>, default: &str) -> String {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default)
        .to_ascii_lowercase()
}

fn sanitize_segment(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch.to_ascii_lowercase());
        } else if ch.is_whitespace() || ch == '/' {
            out.push('-');
        }
    }
    let out = out.trim_matches('-');
    if out.is_empty() {
        DEFAULT_ACTION.to_string()
    } else {
        out.to_string()
    }
}

fn build_record(args: &BrainVaultPatchArgs) -> String {
    let title = args
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or("Goal Loop Record");
    let action = args
        .action
        .as_deref()
        .map(str::trim)
        .filter(|action| !action.is_empty())
        .unwrap_or(DEFAULT_ACTION);
    let phase = args
        .loop_phase
        .as_deref()
        .map(str::trim)
        .filter(|phase| !phase.is_empty())
        .unwrap_or(action);
    let body = args
        .content
        .as_deref()
        .or(args.summary.as_deref())
        .map(str::trim)
        .filter(|content| !content.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| render_extra_fields(&args.extra));
    let timestamp = chrono::Utc::now().to_rfc3339();

    format!(
        "\n## {title}\n\n- timestamp: {timestamp}\n- action: {action}\n- loop_phase: {phase}\n\n{body}\n"
    )
}

fn render_extra_fields(extra: &serde_json::Map<String, Value>) -> String {
    if extra.is_empty() {
        return "No additional content supplied.".to_string();
    }
    let mut out = String::new();
    for (key, value) in extra {
        out.push_str("- ");
        out.push_str(key);
        out.push_str(": ");
        match value {
            Value::String(text) => out.push_str(text),
            _ => out.push_str(&value.to_string()),
        }
        out.push('\n');
    }
    out
}

async fn prepare_record_write(
    target: &VaultRecordTarget,
    record: &str,
    expected_base_hash: Option<&str>,
) -> Result<PreparedRecordWrite, FunctionCallError> {
    let previous = read_record_content(&target.path).await?;
    let previous_content = previous.as_deref().unwrap_or("");
    let base_hash = content_hash(previous_content);
    if let Some(expected_base_hash) = expected_base_hash
        && expected_base_hash != base_hash
    {
        return Err(FunctionCallError::RespondToModel(format!(
            "brain_vault_patch CAS mismatch for {}: expected baseHash {expected_base_hash}, current baseHash {base_hash}",
            target.path.display()
        )));
    }

    let next_content = format!("{previous_content}{record}");
    let new_hash = content_hash(&next_content);
    let change = file_change_for_record(target, previous.as_deref(), &next_content);
    let changes = HashMap::from([(target.display_path.clone(), change)]);

    Ok(PreparedRecordWrite {
        base_hash,
        new_hash,
        next_content,
        changes,
    })
}

#[allow(clippy::await_holding_invalid_type)]
async fn commit_record_write(
    target: &VaultRecordTarget,
    write: &PreparedRecordWrite,
) -> Result<(), FunctionCallError> {
    let lock = BRAIN_VAULT_WRITE_LOCK.get_or_init(|| tokio::sync::Mutex::new(()));
    let _guard = lock.lock().await;

    let latest = read_record_content(&target.path).await?;
    let latest_hash = content_hash(latest.as_deref().unwrap_or(""));
    if latest_hash != write.base_hash {
        return Err(FunctionCallError::RespondToModel(format!(
            "brain_vault_patch CAS mismatch for {}: prepared baseHash {}, current baseHash {latest_hash}",
            target.path.display(),
            write.base_hash
        )));
    }

    let Some(parent) = target.path.parent() else {
        return Err(FunctionCallError::RespondToModel(
            "brain_vault_patch target has no parent directory".to_string(),
        ));
    };
    tokio::fs::create_dir_all(parent).await.map_err(|err| {
        FunctionCallError::RespondToModel(format!(
            "failed to create Brain/Wiki directory {}: {err}",
            parent.display()
        ))
    })?;
    tokio::fs::write(&target.path, write.next_content.as_bytes())
        .await
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "failed to write Brain/Wiki record {}: {err}",
                target.path.display()
            ))
        })?;
    Ok(())
}

async fn read_record_content(path: &Path) -> Result<Option<String>, FunctionCallError> {
    match tokio::fs::read_to_string(path).await {
        Ok(content) => Ok(Some(content)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(FunctionCallError::RespondToModel(format!(
            "failed to read Brain/Wiki record {}: {err}",
            path.display()
        ))),
    }
}

fn file_change_for_record(
    target: &VaultRecordTarget,
    previous: Option<&str>,
    next_content: &str,
) -> FileChange {
    match previous {
        Some(previous) => FileChange::Update {
            unified_diff: similar::TextDiff::from_lines(previous, next_content)
                .unified_diff()
                .context_radius(3)
                .header(
                    &format!("a/{}", target.display_path.display()),
                    &format!("b/{}", target.display_path.display()),
                )
                .to_string(),
            move_path: None,
        },
        None => FileChange::Add {
            content: next_content.to_string(),
        },
    }
}

fn content_hash(content: &str) -> String {
    let mut hasher = sha1::Sha1::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args_for_path() -> BrainVaultPatchArgs {
        BrainVaultPatchArgs {
            action: Some("record_decision".to_string()),
            scope: None,
            vault: None,
            location: None,
            loop_phase: Some("decision".to_string()),
            title: None,
            content: None,
            summary: None,
            base_hash: None,
            extra: serde_json::Map::new(),
        }
    }

    #[test]
    fn brain_vault_patch_record_path_defaults_to_project_brain() {
        let path = vault_record_target(Path::new("/tmp/project"), &args_for_path())
            .expect("path")
            .path;
        assert!(path.starts_with("/tmp/project/.ilhae/brain/goal-loops"));
        assert!(path.to_string_lossy().ends_with("-decision.md"));
    }

    #[test]
    fn brain_vault_patch_sanitizes_phase_path_segment() {
        assert_eq!(
            sanitize_segment("Decision Loop / Phase"),
            "decision-loop---phase"
        );
    }

    #[test]
    fn brain_vault_patch_resource_uri_includes_scope_vault_and_relative_path() {
        let target =
            vault_record_target(Path::new("/tmp/project"), &args_for_path()).expect("path");
        assert_eq!(
            target.resource_uri,
            format!(
                "brain://vault/project/brain/goal-loops/{}",
                target.path.file_name().expect("filename").to_string_lossy()
            )
        );
        assert!(target.display_path.starts_with(".ilhae/brain/goal-loops"));
    }

    #[test]
    fn brain_vault_patch_file_change_uses_patch_for_existing_content() {
        let target = VaultRecordTarget {
            path: PathBuf::from("/tmp/project/.ilhae/brain/goal-loops/record.md"),
            display_path: PathBuf::from(".ilhae/brain/goal-loops/record.md"),
            resource_uri: "brain://vault/project/brain/goal-loops/record.md".to_string(),
        };
        let change = file_change_for_record(&target, Some("old\n"), "old\nnew\n");
        let FileChange::Update { unified_diff, .. } = change else {
            panic!("expected update change");
        };
        assert!(unified_diff.contains("+new"));
    }
}
