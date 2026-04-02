pub mod runner;
pub mod state;
pub mod user_agent;

pub const AUTONOMOUS_MAX_TURNS: u32 = 10;
pub const USER_AGENT_ENDPOINT: &str = "http://localhost:4325";

pub fn resolve_user_agent_endpoint() -> String {
    std::env::var("ILHAE_USER_AGENT_ENDPOINT").unwrap_or_else(|_| USER_AGENT_ENDPOINT.to_string())
}
/// Ralph Loop template: progress-aware re-injection instead of bare "continue".
/// {progress} is replaced with accumulated output summary from previous turns.
pub const RALPH_LOOP_TEMPLATE: &str = r#"[RALPH LOOP - 자율 실행 모드]

## 지금까지의 진행 상황:
{progress}

## 지시:
- 위 진행 상황을 참고하여 아직 완료되지 않은 작업을 계속하세요.
- 이미 완료된 작업을 반복하지 마세요.
- 모든 작업이 완료되었으면 최종 결과를 출력하세요.
- 다른 팀원에게 위임이 필요하면 tool로 직접 호출하세요.
"#;

pub fn build_ralph_loop_prompt(progress: &str) -> String {
    RALPH_LOOP_TEMPLATE.replace("{progress}", progress)
}
