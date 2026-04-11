# Ilhae Session/Harness Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `codex-rs/ilhae`에서 durable session state와 process-local harness state를 분리해서, 세션 오염, 잘못된 artifact 저장, 체크포인트 오복구, 신규 첨부파일 실패를 없앤다.

**Architecture:** `brain`과 `brain-session-rs`를 세션의 단일 진실 공급원으로 유지하고, 프록시 프로세스 안의 전역 mutable state는 세션 스코프나 요청 스코프로 내린다. `session`은 복구 가능한 사실 흐름으로 유지하고, `harness`는 라우팅/캐시/실행만 담당하도록 정리한다.

**Tech Stack:** Rust 2024, Tokio, moka, rusqlite, sacp, `brain-rs`, `brain-session-rs`

---

## Review Summary

- 현재 `active_session_id`가 전역 단일 값이라서 artifact 저장, 파일 쓰기 sandbox, 팀 위임 컨텍스트가 마지막 활성 세션에 묶인다.
- 현재 `channel_memory`가 전역 단일 맵이라서 여러 팀 세션이 동시에 돌면 체크포인트와 resume가 서로 덮어쓴다.
- 현재 `ensure_session()`이 SQLite에서는 idempotent처럼 보이지만 markdown dual-write는 기존 세션을 초기 상태로 덮어쓸 수 있다.
- 현재 신규 모바일 세션은 기본 `cwd`가 `/`라서 첨부파일 저장이 일반 권한 환경에서 실패할 가능성이 높다.

## Workstream 1: Remove Global `active_session_id` As A Hidden Session Selector

**Priority:** P0

**Why:** 이 값 하나에 artifact 저장, fs write sandbox, team delegation context가 기대고 있어서 동시 세션에서 잘못된 세션으로 write가 붙을 수 있다.

**Files:**
- Modify: `codex-rs/ilhae/src/shared_state.rs`
- Modify: `codex-rs/ilhae/src/relay_commands/chat.rs`
- Modify: `codex-rs/ilhae/src/context_proxy/prompt.rs`
- Modify: `codex-rs/ilhae/src/builtins/artifact.rs`
- Modify: `codex-rs/ilhae/src/context_proxy/fs_handlers.rs`
- Modify: `codex-rs/ilhae/src/builtins/team.rs`
- Modify: `codex-rs/ilhae/src/tools_proxy.rs`
- Test: `codex-rs/ilhae/tests/team/brain_session.rs`
- Test: `codex-rs/ilhae/tests/proxy/integration.rs`

**Action items:**
- [ ] `SessionState.active_session_id`를 전역 단일 `RwLock<String>`에서 세션별/요청별 컨텍스트 조회 구조로 바꾼다.
- [ ] artifact 계열 도구는 “현재 전역 세션”을 읽지 말고 호출 컨텍스트에서 명시적 `session_id`를 받거나, 최소한 `McpConnection`에 바인딩된 세션을 사용하게 바꾼다.
- [ ] `fs/write_text_file`의 subagent 차단은 `active_session_id.starts_with("subagent_")`로 판단하지 말고, 요청 세션 또는 agent mode 메타데이터로 판단하게 바꾼다.
- [ ] team delegation 시작/완료/백그라운드 구독 경로는 전역 `active_session_id` 의존을 제거하고, delegation을 시작한 세션 ID를 명시적으로 넘기게 바꾼다.
- [ ] `relay_commands/chat.rs`와 `context_proxy/prompt.rs`에서 “현재 세션”을 side effect로 기록하는 코드를 줄이고, 세션별 캐시 갱신만 남긴다.

**Acceptance criteria:**
- [ ] 두 개의 세션이 번갈아 요청을 보내도 artifact가 다른 세션에 저장되지 않는다.
- [ ] subagent가 아닌 일반 세션의 파일 쓰기가 다른 세션 상태 때문에 차단되지 않는다.
- [ ] background delegation 완료 알림과 checkpoint 저장이 delegation을 시작한 세션에 귀속된다.

**Verification:**
- [ ] 새 테스트를 추가해서 세션 A와 세션 B가 교차 호출될 때 artifact 저장 대상이 섞이지 않는지 검증한다.
- [ ] 새 테스트를 추가해서 subagent 세션과 일반 세션이 섞인 상태에서 fs write 차단이 요청 세션 기준으로만 동작하는지 검증한다.

