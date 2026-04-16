use codex_protocol::commands::{CommandCategory, CommandMeta};
use strum::IntoEnumIterator;
use strum_macros::AsRefStr;
use strum_macros::EnumIter;
use strum_macros::EnumString;
use strum_macros::IntoStaticStr;

/// Commands that can be invoked by starting a message with a leading slash.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, EnumString, EnumIter, AsRefStr, IntoStaticStr,
)]
#[strum(serialize_all = "kebab-case")]
pub enum SlashCommand {
    // DO NOT ALPHA-SORT! Enum order is presentation order in the popup, so
    // more frequently used commands should be listed first.
    Model,
    Fast,
    Profile,
    Advisor,
    Auto,
    Team,
    Kairos,
    #[strum(serialize = "dream-bg")]
    BgDream,
    Dream,
    Embed,
    Improve,
    Tmux,
    Worktree,
    Remote,
    Approvals,
    Permissions,
    #[strum(serialize = "setup-default-sandbox")]
    ElevateSandbox,
    #[strum(serialize = "sandbox-add-read-dir")]
    SandboxReadRoot,
    Experimental,
    Skills,
    Review,
    Rename,
    New,
    Resume,
    Fork,
    Init,
    Compact,
    Plan,
    Collab,
    Agent,
    // Undo,
    Copy,
    Diff,
    Mention,
    Help,
    Status,
    DebugConfig,
    Title,
    Statusline,
    Theme,
    Mcp,
    Apps,
    Plugins,
    Logout,
    Quit,
    Exit,
    Feedback,
    Rollout,
    Ps,
    #[strum(to_string = "stop", serialize = "clean")]
    Stop,
    Clear,
    Personality,
    Realtime,
    Settings,
    TestApproval,
    #[strum(serialize = "subagents")]
    MultiAgents,
    // Debugging commands.
    #[strum(serialize = "debug-m-drop")]
    MemoryDrop,
    #[strum(serialize = "debug-m-update")]
    MemoryUpdate,
}

