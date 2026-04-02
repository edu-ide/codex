use crate::settings_store::Settings;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    SoloInteractive,
    SoloAutonomous,
    TeamInteractive,
    TeamAutonomous,
    TeamMock,
}

impl ExecutionMode {
    pub fn is_team(self) -> bool {
        matches!(
            self,
            Self::TeamInteractive | Self::TeamAutonomous | Self::TeamMock
        )
    }

    pub fn is_autonomous(self) -> bool {
        matches!(self, Self::SoloAutonomous | Self::TeamAutonomous)
    }

    pub fn is_mock(self) -> bool {
        matches!(self, Self::TeamMock)
    }
}

pub fn decide_execution_mode(settings: &Settings) -> ExecutionMode {
    let mock_mode_enabled = settings.agent.mock_mode
        || std::env::var("ILHAE_MOCK")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

    match (
        settings.agent.team_mode,
        settings.agent.autonomous_mode,
        mock_mode_enabled,
    ) {
        (true, _, true) => ExecutionMode::TeamMock,
        (true, true, false) => ExecutionMode::TeamAutonomous,
        (true, false, false) => ExecutionMode::TeamInteractive,
        (false, true, false) => ExecutionMode::SoloAutonomous,
        (false, false, false) | (false, _, true) => ExecutionMode::SoloInteractive,
    }
}