## Workstream 2: Make Team Shared State Session-Scoped

**Priority:** P0

**Why:** `channel_memory`가 전역 하나라서 팀 세션 여러 개가 동시에 실행되면 체크포인트와 auto-resume가 잘못된 세션 메모리를 덮어쓴다.

**Files:**
- Modify: `codex-rs/ilhae/src/shared_state.rs`
- Modify: `codex-rs/ilhae/src/startup_main.rs`
- Modify: `codex-rs/ilhae/src/builtins/team.rs`
- Modify: `codex-rs/ilhae/src/process_supervisor.rs`
- Test: `codex-rs/ilhae/tests/team/persistence.rs`
- Test: `codex-rs/ilhae/tests/team/brain_session.rs`

**Action items:**
- [ ] `TeamState.channel_memory`를 전역 `HashMap<String, Value>` 하나가 아니라 `session_id` 또는 `(session_id, thread_id)` 기준으로 분리된 저장소로 바꾼다.
- [ ] `team_update_channel`, `team_read_channel`, `team_save_checkpoint`, `team_resume_task`가 모두 명시적 세션 컨텍스트를 사용하도록 바꾼다.
- [ ] `auto_save_checkpoint()`는 호출 시 받은 `session_id`의 channel state만 serialize 하도록 바꾼다.
- [ ] `auto_restore_latest_checkpoint()`는 전역 `active_session_id`를 읽지 말고, 실제 재시작된 팀 세션 ID를 인자로 받도록 바꾼다.
- [ ] `process_supervisor`에서 agent restart 후 복구할 때 “현재 프록시에서 마지막으로 활성화된 세션”이 아니라 “해당 팀 프로세스가 속한 세션”을 복구하도록 바꾼다.

**Acceptance criteria:**
- [ ] 팀 세션 A와 팀 세션 B가 동시에 checkpoint를 저장해도 서로의 channel state를 덮어쓰지 않는다.
- [ ] 팀 에이전트 재시작 후 auto-resume가 올바른 세션의 최신 checkpoint만 복구한다.
- [ ] `team_read_channel`이 다른 세션의 메모리를 노출하지 않는다.

**Verification:**
- [ ] 새 테스트를 추가해서 세션 A/B가 서로 다른 channel 값을 저장한 뒤 각각 resume했을 때 자기 값만 복원되는지 검증한다.
- [ ] 새 테스트를 추가해서 재시작된 role 목록과 연결된 세션만 auto-restore 되는지 검증한다.

## Workstream 3: Make `ensure_session()` Truly Idempotent

**Priority:** P0

**Why:** 현재 `ensure_session()`은 DB row 생성은 무시해도 markdown dual-write는 기존 세션을 `Untitled`, `message_count: 0`, 새 `created_at`으로 다시 써 버릴 수 있다.

**Files:**
- Modify: `brain-session-rs/src/session_store.rs`
- Modify: `brain-session-rs/src/brain_session_writer.rs`
- Modify: `brain-rs/src/service/session.rs`
- Modify: `codex-rs/ilhae/src/persistence_proxy.rs`
- Modify: `codex-rs/ilhae/src/session_context_service.rs`
- Test: `brain-session-rs/src/session_store.rs`

**Action items:**
- [ ] `ensure_session_with_channel_meta_engine()`에서 실제 insert가 일어났을 때만 markdown write를 하도록 바꾼다.
- [ ] 기존 세션이 존재하면 `ensure_session()`은 metadata reset 없이 no-op이어야 한다.
- [ ] cross-agent load 경로에서 기존 세션에 대해 `ensure_session(..., "/")`를 다시 호출하는 코드를 제거하거나, metadata preservation이 보장되는 함수로 교체한다.
- [ ] 기존 세션 owner만 바꿔야 하는 경우 `update_session_agent_id()`만 수행하고, `cwd`, `title`, `created_at`, `message_count`를 건드리지 않게 정리한다.
- [ ] `brain_session_writer.write_session()`을 호출하는 경로와 `append_message()` 경로의 역할을 재정리해서 “ensure”가 전체 세션 스냅샷 write를 유발하지 않게 한다.