impl SlashCommand {
    /// User-visible description shown in the popup.
    pub fn description(self) -> &'static str {
        match self {
            SlashCommand::Feedback => "send logs to maintainers",
            SlashCommand::New => "start a new chat during a conversation",
            SlashCommand::Init => "create an AGENTS.md file with instructions for Codex",
            SlashCommand::Compact => "summarize conversation to prevent hitting the context limit",
            SlashCommand::Review => "review my current changes and find issues",
            SlashCommand::Rename => "rename the current thread",
            SlashCommand::Resume => "resume a saved chat",
            SlashCommand::Clear => "clear the terminal and start a new chat",
            SlashCommand::Fork => "fork the current chat",
            // SlashCommand::Undo => "ask Codex to undo a turn",
            SlashCommand::Quit | SlashCommand::Exit => "exit Codex",
            SlashCommand::Copy => "copy last response as markdown",
            SlashCommand::Diff => "show git diff (including untracked files)",
            SlashCommand::Mention => "mention a file",
            SlashCommand::Help => "show available slash commands",
            SlashCommand::Skills => "use skills to improve how Codex performs specific tasks",
            SlashCommand::Status => "show current session configuration and token usage",
            SlashCommand::DebugConfig => "show config layers and requirement sources for debugging",
            SlashCommand::Title => "configure which items appear in the terminal title",
            SlashCommand::Statusline => "configure which items appear in the status line",
            SlashCommand::Theme => "choose a syntax highlighting theme",
            SlashCommand::Ps => "list background terminals",
            SlashCommand::Stop => "stop all background terminals",
            SlashCommand::MemoryDrop => "DO NOT USE",
            SlashCommand::MemoryUpdate => "DO NOT USE",
            SlashCommand::Model => "choose what model and reasoning effort to use",
            SlashCommand::Fast => "toggle Fast mode to enable fastest inference at 2X plan usage",
            SlashCommand::Profile => "choose the active profile",
            SlashCommand::Advisor => "cycle the advisor preset",
            SlashCommand::Auto => "toggle autonomous mode",
            SlashCommand::Team => "toggle team mode",
            SlashCommand::Kairos => "toggle kairos scheduling",
            SlashCommand::Dream => "toggle background dream mode (unconscious memory hygiene)",
            SlashCommand::Embed => "toggle semantic embedding indexing (Right Brain)",
            SlashCommand::Improve => "toggle self-improvement mode",
            SlashCommand::Tmux => "show tmux workflow guidance",
            SlashCommand::Worktree => "show git worktree workflow guidance",
            SlashCommand::Remote => "show remote-control workflow guidance",
            SlashCommand::Personality => "choose a communication style for Codex",
            SlashCommand::Realtime => "toggle realtime voice mode (experimental)",
            SlashCommand::Settings => "configure realtime microphone/speaker",
            SlashCommand::Plan => "switch to Plan mode",
            SlashCommand::Collab => "change collaboration mode (experimental)",
            SlashCommand::Agent | SlashCommand::MultiAgents => "switch the active agent thread",
            SlashCommand::Approvals => "choose what Codex is allowed to do",
            SlashCommand::Permissions => "choose what Codex is allowed to do",
            SlashCommand::ElevateSandbox => "set up elevated agent sandbox",
            SlashCommand::SandboxReadRoot => {
                "let sandbox read a directory: /sandbox-add-read-dir <absolute_path>"
            }
            SlashCommand::Experimental => "toggle experimental features",
            SlashCommand::Mcp => "list configured MCP tools",
            SlashCommand::Apps => "manage apps",
            SlashCommand::Plugins => "browse plugins",
            SlashCommand::Logout => "log out of Codex",
            SlashCommand::Rollout => "print the rollout file path",
            SlashCommand::TestApproval => "test approval request",
            SlashCommand::BgDream => "",
        }
    }

    pub fn to_meta(self) -> CommandMeta {
        let name = self.command().to_string();
        let help_text = self.description().to_string();

        CommandMeta {
            name,
            help_text,
            usage_example: None,
            is_experimental: matches!(
                self,
                SlashCommand::Realtime | SlashCommand::Collab | SlashCommand::Plan
            ),
            is_visible: self.is_visible_internal(),
            available_during_task: self.available_during_task(),
            category: match self {
            SlashCommand::Model | SlashCommand::Fast | SlashCommand::Personality => {
                CommandCategory::System
            }
            SlashCommand::Profile
            | SlashCommand::Advisor
            | SlashCommand::Auto
            | SlashCommand::Team
            | SlashCommand::Kairos
            | SlashCommand::Dream
            | SlashCommand::Embed
            | SlashCommand::Improve
            | SlashCommand::Tmux
            | SlashCommand::Worktree
            | SlashCommand::Remote => CommandCategory::System,
            SlashCommand::Mcp | SlashCommand::Apps | SlashCommand::Plugins => {
                CommandCategory::Mcp
            }
                SlashCommand::Review | SlashCommand::Plan => CommandCategory::Experimental,
                _ => CommandCategory::System,
            },
            tags: None,
            linked_files: None,
            version: None,
            compatibility: None,
        }
    }

    /// Command string without the leading '/'. Provided for compatibility with
    /// existing code that expects a method named `command()`.
    pub fn command(self) -> &'static str {
        self.into()
    }

    /// Whether this command supports inline args (for example `/review ...`).
    pub fn supports_inline_args(self) -> bool {
        matches!(
            self,
            SlashCommand::Review
                | SlashCommand::Rename
                | SlashCommand::Plan
                | SlashCommand::Fast
                | SlashCommand::Resume
                | SlashCommand::SandboxReadRoot
        )
    }

    /// Whether this command can be run while a task is in progress.
    pub fn available_during_task(self) -> bool {
        match self {
            SlashCommand::New
            | SlashCommand::Resume
            | SlashCommand::Fork
            | SlashCommand::Init
            | SlashCommand::Compact
            // | SlashCommand::Undo
            | SlashCommand::Model
            | SlashCommand::Fast
            | SlashCommand::Profile
            | SlashCommand::Advisor
            | SlashCommand::Auto
            | SlashCommand::Team
            | SlashCommand::Kairos
            | SlashCommand::Dream
            | SlashCommand::Embed
            | SlashCommand::Improve
            | SlashCommand::Personality
            | SlashCommand::Approvals
            | SlashCommand::Permissions
            | SlashCommand::ElevateSandbox
            | SlashCommand::SandboxReadRoot
            | SlashCommand::Experimental
            | SlashCommand::Review
            | SlashCommand::Plan
            | SlashCommand::Clear
            | SlashCommand::Logout
            | SlashCommand::MemoryDrop
            | SlashCommand::MemoryUpdate => false,
            SlashCommand::Diff
            | SlashCommand::Copy
            | SlashCommand::Rename
            | SlashCommand::Mention
            | SlashCommand::Help
            | SlashCommand::Skills
            | SlashCommand::Status
            | SlashCommand::DebugConfig
            | SlashCommand::Tmux
            | SlashCommand::Worktree
            | SlashCommand::Remote
            | SlashCommand::Ps
            | SlashCommand::Stop
            | SlashCommand::Mcp
            | SlashCommand::Apps
            | SlashCommand::Plugins
            | SlashCommand::Feedback
            | SlashCommand::Quit
            | SlashCommand::Exit => true,
            SlashCommand::Rollout => true,
            SlashCommand::TestApproval => true,
            SlashCommand::Realtime => true,
            SlashCommand::Settings => true,
            SlashCommand::Collab => true,
            SlashCommand::Agent | SlashCommand::MultiAgents => true,
            SlashCommand::Statusline => false,
            SlashCommand::Theme => false,
            SlashCommand::Title => false,
            SlashCommand::BgDream => false,
        }
    }

    pub fn is_visible(self) -> bool {
        self.is_visible_internal()
    }

    fn is_visible_internal(self) -> bool {
        match self {
            SlashCommand::SandboxReadRoot => cfg!(target_os = "windows"),
            SlashCommand::Copy => !cfg!(target_os = "android"),
            SlashCommand::Rollout | SlashCommand::TestApproval => cfg!(debug_assertions),
            _ => true,
        }
    }
}

