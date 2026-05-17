use super::*;
use crate::session::tests::make_session_and_context;
use crate::tools::context::ToolCallSource;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::registry::ToolHandler;
use crate::turn_diff_tracker::TurnDiffTracker;
use serde_json::json;
use std::process::Command as StdCommand;
use std::sync::Arc;
use tempfile::tempdir;
use tokio::sync::Mutex;

#[test]
fn parses_basic_results() {
    let stdout = b"/tmp/file_a.rs\n/tmp/file_b.rs\n";
    let parsed = parse_results(stdout, 10);
    assert_eq!(
        parsed,
        vec!["/tmp/file_a.rs".to_string(), "/tmp/file_b.rs".to_string()]
    );
}

#[test]
fn parse_truncates_after_limit() {
    let stdout = b"/tmp/file_a.rs\n/tmp/file_b.rs\n/tmp/file_c.rs\n";
    let parsed = parse_results(stdout, 2);
    assert_eq!(
        parsed,
        vec!["/tmp/file_a.rs".to_string(), "/tmp/file_b.rs".to_string()]
    );
}

#[tokio::test]
async fn run_search_returns_results() -> anyhow::Result<()> {
    if !rg_available() {
        return Ok(());
    }
    let temp = tempdir().expect("create temp dir");
    let dir = temp.path();
    std::fs::write(dir.join("match_one.txt"), "alpha beta gamma").unwrap();
    std::fs::write(dir.join("match_two.txt"), "alpha delta").unwrap();
    std::fs::write(dir.join("other.txt"), "omega").unwrap();

    let results = run_rg_search("alpha", None, dir, 10, dir).await?;
    assert_eq!(results.len(), 2);
    assert!(results.iter().any(|path| path.ends_with("match_one.txt")));
    assert!(results.iter().any(|path| path.ends_with("match_two.txt")));
    Ok(())
}

#[tokio::test]
async fn run_search_with_glob_filter() -> anyhow::Result<()> {
    if !rg_available() {
        return Ok(());
    }
    let temp = tempdir().expect("create temp dir");
    let dir = temp.path();
    std::fs::write(dir.join("match_one.rs"), "alpha beta gamma").unwrap();
    std::fs::write(dir.join("match_two.txt"), "alpha delta").unwrap();

    let results = run_rg_search("alpha", Some("*.rs"), dir, 10, dir).await?;
    assert_eq!(results.len(), 1);
    assert!(results.iter().all(|path| path.ends_with("match_one.rs")));
    Ok(())
}

#[tokio::test]
async fn run_search_respects_limit() -> anyhow::Result<()> {
    if !rg_available() {
        return Ok(());
    }
    let temp = tempdir().expect("create temp dir");
    let dir = temp.path();
    std::fs::write(dir.join("one.txt"), "alpha one").unwrap();
    std::fs::write(dir.join("two.txt"), "alpha two").unwrap();
    std::fs::write(dir.join("three.txt"), "alpha three").unwrap();

    let results = run_rg_search("alpha", None, dir, 2, dir).await?;
    assert_eq!(results.len(), 2);
    Ok(())
}

#[tokio::test]
async fn run_search_handles_no_matches() -> anyhow::Result<()> {
    if !rg_available() {
        return Ok(());
    }
    let temp = tempdir().expect("create temp dir");
    let dir = temp.path();
    std::fs::write(dir.join("one.txt"), "omega").unwrap();

    let results = run_rg_search("alpha", None, dir, 5, dir).await?;
    assert!(results.is_empty());
    Ok(())
}

#[tokio::test]
async fn no_match_hint_does_not_reference_removed_list_dir_tool() -> anyhow::Result<()> {
    if !rg_available() {
        return Ok(());
    }
    let temp = tempdir().expect("create temp dir");
    let dir = temp.path();
    std::fs::write(dir.join("one.txt"), "omega").unwrap();
    let (session, turn) = make_session_and_context().await;
    let output = GrepSearchHandler
        .handle(ToolInvocation {
            session: session.into(),
            turn: turn.into(),
            cancellation_token: tokio_util::sync::CancellationToken::new(),
            tracker: Arc::new(Mutex::new(TurnDiffTracker::new())),
            call_id: "call-grep-search".to_string(),
            tool_name: ToolName::plain("grep_search"),
            source: ToolCallSource::Direct,
            payload: ToolPayload::Function {
                arguments: json!({
                    "query": "alpha",
                    "path": dir,
                    "limit": 5
                })
                .to_string(),
            },
        })
        .await?;

    let hint = output.hint.expect("no-match hint");
    assert!(!hint.contains("list_dir"), "{hint}");
    Ok(())
}

fn rg_available() -> bool {
    StdCommand::new("rg")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}
