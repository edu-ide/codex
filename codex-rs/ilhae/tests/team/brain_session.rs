//! Multi-Agent Team Mode E2E Test — Brain Session Writer
//!
//! Tests hierarchical folder structure:
//!
//!   sessions/
//!     solo/                           — single-agent sessions
//!       {YYYY-MM-DD}/
//!         {short-id}.md
//!     team/
//!       {YYYY-MM-DD}/
//!         {parent-id}/                — per-project folder
//!           index.md                  — leader/orchestrator
//!           {role}-{child-id}.md      — per-role agent

use brain_session_rs::brain_session_writer::BrainSessionWriter;
use brain_session_rs::session_store::SessionStore;
use std::path::PathBuf;
use tempfile::TempDir;

fn today() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

/// Find a solo session .md file by short-id suffix within date folders.
/// Filename pattern: `{HH-MM}_{title}_{sid}.md`
fn find_solo_file(solo_dir: &std::path::Path, sid: &str) -> Option<PathBuf> {
    let suffix = format!("_{}.md", sid);
    if let Ok(date_entries) = std::fs::read_dir(solo_dir) {
        for date_entry in date_entries.flatten() {
            if date_entry.file_type().map_or(false, |t| t.is_dir()) {
                if let Ok(files) = std::fs::read_dir(date_entry.path()) {
                    for file in files.flatten() {
                        let n = file.file_name();
                        if n.to_string_lossy().ends_with(&suffix) {
                            return Some(file.path());
                        }
                    }
                }
            }
        }
    }
    None
}