/// Return all built-in commands in a Vec paired with their command string.
pub fn built_in_slash_commands() -> Vec<(&'static str, SlashCommand)> {
    SlashCommand::iter()
        .filter(|command| command.is_visible())
        .map(|c| (c.command(), c))
        .collect()
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use std::str::FromStr;

    use super::SlashCommand;

    #[test]
    fn stop_command_is_canonical_name() {
        assert_eq!(SlashCommand::Stop.command(), "stop");
    }

    #[test]
    fn clean_alias_parses_to_stop_command() {
        assert_eq!(SlashCommand::from_str("clean"), Ok(SlashCommand::Stop));
    }

    #[test]
    fn runtime_mode_commands_are_system_commands() {
        use codex_protocol::commands::CommandCategory;

        for command in [
            SlashCommand::Profile,
            SlashCommand::Advisor,
            SlashCommand::Auto,
            SlashCommand::Team,
            SlashCommand::Kairos,
            SlashCommand::Improve,
        ] {
            let meta = command.to_meta();
            assert_eq!(meta.category, CommandCategory::System);
            assert!(!meta.available_during_task);
        }
    }

    #[test]
    fn workflow_surface_commands_are_system_commands() {
        use codex_protocol::commands::CommandCategory;

        for command in [
            SlashCommand::Tmux,
            SlashCommand::Worktree,
            SlashCommand::Remote,
        ] {
            let meta = command.to_meta();
            assert_eq!(meta.category, CommandCategory::System);
            assert!(meta.available_during_task);
        }
    }

    #[test]
    fn runtime_mode_descriptions_match_trigger_semantics() {
        let cases = [
            (SlashCommand::Profile, "choose the active profile"),
            (SlashCommand::Advisor, "cycle the advisor preset"),
            (SlashCommand::Auto, "toggle autonomous mode"),
            (SlashCommand::Team, "toggle team mode"),
            (SlashCommand::Kairos, "toggle kairos scheduling"),
            (SlashCommand::Improve, "toggle self-improvement mode"),
            (SlashCommand::Tmux, "show tmux workflow guidance"),
            (
                SlashCommand::Worktree,
                "show git worktree workflow guidance",
            ),
            (
                SlashCommand::Remote,
                "show remote-control workflow guidance",
            ),
        ];

        for (command, expected) in cases {
            assert_eq!(command.description(), expected);
        }
    }
}
