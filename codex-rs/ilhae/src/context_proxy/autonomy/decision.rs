use brain_session_rs::session_store::SessionStore;
use serde_json::json;

use super::build_ralph_loop_prompt;

pub const STOP_REASON_COMPLETE: &str = "complete";
pub const STOP_REASON_TIMEBOX: &str = "timebox_reached";
pub const STOP_REASON_MAX_TURNS: &str = "max_turns_reached";
pub const STOP_REASON_STALL: &str = "stalled_no_progress";
pub const STOP_REASON_USER_AGENT_FAILURE: &str = "user_agent_failure";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoUnusableKind {
    Empty,
    QuotaOrAgentError,
    Unreachable,
    RalphTemplate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassifiedUserAgentOutcome {
    Continue(String),
    Complete,
    Unusable {
        kind: AutoUnusableKind,
        detail: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoLoopDecision {
    Accept {
        text: String,
    },
    Complete,
    RecoverOnce {
        prompt: String,
        kind: AutoUnusableKind,
        detail: String,
    },
    Stop {
        stop_reason: String,
    },
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AutoDirectiveGate {
    pub consecutive_unusable: u32,
}

impl AutoDirectiveGate {
    pub fn step(
        &mut self,
        outcome: ClassifiedUserAgentOutcome,
        progress: &str,
    ) -> AutoLoopDecision {
        let decision = decide_auto_loop_action(outcome, self.consecutive_unusable, progress);
        match &decision {
            AutoLoopDecision::Accept { .. } | AutoLoopDecision::Complete => {
                self.consecutive_unusable = 0;
            }
            AutoLoopDecision::RecoverOnce { .. } | AutoLoopDecision::Stop { .. } => {
                self.consecutive_unusable = self.consecutive_unusable.saturating_add(1);
            }
        }
        decision
    }
}

pub fn classify_user_agent_text(text: &str) -> ClassifiedUserAgentOutcome {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return ClassifiedUserAgentOutcome::Unusable {
            kind: AutoUnusableKind::Empty,
            detail: String::new(),
        };
    }
    if is_quota_or_agent_error(trimmed) {
        return ClassifiedUserAgentOutcome::Unusable {
            kind: AutoUnusableKind::QuotaOrAgentError,
            detail: trimmed.to_string(),
        };
    }
    if is_unreachable_style_failure(trimmed) {
        return ClassifiedUserAgentOutcome::Unusable {
            kind: AutoUnusableKind::Unreachable,
            detail: trimmed.to_string(),
        };
    }
    if is_ralph_template_body(trimmed) {
        return ClassifiedUserAgentOutcome::Unusable {
            kind: AutoUnusableKind::RalphTemplate,
            detail: trimmed.to_string(),
        };
    }
    if trimmed.contains("완료되었습니다") || trimmed.contains("프로젝트를 종료") {
        return ClassifiedUserAgentOutcome::Complete;
    }
    ClassifiedUserAgentOutcome::Continue(trimmed.to_string())
}

pub fn classify_user_agent_error(error: &str) -> ClassifiedUserAgentOutcome {
    let trimmed = error.trim();
    if is_quota_or_agent_error(trimmed) {
        return ClassifiedUserAgentOutcome::Unusable {
            kind: AutoUnusableKind::QuotaOrAgentError,
            detail: trimmed.to_string(),
        };
    }
    ClassifiedUserAgentOutcome::Unusable {
        kind: AutoUnusableKind::Unreachable,
        detail: trimmed.to_string(),
    }
}

pub fn decide_auto_loop_action(
    outcome: ClassifiedUserAgentOutcome,
    consecutive_unusable: u32,
    progress: &str,
) -> AutoLoopDecision {
    match outcome {
        ClassifiedUserAgentOutcome::Continue(text) => AutoLoopDecision::Accept { text },
        ClassifiedUserAgentOutcome::Complete => AutoLoopDecision::Complete,
        ClassifiedUserAgentOutcome::Unusable { kind, detail } => {
            if consecutive_unusable >= 1 {
                AutoLoopDecision::Stop {
                    stop_reason: STOP_REASON_USER_AGENT_FAILURE.to_string(),
                }
            } else {
                AutoLoopDecision::RecoverOnce {
                    prompt: build_ralph_loop_prompt(progress),
                    kind,
                    detail,
                }
            }
        }
    }
}

pub fn followup_text_to_persist(decision: &AutoLoopDecision) -> Option<&str> {
    match decision {
        AutoLoopDecision::Accept { text } => Some(text.as_str()),
        AutoLoopDecision::Complete
        | AutoLoopDecision::RecoverOnce { .. }
        | AutoLoopDecision::Stop { .. } => None,
    }
}

pub fn persist_autonomous_followup(
    store: &SessionStore,
    session_id: &str,
    directive: &str,
) -> Result<(), String> {
    match store.get_session(session_id) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return Err(format!(
                "failed to persist auto follow-up: session `{session_id}` not found"
            ));
        }
        Err(error) => {
            return Err(format!("failed to persist auto follow-up: {error}"));
        }
    }

    let directive_blocks = serde_json::to_string(&vec![json!({
        "type": "text",
        "text": directive
    })])
    .unwrap_or_else(|_| "[]".to_string());

    store
        .add_full_message_with_blocks(
            session_id,
            "system",
            directive,
            "leader",
            "",
            "[]",
            &directive_blocks,
            0,
            0,
            0,
            0,
        )
        .map_err(|error| format!("failed to persist auto follow-up: {error}"))?;
    Ok(())
}

pub fn budget_stop_reason(
    timebox_reached: bool,
    turn: u32,
    max_turns: u32,
    stalled_turns: u32,
) -> Option<&'static str> {
    if timebox_reached {
        return Some(STOP_REASON_TIMEBOX);
    }
    if stalled_turns >= 2 {
        return Some(STOP_REASON_STALL);
    }
    if turn >= max_turns {
        return Some(STOP_REASON_MAX_TURNS);
    }
    None
}

