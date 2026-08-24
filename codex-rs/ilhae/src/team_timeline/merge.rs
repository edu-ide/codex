use brain_session_rs::session_store::SessionStore;
use serde_json::json;

use crate::context_proxy::autonomy::AutoUnusableKind;
use crate::context_proxy::autonomy::ClassifiedUserAgentOutcome;
use crate::context_proxy::autonomy::classify_user_agent_text;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamWorkerContribution {
    pub role: String,
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamMergePolicyKind {
    AppendAll,
    LeaderOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeableContribution {
    pub role: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedContribution {
    pub role: String,
    pub kind: AutoUnusableKind,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamMergeDecision {
    pub user_facing_role: String,
    pub user_facing_text: String,
    pub included: Vec<MergeableContribution>,
    pub rejected: Vec<RejectedContribution>,
    pub conflict: bool,
}

pub fn parse_team_merge_policy(policy: &str) -> TeamMergePolicyKind {
    match policy.trim().to_ascii_lowercase().as_str() {
        "leader_only" => TeamMergePolicyKind::LeaderOnly,
        _ => TeamMergePolicyKind::AppendAll,
    }
}

pub fn worker_body_mergeable_text(body: &str) -> Result<String, AutoUnusableKind> {
    match classify_user_agent_text(body) {
        ClassifiedUserAgentOutcome::Continue(text) => Ok(text),
        ClassifiedUserAgentOutcome::Complete => Ok(body.trim().to_string()),
        ClassifiedUserAgentOutcome::Unusable { kind, .. } => Err(kind),
    }
}

pub fn merge_team_contributions(
    policy: &str,
    contributions: &[TeamWorkerContribution],
    leader_role: &str,
) -> Option<TeamMergeDecision> {
    let leader_role = normalize_role(leader_role);
    let mut included = Vec::new();
    let mut rejected = Vec::new();
    for contribution in contributions {
        let role = normalize_role(&contribution.role);
        match worker_body_mergeable_text(&contribution.body) {
            Ok(body) => included.push(MergeableContribution { role, body }),
            Err(kind) => rejected.push(RejectedContribution {
                role,
                kind,
                detail: contribution.body.trim().to_string(),
            }),
        }
    }
    if included.is_empty() {
        return None;
    }

    let conflict = contributions_conflict(&included);
    let (user_facing_role, user_facing_text) = match parse_team_merge_policy(policy) {
        TeamMergePolicyKind::AppendAll => {
            let role = if included.iter().all(|item| item.role == included[0].role) {
                included[0].role.clone()
            } else {
                "team".to_string()
            };
            (role, format_append_all(&included))
        }
        TeamMergePolicyKind::LeaderOnly => (
            leader_role.clone(),
            format_leader_only(&included, &leader_role, conflict),
        ),
    };

    Some(TeamMergeDecision {
        user_facing_role,
        user_facing_text,
        included,
        rejected,
        conflict,
    })
}

pub fn persistable_team_merge_text(decision: &TeamMergeDecision) -> &str {
    &decision.user_facing_text
}

pub fn persist_team_merge_followup(
    store: &SessionStore,
    session_id: &str,
    decision: &TeamMergeDecision,
) -> Result<(), String> {
    match store.get_session(session_id) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return Err(format!(
                "failed to persist team merge follow-up: session `{session_id}` not found"
            ));
        }
        Err(error) => {
            return Err(format!("failed to persist team merge follow-up: {error}"));
        }
    }

    let directive_blocks = serde_json::to_string(&vec![json!({
        "type": "text",
        "text": decision.user_facing_text
    })])
    .unwrap_or_else(|_| "[]".to_string());

    store
        .add_full_message_with_blocks_channel(
            session_id,
            "assistant",
            &decision.user_facing_text,
            &decision.user_facing_role,
            "",
            "[]",
            &directive_blocks,
            "team-merge",
            0,
            0,
            0,
            0,
        )
        .map_err(|error| format!("failed to persist team merge follow-up: {error}"))?;
    Ok(())
}

fn normalize_role(role: &str) -> String {
    let trimmed = role.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        "worker".to_string()
    } else {
        trimmed
    }
}

fn format_append_all(included: &[MergeableContribution]) -> String {
    included
        .iter()
        .map(|item| format!("[{}]\n{}", item.role, item.body))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn format_leader_only(
    included: &[MergeableContribution],
    leader_role: &str,
    conflict: bool,
) -> String {
    if included.len() == 1 && included[0].role == leader_role {
        return included[0].body.clone();
    }
    let header = if conflict {
        "Leader arbitration: conflicting specialist conclusions; both are attributed below and neither is treated as unlabeled consensus."
    } else if included.len() == 1 {
        "Leader arbitration from a specialist; the raw worker body is not emitted as the user-facing role."
    } else {
        "Leader arbitration: specialist conclusions attributed below."
    };
    let mut lines = vec![header.to_string()];
    for item in included {
        lines.push(format!("[{}] {}", item.role, compact_line(&item.body)));
    }
    lines.push("- Full worker output is preserved in the team timeline and audit log.".to_string());
    lines.join("\n")
}

fn compact_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(180).collect::<String>())
        .unwrap_or_default()
}

