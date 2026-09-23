use super::*;
use pretty_assertions::assert_eq;

#[test]
fn ilhae_cli_binary_name_accepts_windows_exe_suffix() {
    assert!(is_ilhae_cli_binary_name("ilhae"));
    assert!(is_ilhae_cli_binary_name("ilhae.exe"));
    assert!(is_ilhae_cli_binary_name("codex-ilhae.exe"));
    assert!(is_ilhae_cli_binary_name("codex-ilhae-cli.exe"));
    assert!(!is_ilhae_cli_binary_name("codex.exe"));
}

#[test]
fn ilhae_goal_loop_phase_marks_kairos_as_kairos_loop() {
    let phase = thread_goal_loop_phase_from_ilhae_parts(
        "super_loop:kairos:1779027374405",
        "Running Super Loop",
        codex_ilhae::LoopLifecycleKind::SuperLoop,
    );

    assert_eq!(phase, codex_state::ThreadGoalLoopPhase::KairosLoop);
}