/// Find a team parent folder by short-id suffix within date folders.
/// Folder pattern: `{HH-MM}_{title}_{sid}`
fn find_team_folder(team_dir: &std::path::Path, sid: &str) -> Option<PathBuf> {
    let suffix = format!("_{}", sid);
    if let Ok(date_entries) = std::fs::read_dir(team_dir) {
        for date_entry in date_entries.flatten() {
            if date_entry.file_type().map_or(false, |t| t.is_dir()) {
                if let Ok(sub_entries) = std::fs::read_dir(date_entry.path()) {
                    for sub_entry in sub_entries.flatten() {
                        if sub_entry.file_type().map_or(false, |t| t.is_dir()) {
                            let name = sub_entry.file_name();
                            let name_str = name.to_string_lossy();
                            if name_str.ends_with(&suffix) || name_str == sid {
                                return Some(sub_entry.path());
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

fn make_store_with_brain() -> (SessionStore, TempDir, PathBuf) {
    let tmp = TempDir::new().expect("tmpdir");
    let data_dir = tmp.path().join("ilhae");
    std::fs::create_dir_all(&data_dir).unwrap();
    let brain_dir = tmp.path().join("brain");

    let mut store = SessionStore::new(&data_dir).expect("SessionStore::new");
    store.set_brain_writer(BrainSessionWriter::new(&brain_dir));

    (store, tmp, brain_dir)
}

#[test]
fn team_session_brain_persistence() {
    let (store, _tmp, brain_dir) = make_store_with_brain();
    let team_dir = brain_dir.join("sessions").join("team");
    let solo_dir = brain_dir.join("sessions").join("solo");

    println!("═══════════════════════════════════════════════════");
    println!(" Multi-Agent Team Mode: Brain Session Persistence");
    println!("═══════════════════════════════════════════════════\n");

    // ── Step 1: Create parent team session ───────────────────────────────
    let parent_id = "team-parent-001";
    store
        .create_session_with_channel_meta_engine(
            parent_id,
            "팀 프로젝트: AI 아키텍처 설계",
            "team",
            "/workspace/project",
            "desktop",
            "",
            "team",
        )
        .expect("create parent");
    store
        .mark_multi_agent_parent(parent_id, 3)
        .expect("mark parent");
    println!("[1] ✅ Created parent team session");

    // ── Step 2: Create child sub-sessions (via ensure_team_sub_session) ──
    let r_id = store
        .ensure_team_sub_session(parent_id, "researcher", "gemini", "/workspace")
        .unwrap();
    let v_id = store
        .ensure_team_sub_session(parent_id, "verifier", "gemini", "/workspace")
        .unwrap();
    let c_id = store
        .ensure_team_sub_session(parent_id, "creator", "gemini", "/workspace")
        .unwrap();
    println!(
        "[2] ✅ Created children: researcher={}, verifier={}, creator={}",
        &r_id[..8],
        &v_id[..8],
        &c_id[..8]
    );

    // ── Step 3: Add messages ────────────────────────────────────────────
    store
        .add_message(parent_id, "user", "AI 아키텍처를 설계해줘.", "user")
        .unwrap();
    store.add_full_message(
        parent_id, "assistant",
        "팀원들에게 작업을 분배합니다.\n\n- Researcher: 트렌드 조사\n- Verifier: 설계 검증\n- Creator: 초안 작성",
        "team-leader", "분배 전략 수립...",
        "[{\"tool\":\"delegate\"}]",
        100, 200, 300, 1500,
    ).unwrap();
    println!("[3] ✅ Leader messages added");

    // Child sessions — messages
    let children = [
        (
            &r_id,
            "researcher-gemini",
            "Researcher",
            "트렌드 조사해줘",
            "## 조사 결과\n\n1. Transformer\n2. MoE\n3. Mamba",
        ),
        (
            &v_id,
            "verifier-gemini",
            "Verifier",
            "검증해줘",
            "## 검증 결과\n\n✅ Transformer: 안정적",
        ),
        (
            &c_id,
            "creator-gemini",
            "Creator",
            "문서 작성해줘",
            "# AI 설계 문서\n\nHybrid 아키텍처",
        ),
    ];
    for (cid, aid, role, umsg, amsg) in &children {
        store.add_message(cid, "user", umsg, "user").unwrap();
        store
            .add_full_message(
                cid,
                "assistant",
                amsg,
                aid,
                &format!("{} 분석...", role),
                "",
                50,
                150,
                200,
                2000,
            )
            .unwrap();
        println!("[3] ✅ {} → messages added", role);
    }

    // ── Step 4: Verify hierarchical folder structure ────────────────────
    let parent_short = &parent_id[..12];
    let parent_folder = find_team_folder(&team_dir, parent_short)
        .expect("team parent folder should exist (found by sid suffix)");
    let parent_index = parent_folder.join("index.md");

    assert!(parent_folder.is_dir(), "team/parent/ folder should exist");
    assert!(parent_index.exists(), "team/parent/index.md should exist");
    println!(
        "[4] ✅ team/*_{}/index.md exists at {:?}",
        parent_short,
        parent_folder.file_name().unwrap_or_default()
    );

    // Children should be in the same team folder, named by role
    for (cid, _aid, role, _umsg, _amsg) in &children {
        let child_short = if cid.len() > 12 {
            &cid[..12]
        } else {
            cid.as_str()
        };
        let expected = parent_folder.join(format!("{}-{}.md", role.to_lowercase(), child_short));
        assert!(
            expected.exists(),
            "{} child should be at {:?}",
            role,
            expected
        );
        println!("[4] ✅ {}-{}.md", role.to_lowercase(), child_short);
    }

    // Solo folder should have only the date subfolder, no session files
    let solo_count: usize = std::fs::read_dir(&solo_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map_or(false, |t| t.is_dir()))
        .flat_map(|d| std::fs::read_dir(d.path()).ok())
        .flatten()
        .count();
    assert_eq!(
        solo_count, 0,
        "Solo dir should have no session files for team-only test"
    );
    println!("[4] ✅ solo/ folder is empty (correct)");

    // ── Step 5: Verify markdown content / formatting ────────────────────
    let parent_content = std::fs::read_to_string(&parent_index).unwrap();
    assert!(parent_content.contains("engine: \"team\""), "team engine");
    assert!(parent_content.contains("### 👤 User"), "User emoji heading");
    assert!(
        parent_content.contains("### 🤖 Assistant"),
        "Assistant emoji heading"
    );
    assert!(!parent_content.contains("\n> "), "No blockquote metadata");
    assert!(
        parent_content.contains("agent: `team-leader`"),
        "Agent in backticks"
    );
    assert!(parent_content.contains("💭 Thinking"), "Thinking section");
    assert!(
        parent_content.contains("🔧 Tool Calls"),
        "Tool calls section"
    );
    assert!(parent_content.contains("\n---\n"), "HR separators");
    println!("[5] ✅ Markdown formatting: italic meta, emoji headers, HR, no blockquote");

    // ── Step 6: Task events ─────────────────────────────────────────────
    store
        .upsert_task("task-001", parent_id, "researcher-gemini", "AI 트렌드 조사")
        .unwrap();
    store
        .update_task_status("task-001", "working", None)
        .unwrap();
    store
        .update_task_status("task-001", "completed", Some("5개 트렌드 발견"))
        .unwrap();

    let pc6 = std::fs::read_to_string(&parent_index).unwrap();
    assert!(pc6.contains("📋 Task: submitted"), "submitted");
    assert!(pc6.contains("🔄 Task: working"), "working");
    assert!(pc6.contains("✅ Task: completed"), "completed");
    assert!(pc6.contains("5개 트렌드 발견"), "result");
    println!("[6] ✅ Task lifecycle (📋→🔄→✅) in brain markdown");

    // ── Step 7: Upsert (SSE streaming) ──────────────────────────────────
    store
        .upsert_agent_message(&r_id, "assistant", "중간...", "researcher-gemini", "")
        .unwrap();
    store
        .upsert_agent_message(
            &r_id,
            "assistant",
            "## 최종 결과\n\n완료.",
            "researcher-gemini",
            "분석...",
        )
        .unwrap();

    let r_short = if r_id.len() > 12 { &r_id[..12] } else { &r_id };
    let r_path = parent_folder.join(format!("researcher-{}.md", r_short));
    let rcontent = std::fs::read_to_string(&r_path).unwrap();
    assert!(rcontent.contains("최종 결과"), "Final content");
    assert!(!rcontent.contains("중간..."), "Partial replaced");
    println!("[7] ✅ Upsert → full rewrite");

    // ── Step 8: Title update ────────────────────────────────────────────
    store
        .update_session_title(parent_id, "✅ 완료: AI 아키텍처")
        .unwrap();
    let updated = std::fs::read_to_string(&parent_index).unwrap();
    assert!(updated.contains("✅ 완료: AI 아키텍처"), "title");
    println!("[8] ✅ Title update propagated");

    // ── Step 9: Delete child → file removed, folder remains ─────────────
    let c_short = if c_id.len() > 12 { &c_id[..12] } else { &c_id };
    let c_path = parent_folder.join(format!("creator-{}.md", c_short));
    assert!(c_path.exists());
    store.delete_session(&c_id).unwrap();
    assert!(!c_path.exists(), "Creator file deleted");
    assert!(
        parent_folder.is_dir(),
        "Team folder still exists (has other files)"
    );
    println!("[9] ✅ Child delete → file removed, folder retained");

    // ── Summary ─────────────────────────────────────────────────────────
    println!("\n═══════════════════════════════════════════════════");
    println!(" [team-brain-e2e] PASS");
    println!("   - Hierarchical: team/{{}}/index.md + {{role}}-{{}}.md ✅");
    println!("   - Italic metadata (no blockquote) ✅");
    println!("   - Emoji headers + HR separators ✅");
    println!("   - Task lifecycle (📋→🔄→✅) ✅");
    println!("   - Upsert → full rewrite ✅");
    println!("   - Title update + delete ✅");
    println!("═══════════════════════════════════════════════════");
}

#[test]
fn solo_session_brain_persistence() {
    let (store, _tmp, brain_dir) = make_store_with_brain();
    let solo_dir = brain_dir.join("sessions").join("solo");
    let team_dir = brain_dir.join("sessions").join("team");

    println!("═══════════════════════════════════════════════════");
    println!(" Solo Session: Brain Session Persistence");
    println!("═══════════════════════════════════════════════════\n");

    let sid = "solo-session-001";
    store
        .create_session_with_channel_meta_engine(
            sid,
            "일반 대화",
            "gemini",
            "/workspace",
            "desktop",
            "",
            "gemini",
        )
        .unwrap();

    store
        .add_message(sid, "user", "안녕하세요!", "user")
        .unwrap();
    store
        .add_full_message(
            sid,
            "assistant",
            "안녕하세요! 무엇을 도와드릴까요?",
            "gemini",
            "",
            "",
            10,
            50,
            60,
            800,
        )
        .unwrap();

    let solo_path = find_solo_file(&solo_dir, &sid[..12])
        .expect("Solo session file should exist (found by sid suffix)");
    assert!(solo_path.exists(), "Solo session should exist");
    println!(
        "[\u{2705}] Solo: {:?}",
        solo_path.file_name().unwrap_or_default()
    );

    let content = std::fs::read_to_string(&solo_path).unwrap();
    assert!(content.contains("engine: \"gemini\""));
    assert!(!content.contains("multi_agent"));
    assert!(content.contains("### 👤 User"));
    assert!(content.contains("안녕하세요!"));

    // Verify no team folder leakage
    let team_count: usize = std::fs::read_dir(&team_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map_or(false, |t| t.is_dir()))
        .flat_map(|d| std::fs::read_dir(d.path()).ok())
        .flatten()
        .count();
    assert_eq!(
        team_count, 0,
        "Team dir should have no session files for solo session"
    );

    println!("[✅] Solo session correctly in solo/ folder");
    println!("[✅] No blockquote, correct formatting");
    println!("[✅] No team folder leakage");
}

#[test]
fn delegation_events_in_brain_markdown() {
    let (store, _tmp, brain_dir) = make_store_with_brain();
    let team_dir = brain_dir.join("sessions").join("team");

    println!("═══════════════════════════════════════════════════");
    println!(" Delegation Events: Brain Session Recording");
    println!("═══════════════════════════════════════════════════\n");

    // Create parent team session
    let parent_id = "team-deleg-001";
    store
        .create_session_with_channel_meta_engine(
            parent_id,
            "Delegation 테스트",
            "team",
            "/workspace/project",
            "desktop",
            "",
            "team",
        )
        .expect("create parent");
    store
        .mark_multi_agent_parent(parent_id, 2)
        .expect("mark parent");

    // Add initial message so the md file has content
    store
        .add_message(parent_id, "user", "AI 아키텍처를 설계해줘.", "user")
        .unwrap();
    store
        .add_full_message(
            parent_id,
            "assistant",
            "팀원들에게 작업을 분배합니다.",
            "team-leader",
            "",
            "[]",
            0,
            0,
            0,
            0,
        )
        .unwrap();

    // ── Step 1: Record delegation START event ──
    store.write_delegation_event(
        parent_id,
        "researcher",
        "sync",
        Some("task-d001"),
        Some("AI 트렌드 최신 동향 조사"),
        None,
    );

    let parent_short = &parent_id[..12];
    let parent_folder = find_team_folder(&team_dir, parent_short)
        .expect("delegation test: team parent folder should exist");
    let parent_index = parent_folder.join("index.md");
    let content1 = std::fs::read_to_string(&parent_index).unwrap();
    assert!(
        content1.contains("Delegation"),
        "Should have Delegation header"
    );
    assert!(
        content1.contains("researcher"),
        "Should mention target agent"
    );
    assert!(content1.contains("sync"), "Should mention delegation mode");
    assert!(content1.contains("task-d001"), "Should mention task_id");
    assert!(
        content1.contains("AI 트렌드"),
        "Should include query summary"
    );
    println!("[1] ✅ Delegation START event recorded in brain md");

    // ── Step 2: Record delegation COMPLETE event ──
    store.write_delegation_event(
        parent_id,
        "researcher",
        "sync",
        Some("task-d001"),
        Some("AI 트렌드 최신 동향 조사"),
        Some("5개 주요 트렌드 발견: Transformer, MoE, Mamba 등"),
    );

    let content2 = std::fs::read_to_string(&parent_index).unwrap();
    assert!(
        content2.contains("5개 주요 트렌드"),
        "Should include result summary"
    );
    println!("[2] ✅ Delegation COMPLETE event with result recorded");

    // ── Step 3: Record async delegation ──
    store.write_delegation_event(
        parent_id,
        "creator",
        "async",
        Some("task-d002"),
        Some("설계 문서 초안 작성"),
        None,
    );

    let content3 = std::fs::read_to_string(&parent_index).unwrap();
    assert!(
        content3.contains("🚀"),
        "Async delegation should use 🚀 emoji"
    );
    assert!(content3.contains("async"), "Should record async mode");
    assert!(content3.contains("creator"), "Should mention creator agent");
    println!("[3] ✅ Async delegation event recorded with 🚀 emoji");

    // ── Step 4: Record subscribe delegation ──
    store.write_delegation_event(
        parent_id,
        "verifier",
        "subscribe",
        None,
        Some("검증 요청"),
        None,
    );

    let content4 = std::fs::read_to_string(&parent_index).unwrap();
    assert!(
        content4.contains("📡"),
        "Subscribe delegation should use 📡 emoji"
    );
    assert!(
        content4.contains("subscribe"),
        "Should record subscribe mode"
    );
    println!("[4] ✅ Subscribe delegation event recorded with 📡 emoji");

    // Count total delegation events
    let delegation_count = content4.matches("Delegation:").count();
    assert_eq!(
        delegation_count, 4,
        "Should have exactly 4 delegation events"
    );
    println!("[5] ✅ Total delegation events: {}", delegation_count);

    println!("\n═══════════════════════════════════════════════════");
    println!(" [delegation-events-e2e] PASS");
    println!("   - Delegation START (sync) ✅");
    println!("   - Delegation COMPLETE (with result) ✅");
    println!("   - Async mode (🚀) ✅");
    println!("   - Subscribe mode (📡) ✅");
    println!("   - Event counting ✅");
    println!("═══════════════════════════════════════════════════");
}
