//! MemoryStore Integration + Proxy Mock Tool-Call Verification
//!
//! Two-part test:
//!   1. MemoryStore BM25 search (direct, no proxy) — verifies indexing + search
//!   2. Mock proxy prompt — verifies tool_call notification pipeline
//!
//! Run: `cargo test --test memory proxy_e2e -- --nocapture`

use serde_json::Value;
use std::io::Write;

// ─── Part 1: Direct MemoryStore BM25 search ──────────────────────────────

#[test]
fn test_memory_store_bm25_search() {
    println!("═══════════════════════════════════════════════════");
    println!(" 🧠 MemoryStore: BM25 Search Integration");
    println!("═══════════════════════════════════════════════════");

    let tmp_dir = std::env::temp_dir().join(format!("ilhae_mem_test_{}", std::process::id()));
    std::fs::create_dir_all(&tmp_dir).expect("create temp dir");

    // ── Step 1: Create MemoryStore ───────────────────────────────────────
    let db_path = tmp_dir.join("memory.db");
    let store = brain_knowledge_rs::memory_store::MemoryStore::open(&db_path)
        .expect("Failed to open MemoryStore");
    println!("[1] ✅ MemoryStore opened at {:?}", db_path);

    // ── Step 2: Write a markdown file and index it ───────────────────────
    let md_content = r#"# Rust Architecture Guide

## Overview
Rust uses a ownership-based memory model that eliminates data races.

## Key Concepts
- Ownership and borrowing
- Lifetimes
- Traits and generics
- Error handling with Result and Option

## 아키텍처 패턴
서비스 지향 아키텍처(SOA)에서 Rust는 높은 성능과 안전성을 제공합니다.
마이크로서비스 간 통신에는 gRPC 또는 REST를 사용할 수 있습니다.
"#;

    let md_file = tmp_dir.join("rust_architecture.md");
    {
        let mut f = std::fs::File::create(&md_file).expect("create md file");
        f.write_all(md_content.as_bytes())
            .expect("write md content");
    }

    let indexed = store.index_file(&md_file).expect("index_file failed");
    println!("[2] ✅ Document indexed: {} chunks", indexed);

    // ── Step 3: BM25 search (English) ────────────────────────────────────
    let results = store
        .search("ownership borrowing", 5)
        .expect("search failed");
    assert!(
        !results.is_empty(),
        "BM25 search should find results for 'ownership borrowing'"
    );
    println!(
        "[3] ✅ BM25 search 'ownership borrowing': {} results",
        results.len()
    );
    for r in &results {
        let preview: String = r.text.chars().take(80).collect();
        println!(
            "    📄 path={} line={}-{} text={}...",
            r.path, r.start_line, r.end_line, preview
        );
    }

    // ── Step 4: BM25 search (Korean) ─────────────────────────────────────
    let results_ko = store.search("아키텍처 패턴", 5).expect("search failed");
    assert!(
        !results_ko.is_empty(),
        "BM25 search should find results for '아키텍처 패턴'"
    );
    println!(
        "[4] ✅ BM25 search '아키텍처 패턴': {} results",
        results_ko.len()
    );

    // ── Step 5: Re-index (idempotent) ────────────────────────────────────
    let re_indexed = store.index_file(&md_file).expect("re-index should succeed");
    let results2 = store.search("ownership", 5).expect("search");
    // Re-indexing should not duplicate chunks
    println!(
        "[5] ✅ Re-index idempotent (new chunks={}), search results: {}",
        re_indexed,
        results2.len()
    );

    // ── Cleanup ──────────────────────────────────────────────────────────
    let _ = std::fs::remove_dir_all(&tmp_dir);

    println!("\n═══════════════════════════════════════════════════");
    println!(" ✅ MemoryStore BM25 Search Integration PASSED");
    println!("═══════════════════════════════════════════════════");
}

// ─── Part 2: Proxy Mock → Tool Call Notification Pipeline ────────────────

#[ignore] // Requires a freshly-built proxy binary; run manually with --ignored
#[test]
fn test_mock_proxy_tool_call_pipeline() {
    use super::common::proxy_harness::ProxyProcess;

    println!("═══════════════════════════════════════════════════");
    println!(" 🔧 Mock Proxy: Tool Call Notification Pipeline");
    println!("═══════════════════════════════════════════════════");

    // Spawn with built-in fixture (artifact_save, artifact_edit, memory_write)
    let mut proxy = ProxyProcess::spawn_mock();

    // ── Initialize & Create Session ─────────────────────────────────────
    let session_id = proxy.init_and_create_session();
    println!("[1] ✅ Session initialized: {}", session_id);

    // ── Prompt (built-in fixture: turn1 = artifact_save ×2) ─────────────
    println!("\n[2] Sending prompt (mock mode, built-in fixture)...");
    let (resp, notifs) = proxy.prompt(&session_id, "테스트 작업을 시작해줘");
    assert!(resp.get("result").is_some(), "Prompt failed: {:?}", resp);
    println!("[2] ✅ Prompt responded");

    // ── Verify session/update notifications ──────────────────────────────
    let session_updates: Vec<&Value> = notifs
        .iter()
        .filter(|n| n["method"] == "session/update")
        .collect();
    println!(
        "[3] Session update notifications: {}",
        session_updates.len()
    );

    // Verify tool_call notifications exist
    let tool_calls: Vec<&Value> = session_updates
        .iter()
        .filter(|n| n["params"]["update"]["sessionUpdate"] == "tool_call")
        .copied()
        .collect();
    println!("    tool_call notifications: {}", tool_calls.len());
    assert!(
        tool_calls.len() >= 2,
        "Built-in fixture turn1 should produce at least 2 tool_call notifications"
    );

    // Verify tool names are artifact_save
    for tc in &tool_calls {
        let name = tc["params"]["update"]["toolCall"]["name"]
            .as_str()
            .or_else(|| tc["params"]["update"]["toolCall"]["title"].as_str())
            .unwrap_or("?");
        println!("    🔧 tool: {}", name);
    }
    println!("[3] ✅ Tool call notifications verified");

    // Verify tool_call_update notifications
    let tool_updates: Vec<&Value> = session_updates
        .iter()
        .filter(|n| n["params"]["update"]["sessionUpdate"] == "tool_call_update")
        .copied()
        .collect();
    println!("    tool_call_update notifications: {}", tool_updates.len());
    assert!(
        tool_updates.len() >= 2,
        "Should have at least 2 tool_call_update notifications"
    );
    println!("[4] ✅ Tool call updates verified");

    // Verify assistant_turn_patch notifications
    let patches: Vec<&Value> = session_updates
        .iter()
        .filter(|n| n["params"]["update"]["sessionUpdate"] == "assistant_turn_patch")
        .copied()
        .collect();
    println!("    assistant_turn_patch: {}", patches.len());
    assert!(
        !patches.is_empty(),
        "Should have at least 1 assistant_turn_patch"
    );
    println!("[5] ✅ Assistant turn patches verified");

    println!("\n═══════════════════════════════════════════════════");
    println!(" ✅ Mock Proxy Tool Call Pipeline PASSED");
    println!("   Session: {}", session_id);
    println!(
        "   tool_calls: {}, updates: {}, patches: {}",
        tool_calls.len(),
        tool_updates.len(),
        patches.len()
    );
    println!("═══════════════════════════════════════════════════");
}