fn is_quota_or_agent_error(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("quota will reset")
        || lower.contains("quota exceeded")
        || lower.contains("resource_exhausted")
        || lower.contains("resource exhausted")
        || lower.contains("rate limit")
        || lower.contains("agent error")
        || lower.contains("unknown agent message")
}

fn is_ralph_template_body(text: &str) -> bool {
    text.contains("[RALPH LOOP") || text.contains("RALPH LOOP - 자율 실행 모드")
}

fn is_unreachable_style_failure(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("timed out")
        || lower.contains("unreachable")
        || lower.contains("connection refused")
}

#[cfg(test)]
mod tests {
    use super::*;
    use brain_session_rs::session_store::SessionStore;
    use tempfile::TempDir;

    fn korean_next_step() -> &'static str {
        "첨부 원문을 근거로 회신 초안 문단을 작성하세요."
    }

    fn quota_agent_error() -> &'static str {
        "Agent Error, unknown agent message: quota will reset at midnight UTC"
    }

    #[test]
    fn korean_next_step_is_accepted_and_persistable() {
        let outcome = classify_user_agent_text(korean_next_step());
        let decision = decide_auto_loop_action(outcome, 0, "progress so far");
        assert_eq!(
            followup_text_to_persist(&decision),
            Some(korean_next_step())
        );
        assert!(matches!(decision, AutoLoopDecision::Accept { .. }));
    }

    #[test]
    fn quota_agent_error_text_is_rejected_and_not_persistable() {
        let outcome = classify_user_agent_text(quota_agent_error());
        let decision = decide_auto_loop_action(outcome, 0, "progress so far");
        assert!(followup_text_to_persist(&decision).is_none());
        match decision {
            AutoLoopDecision::RecoverOnce {
                kind,
                detail,
                prompt,
            } => {
                assert_eq!(kind, AutoUnusableKind::QuotaOrAgentError);
                assert!(detail.contains("quota will reset"));
                assert!(!detail.is_empty());
                assert!(prompt.contains("[RALPH LOOP"));
                assert!(!prompt.contains("quota will reset"));
            }
            other => panic!("expected RecoverOnce, got {other:?}"),
        }
    }

    #[test]
    fn empty_string_is_rejected_and_not_persistable() {
        let outcome = classify_user_agent_text("   \n\t  ");
        assert!(matches!(
            outcome,
            ClassifiedUserAgentOutcome::Unusable {
                kind: AutoUnusableKind::Empty,
                ..
            }
        ));
        let decision = decide_auto_loop_action(outcome, 0, "progress");
        assert!(followup_text_to_persist(&decision).is_none());
    }

    #[test]
    fn unreachable_timeout_error_is_rejected_and_not_persistable() {
        let outcome = classify_user_agent_error("User Agent timed out after 5s");
        assert!(matches!(
            outcome,
            ClassifiedUserAgentOutcome::Unusable {
                kind: AutoUnusableKind::Unreachable,
                ..
            }
        ));
        let decision = decide_auto_loop_action(outcome, 0, "progress");
        assert!(followup_text_to_persist(&decision).is_none());
        assert!(matches!(decision, AutoLoopDecision::RecoverOnce { .. }));
    }

    #[test]
    fn unreachable_connection_error_is_rejected() {
        let outcome = classify_user_agent_error(
            "User Agent unreachable (http://localhost:4325): connection refused",
        );
        assert!(matches!(
            outcome,
            ClassifiedUserAgentOutcome::Unusable {
                kind: AutoUnusableKind::Unreachable,
                detail,
            } if detail.contains("unreachable")
        ));
    }

    #[test]
    fn quota_error_string_classifies_as_quota_not_unreachable() {
        let outcome = classify_user_agent_error(&format!(
            "unusable user-agent directive (QuotaOrAgentError): {}",
            quota_agent_error()
        ));
        assert!(matches!(
            outcome,
            ClassifiedUserAgentOutcome::Unusable {
                kind: AutoUnusableKind::QuotaOrAgentError,
                ..
            }
        ));
    }

    #[test]
    fn complete_phrase_is_not_a_followup_row() {
        let outcome = classify_user_agent_text("모든 작업이 완료되었습니다");
        let decision = decide_auto_loop_action(outcome, 0, "progress");
        assert_eq!(decision, AutoLoopDecision::Complete);
        assert!(followup_text_to_persist(&decision).is_none());
    }

    #[test]
    fn persist_valid_followup_is_readable_from_fresh_store() {
        let tmp = TempDir::new().expect("tmpdir");
        let data_dir = tmp.path().join("ilhae");
        let session_id = "auto-followup-1";
        let directive = korean_next_step();

        {
            let store = SessionStore::new(&data_dir).expect("store");
            store
                .create_session(session_id, "auto follow-up", "leader", "/")
                .expect("create session");
            persist_autonomous_followup(&store, session_id, directive)
                .expect("persist should succeed");
        }

        let fresh = SessionStore::new(&data_dir).expect("fresh store");
        let messages = fresh
            .load_session_messages(session_id)
            .expect("load messages");
        let followup = messages.iter().find(|message| {
            message.role == "system" && message.agent_id == "leader" && message.content == directive
        });
        assert!(
            followup.is_some(),
            "fresh store should still have the system/leader follow-up row, got {messages:?}"
        );
    }

    #[test]
    fn persist_error_surfaces_when_session_is_missing() {
        let tmp = TempDir::new().expect("tmpdir");
        let data_dir = tmp.path().join("ilhae");
        let store = SessionStore::new(&data_dir).expect("store");
        let error = persist_autonomous_followup(&store, "missing-session", korean_next_step())
            .expect_err("missing session must fail the write");
        assert!(
            error.contains("not found"),
            "persist error should mention the missing session, got {error}"
        );
    }

    #[test]
    fn second_consecutive_unusable_stops_without_persisting_ralph_or_error() {
        let mut gate = AutoDirectiveGate::default();
        let mut persisted = Vec::new();
        let progress = "same progress";
        let outcomes = [
            classify_user_agent_text(quota_agent_error()),
            classify_user_agent_text(""),
            classify_user_agent_text(quota_agent_error()),
        ];

        let mut last_decision = None;
        for outcome in outcomes {
            let decision = gate.step(outcome, progress);
            if let Some(text) = followup_text_to_persist(&decision) {
                persisted.push(text.to_string());
            }
            let should_stop = matches!(decision, AutoLoopDecision::Stop { .. });
            last_decision = Some(decision);
            if should_stop {
                break;
            }
        }

        match last_decision {
            Some(AutoLoopDecision::Stop { stop_reason }) => {
                assert_eq!(stop_reason, STOP_REASON_USER_AGENT_FAILURE);
            }
            other => panic!("expected terminal user-agent failure, got {other:?}"),
        }
        assert!(
            persisted.is_empty(),
            "unusable quota/empty/Ralph bodies must not be stored as follow-up rows, got {persisted:?}"
        );
        assert_eq!(gate.consecutive_unusable, 2);
    }

    #[test]
    fn identical_ralph_template_body_is_unusable() {
        let ralph = build_ralph_loop_prompt("진행 요약");
        let outcome = classify_user_agent_text(&ralph);
        assert!(matches!(
            outcome,
            ClassifiedUserAgentOutcome::Unusable {
                kind: AutoUnusableKind::RalphTemplate,
                ..
            }
        ));
        let first = decide_auto_loop_action(outcome.clone(), 0, "진행 요약");
        assert!(followup_text_to_persist(&first).is_none());
        let second = decide_auto_loop_action(outcome, 1, "진행 요약");
        assert_eq!(
            second,
            AutoLoopDecision::Stop {
                stop_reason: STOP_REASON_USER_AGENT_FAILURE.to_string(),
            }
        );
    }

    #[test]
    fn accepted_directive_resets_consecutive_unusable() {
        let mut gate = AutoDirectiveGate::default();
        let _ = gate.step(classify_user_agent_text(""), "progress");
        assert_eq!(gate.consecutive_unusable, 1);
        let decision = gate.step(classify_user_agent_text(korean_next_step()), "progress");
        assert!(matches!(decision, AutoLoopDecision::Accept { .. }));
        assert_eq!(gate.consecutive_unusable, 0);
    }

    #[test]
    fn budget_stops_do_not_require_a_live_llm() {
        assert_eq!(
            budget_stop_reason(true, 1, 10, 0),
            Some(STOP_REASON_TIMEBOX)
        );
        assert_eq!(
            budget_stop_reason(false, 10, 10, 0),
            Some(STOP_REASON_MAX_TURNS)
        );
        assert_eq!(budget_stop_reason(false, 3, 10, 2), Some(STOP_REASON_STALL));
        assert_eq!(budget_stop_reason(false, 3, 10, 1), None);
    }
}