**Acceptance criteria:**
- [ ] 이미 메시지가 있는 세션에 `ensure_session()`을 여러 번 호출해도 markdown 파일의 title/message_count/created_at이 초기 상태로 돌아가지 않는다.
- [ ] cross-agent handoff 후에도 기존 세션의 `cwd`와 title이 유지된다.
- [ ] owner 변경이 필요한 경우 `agent_id`만 바뀌고 다른 필드는 유지된다.

**Verification:**
- [ ] `brain-session-rs`에 회귀 테스트를 추가해서 동일 세션에 대한 2회 `ensure_session()` 후 markdown/frontmatter가 유지되는지 검증한다.
- [ ] cross-agent continuity 경로를 위한 테스트를 추가해서 handoff 후 세션 metadata가 보존되는지 검증한다.

## Workstream 4: Fix New-Session Attachment Storage

**Priority:** P1

**Why:** 새 모바일 세션은 기본 `cwd`가 `/`라서 첨부파일 저장 경로가 `/ilhae-uploads/...`가 된다.

**Files:**
- Modify: `codex-rs/ilhae/src/relay_commands/chat.rs`
- Modify: `codex-rs/ilhae/src/helpers.rs`
- Test: `codex-rs/ilhae/tests/agent_chat/mock_chat.rs`
- Test: `codex-rs/ilhae/tests/proxy/integration.rs`

**Action items:**
- [ ] 새 세션 생성 시 기본 `cwd`를 `/`가 아니라 실제 쓰기 가능한 작업 디렉터리로 설정한다.
- [ ] `save_mobile_attachments_to_cwd()`는 루트(`/`)나 빈 값이 들어오면 안전한 fallback 디렉터리로 내려가도록 바꾼다.
- [ ] 오류 메시지에 현재 세션 ID와 최종 fallback 경로를 포함해서 운영 중 디버깅이 가능하게 한다.
- [ ] 첨부파일이 없는 일반 메시지 경로에는 영향이 없도록 helper 단위에서 경계 조건을 테스트한다.

**Acceptance criteria:**
- [ ] 신규 세션 + attachment 조합이 비루트 권한 환경에서도 성공한다.
- [ ] 세션 `cwd`가 비어 있거나 `/`여도 첨부파일이 안전한 앱 데이터 디렉터리 아래에 저장된다.
- [ ] 저장 실패 시 경로 정보가 로그와 오류 응답에 남는다.

**Verification:**
- [ ] helper 단위 테스트를 추가해서 `/`, `""`, 정상 작업 디렉터리 입력을 각각 검증한다.
- [ ] 프록시 통합 테스트를 추가해서 첫 메시지에 attachment가 포함된 경우 성공하는지 검증한다.

## Execution Order

- [ ] 1단계: Workstream 3부터 처리해서 `session` persistence를 truly idempotent하게 만든다.
- [ ] 2단계: Workstream 1을 처리해서 잘못된 전역 세션 선택을 제거한다.
- [ ] 3단계: Workstream 2를 처리해서 팀 상태를 세션 스코프로 분리한다.
- [ ] 4단계: Workstream 4로 신규 attachment 경로 회귀를 막는다.
- [ ] 5단계: 전체 회귀 테스트를 실행하고, session A/B 동시성 시나리오를 추가로 검증한다.

## Guardrails

- [ ] durable state는 `brain`/`brain-session-rs`가 소유하고, `ilhae` 프록시 안의 캐시는 복구 가능한 파생 상태만 가져야 한다.
- [ ] 세션 선택은 전역 mutable singleton이 아니라 요청 메타데이터나 세션 바운드 컨텍스트에서 결정해야 한다.
- [ ] 체크포인트는 항상 `(session_id, thread_id)` 단위로 읽고 써야 하며, “현재 활성 세션” 같은 암묵적 선택자를 사용하지 않는다.
- [ ] `ensure_*` 계열 함수는 metadata reset이나 full snapshot rewrite를 일으키면 안 된다.

## Suggested Verification Commands

- [ ] Run: `cargo test -p codex-ilhae`
- [ ] Run: `cargo test -p brain-session-rs`
- [ ] Run: `cargo test -p brain-rs`
- [ ] Run targeted tests for any new cross-session isolation cases added under `codex-rs/ilhae/tests/team/`