fn contributions_conflict(included: &[MergeableContribution]) -> bool {
    for (index, left) in included.iter().enumerate() {
        for right in included.iter().skip(index + 1) {
            if response_similarity_score(&left.body, &right.body) < 35 {
                return true;
            }
        }
    }
    false
}

fn response_similarity_score(left: &str, right: &str) -> i32 {
    let left_tokens = tokens(left);
    let right_tokens = tokens(right);
    if left_tokens.is_empty() || right_tokens.is_empty() {
        return 0;
    }
    let overlap = left_tokens
        .iter()
        .filter(|token| right_tokens.contains(token))
        .count() as i32;
    let baseline = left_tokens.len().min(right_tokens.len()) as i32;
    overlap * 100 / baseline.max(1)
}

fn tokens(text: &str) -> Vec<String> {
    text.split(|ch: char| !ch.is_alphanumeric() && ch != '_' && ch != '-')
        .map(|token| token.trim().to_ascii_lowercase())
        .filter(|token| token.chars().count() >= 2)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use brain_session_rs::session_store::SessionStore;
    use tempfile::TempDir;

    fn researcher_body() -> &'static str {
        "첨부 원문을 근거로 회신 초안 문단을 작성하세요."
    }

    fn verifier_body() -> &'static str {
        "견적서 부가세 누락을 먼저 수정한 뒤 재발행하세요."
    }

    fn quota_body() -> &'static str {
        "Agent Error, unknown agent message: quota will reset at midnight UTC"
    }

    fn two_valid() -> Vec<TeamWorkerContribution> {
        vec![
            TeamWorkerContribution {
                role: "researcher".to_string(),
                body: researcher_body().to_string(),
            },
            TeamWorkerContribution {
                role: "verifier".to_string(),
                body: verifier_body().to_string(),
            },
        ]
    }

    #[test]
    fn korean_specialist_body_is_mergeable() {
        let decision = merge_team_contributions(
            "append_all",
            &[TeamWorkerContribution {
                role: "researcher".to_string(),
                body: researcher_body().to_string(),
            }],
            "leader",
        )
        .expect("mergeable");
        assert!(persistable_team_merge_text(&decision).contains(researcher_body()));
        assert_eq!(decision.user_facing_role, "researcher");
        assert!(decision.rejected.is_empty());
    }

    #[test]
    fn quota_agent_error_is_not_a_parent_answer() {
        let decision = merge_team_contributions(
            "append_all",
            &[TeamWorkerContribution {
                role: "researcher".to_string(),
                body: quota_body().to_string(),
            }],
            "leader",
        );
        assert!(
            decision.is_none(),
            "quota text must not become a persistable parent-session answer"
        );
        assert!(matches!(
            worker_body_mergeable_text(quota_body()),
            Err(AutoUnusableKind::QuotaOrAgentError)
        ));
    }

    #[test]
    fn empty_body_is_not_mergeable() {
        assert!(matches!(
            worker_body_mergeable_text("  \n\t"),
            Err(AutoUnusableKind::Empty)
        ));
        assert!(
            merge_team_contributions(
                "append_all",
                &[TeamWorkerContribution {
                    role: "verifier".to_string(),
                    body: "   ".to_string(),
                }],
                "leader",
            )
            .is_none()
        );
    }

    #[test]
    fn unreachable_style_failure_is_not_mergeable() {
        assert!(matches!(
            worker_body_mergeable_text("User Agent timed out after 5s"),
            Err(AutoUnusableKind::Unreachable)
        ));
    }

    #[test]
    fn mixed_valid_and_quota_keeps_only_mergeable() {
        let decision = merge_team_contributions(
            "append_all",
            &[
                TeamWorkerContribution {
                    role: "researcher".to_string(),
                    body: researcher_body().to_string(),
                },
                TeamWorkerContribution {
                    role: "verifier".to_string(),
                    body: quota_body().to_string(),
                },
            ],
            "leader",
        )
        .expect("valid researcher remains");
        assert!(persistable_team_merge_text(&decision).contains(researcher_body()));
        assert!(
            !persistable_team_merge_text(&decision).contains("quota will reset"),
            "rejected quota must not appear in the parent-session answer"
        );
        assert_eq!(decision.rejected.len(), 1);
        assert_eq!(
            decision.rejected[0].kind,
            AutoUnusableKind::QuotaOrAgentError
        );
    }

    #[test]
    fn persist_two_role_merge_is_readable_from_fresh_store() {
        let tmp = TempDir::new().expect("tmpdir");
        let data_dir = tmp.path().join("ilhae");
        let session_id = "team-merge-parent";
        let decision =
            merge_team_contributions("append_all", &two_valid(), "leader").expect("merge");

        {
            let store = SessionStore::new(&data_dir).expect("store");
            store
                .create_session(session_id, "team merge", "leader", "/")
                .expect("create");
            persist_team_merge_followup(&store, session_id, &decision).expect("persist");
        }

        let fresh = SessionStore::new(&data_dir).expect("fresh");
        let messages = fresh.load_session_messages(session_id).expect("load");
        let followup = messages.iter().find(|message| {
            message.role == "assistant"
                && message.content.contains("[researcher]")
                && message.content.contains(researcher_body())
                && message.content.contains("[verifier]")
                && message.content.contains(verifier_body())
        });
        assert!(
            followup.is_some(),
            "fresh store missing role-attributed parent follow-up, got {messages:?}"
        );
    }

    #[test]
    fn conflicting_bodies_are_not_collapsed_into_one_unlabeled_consensus() {
        let decision =
            merge_team_contributions("append_all", &two_valid(), "leader").expect("merge");
        assert!(decision.conflict);
        let text = persistable_team_merge_text(&decision);
        assert!(text.contains("[researcher]"));
        assert!(text.contains("[verifier]"));
        assert_ne!(text, researcher_body());
        assert_ne!(text, verifier_body());
        assert!(
            text.contains(researcher_body()) && text.contains(verifier_body()),
            "conflict must keep both attributed bodies, got {text}"
        );
    }

    #[test]
    fn persist_error_surfaces_when_session_is_missing() {
        let tmp = TempDir::new().expect("tmpdir");
        let store = SessionStore::new(&tmp.path().join("ilhae")).expect("store");
        let decision = merge_team_contributions(
            "append_all",
            &[TeamWorkerContribution {
                role: "researcher".to_string(),
                body: researcher_body().to_string(),
            }],
            "leader",
        )
        .expect("merge");
        let error = persist_team_merge_followup(&store, "missing-session", &decision)
            .expect_err("missing session must fail");
        assert!(
            error.contains("not found"),
            "persist error should surface, got {error}"
        );
    }

    #[test]
    fn append_all_includes_both_roles_in_user_facing_payload() {
        let decision =
            merge_team_contributions("append_all", &two_valid(), "leader").expect("merge");
        let text = persistable_team_merge_text(&decision);
        assert!(text.contains("[researcher]"));
        assert!(text.contains("[verifier]"));
        assert_eq!(decision.user_facing_role, "team");
        assert_ne!(decision.user_facing_role, "leader");
    }

    #[test]
    fn leader_only_emits_leader_role_not_raw_worker_role_or_body() {
        let decision =
            merge_team_contributions("leader_only", &two_valid(), "leader").expect("merge");
        assert_eq!(decision.user_facing_role, "leader");
        let text = persistable_team_merge_text(&decision);
        assert_ne!(text, researcher_body());
        assert_ne!(text, verifier_body());
        assert!(text.contains("[researcher]"));
        assert!(text.contains("[verifier]"));
        assert!(text.contains("Leader arbitration"));
        assert!(decision.conflict);
    }

    #[test]
    fn leader_only_single_worker_does_not_use_worker_as_user_facing_role() {
        let decision = merge_team_contributions(
            "leader_only",
            &[TeamWorkerContribution {
                role: "researcher".to_string(),
                body: researcher_body().to_string(),
            }],
            "leader",
        )
        .expect("merge");
        assert_eq!(decision.user_facing_role, "leader");
        assert_ne!(decision.user_facing_text, researcher_body());
        assert!(decision.user_facing_text.contains("[researcher]"));
    }
}
