//! Shared helpers for filtering and matching built-in slash commands.
//!
//! The same sandbox- and feature-gating rules are used by both the composer
//! and the command popup. Centralizing them here keeps those call sites small
//! and ensures they stay in sync.
use std::str::FromStr;

use codex_utils_fuzzy_match::fuzzy_match;
use strum::IntoEnumIterator;

use crate::slash_command::SlashCommand;
use crate::slash_command::built_in_slash_commands;

/// Hide alias commands in popup/listing views so each unique action appears once.
pub(crate) const ALIAS_COMMANDS: &[SlashCommand] = &[SlashCommand::Quit, SlashCommand::Approvals];

/// Runtime-mode slash commands that act as selectors or toggles.
#[cfg(test)]
pub(crate) const RUNTIME_MODE_COMMANDS: &[SlashCommand] = &[
    SlashCommand::Profile,
    SlashCommand::Advisor,
    SlashCommand::Auto,
    SlashCommand::Team,
    SlashCommand::Kairos,
    SlashCommand::Improve,
];

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct BuiltinCommandFlags {
    pub(crate) collaboration_modes_enabled: bool,
    pub(crate) connectors_enabled: bool,
    pub(crate) plugins_command_enabled: bool,
    pub(crate) fast_command_enabled: bool,
    pub(crate) personality_command_enabled: bool,
    pub(crate) realtime_conversation_enabled: bool,
    pub(crate) audio_device_selection_enabled: bool,
    pub(crate) allow_elevate_sandbox: bool,
    pub(crate) fork_command_enabled: bool,
    pub(crate) terminal_commands_enabled: bool,
}

/// Return the built-ins that should be visible/usable for the current input.
pub(crate) fn builtins_for_input(flags: BuiltinCommandFlags) -> Vec<(&'static str, SlashCommand)> {
    built_in_slash_commands()
        .into_iter()
        .filter(|(_, cmd)| flags.allow_elevate_sandbox || *cmd != SlashCommand::ElevateSandbox)
        .filter(|(_, cmd)| {
            flags.collaboration_modes_enabled
                || !matches!(*cmd, SlashCommand::Collab | SlashCommand::Plan)
        })
        .filter(|(_, cmd)| flags.connectors_enabled || *cmd != SlashCommand::Apps)
        .filter(|(_, cmd)| flags.plugins_command_enabled || *cmd != SlashCommand::Plugins)
        .filter(|(_, cmd)| flags.fast_command_enabled || *cmd != SlashCommand::Fast)
        .filter(|(_, cmd)| flags.personality_command_enabled || *cmd != SlashCommand::Personality)
        .filter(|(_, cmd)| flags.realtime_conversation_enabled || *cmd != SlashCommand::Realtime)
        .filter(|(_, cmd)| flags.audio_device_selection_enabled || *cmd != SlashCommand::Settings)
        .filter(|(_, cmd)| flags.fork_command_enabled || *cmd != SlashCommand::Fork)
        .filter(|(_, cmd)| {
            flags.terminal_commands_enabled
                || !matches!(*cmd, SlashCommand::Ps | SlashCommand::Stop)
        })
        .collect()
}

/// Return the built-ins that should appear in slash-command listing UIs.
pub(crate) fn builtins_for_popup(flags: BuiltinCommandFlags) -> Vec<(&'static str, SlashCommand)> {
    builtins_for_input(flags)
        .into_iter()
        .filter(|(name, _)| !name.starts_with("debug"))
        .filter(|(_, cmd)| *cmd != SlashCommand::Apps)
        .filter(|(_, cmd)| !ALIAS_COMMANDS.contains(cmd))
        .collect()
}

/// Find a single built-in command by exact name, after applying the gating rules.
pub(crate) fn find_builtin_command(name: &str, flags: BuiltinCommandFlags) -> Option<SlashCommand> {
    let cmd = SlashCommand::from_str(name).ok()?;
    matches_builtin_visibility(cmd, flags).then_some(cmd)
}

/// Find the most likely built-in command for an unknown input.
pub(crate) fn suggest_builtin_command(
    name: &str,
    flags: BuiltinCommandFlags,
) -> Option<&'static str> {
    const MAX_TYPOSCORE: i32 = 20;
    SlashCommand::iter()
        .map(|command| (command.command(), command))
        .filter(|(_, command)| matches_builtin_visibility(*command, flags))
        .filter_map(|(command_name, _)| {
            fuzzy_match(command_name, name).map(|(_, score)| (score, command_name))
        })
        .filter(|(score, _)| *score <= MAX_TYPOSCORE)
        .min_by_key(|(score, _)| *score)
        .map(|(_, command_name)| command_name)
}

/// Whether any visible built-in fuzzily matches the provided prefix.
pub(crate) fn has_builtin_prefix(name: &str, flags: BuiltinCommandFlags) -> bool {
    SlashCommand::iter()
        .filter(|command| matches_builtin_visibility(*command, flags))
        .any(|command| fuzzy_match(command.command(), name).is_some())
}

fn matches_builtin_visibility(command: SlashCommand, flags: BuiltinCommandFlags) -> bool {
    if !matches_builtin_flags(command, flags) {
        return false;
    }
    command.is_visible()
}

fn matches_builtin_flags(command: SlashCommand, flags: BuiltinCommandFlags) -> bool {
    (flags.allow_elevate_sandbox || command != SlashCommand::ElevateSandbox)
        && (flags.collaboration_modes_enabled
            || !matches!(command, SlashCommand::Collab | SlashCommand::Plan))
        && (flags.connectors_enabled || command != SlashCommand::Apps)
        && (flags.plugins_command_enabled || command != SlashCommand::Plugins)
        && (flags.fast_command_enabled || command != SlashCommand::Fast)
        && (flags.personality_command_enabled || command != SlashCommand::Personality)
        && (flags.realtime_conversation_enabled || command != SlashCommand::Realtime)
        && (flags.audio_device_selection_enabled || command != SlashCommand::Settings)
        && (flags.fork_command_enabled || command != SlashCommand::Fork)
        && (flags.terminal_commands_enabled
            || !matches!(command, SlashCommand::Ps | SlashCommand::Stop))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn all_enabled_flags() -> BuiltinCommandFlags {
        BuiltinCommandFlags {
            collaboration_modes_enabled: true,
            connectors_enabled: true,
            plugins_command_enabled: true,
            fast_command_enabled: true,
            personality_command_enabled: true,
            realtime_conversation_enabled: true,
            audio_device_selection_enabled: true,
            allow_elevate_sandbox: true,
            fork_command_enabled: true,
            terminal_commands_enabled: true,
        }
    }

    #[test]
    fn debug_command_still_resolves_for_dispatch() {
        let cmd = find_builtin_command("debug-config", all_enabled_flags());
        assert_eq!(cmd, Some(SlashCommand::DebugConfig));
    }

    #[test]
    fn clear_command_resolves_for_dispatch() {
        assert_eq!(
            find_builtin_command("clear", all_enabled_flags()),
            Some(SlashCommand::Clear)
        );
    }

    #[test]
    fn stop_command_resolves_for_dispatch() {
        assert_eq!(
            find_builtin_command("stop", all_enabled_flags()),
            Some(SlashCommand::Stop)
        );
    }

    #[test]
    fn clean_command_alias_resolves_for_dispatch() {
        assert_eq!(
            find_builtin_command("clean", all_enabled_flags()),
            Some(SlashCommand::Stop)
        );
    }

    #[test]
    fn fast_command_is_hidden_when_disabled() {
        let mut flags = all_enabled_flags();
        flags.fast_command_enabled = false;
        assert_eq!(find_builtin_command("fast", flags), None);
    }

    #[test]
    fn realtime_command_is_hidden_when_realtime_is_disabled() {
        let mut flags = all_enabled_flags();
        flags.realtime_conversation_enabled = false;
        assert_eq!(find_builtin_command("realtime", flags), None);
    }

    #[test]
    fn settings_command_is_hidden_when_realtime_is_disabled() {
        let mut flags = all_enabled_flags();
        flags.realtime_conversation_enabled = false;
        flags.audio_device_selection_enabled = false;
        assert_eq!(find_builtin_command("settings", flags), None);
    }

    #[test]
    fn settings_command_is_hidden_when_audio_device_selection_is_disabled() {
        let mut flags = all_enabled_flags();
        flags.audio_device_selection_enabled = false;
        assert_eq!(find_builtin_command("settings", flags), None);
    }

    #[test]
    fn help_command_is_available() {
        let cmd = find_builtin_command("help", all_enabled_flags());
        assert_eq!(cmd, Some(SlashCommand::Help));
    }

    #[test]
    fn runtime_mode_commands_resolve_for_dispatch() {
        for command in RUNTIME_MODE_COMMANDS {
            assert_eq!(
                find_builtin_command(command.command(), all_enabled_flags()),
                Some(*command)
            );
        }
    }

    #[test]
    fn popup_commands_include_runtime_mode_commands() {
        let popup_commands = builtins_for_popup(all_enabled_flags());
        let command_names: Vec<&str> = popup_commands.iter().map(|(name, _)| *name).collect();

        for command in RUNTIME_MODE_COMMANDS {
            let name = command.command();
            assert!(
                command_names.contains(&name),
                "expected popup list to include /{name}, got {command_names:?}"
            );
        }
    }

    #[test]
    fn suggests_similar_command() {
        let suggestion = suggest_builtin_command("stauts", all_enabled_flags());
        assert_eq!(suggestion, Some("status"));
    }

    #[test]
    fn has_builtin_prefix_respects_visibility_and_fuzzy_match() {
        assert_eq!(has_builtin_prefix("st", all_enabled_flags()), true);
        assert_eq!(has_builtin_prefix("stauts", all_enabled_flags()), true);
        assert_eq!(has_builtin_prefix("unknown", all_enabled_flags()), false);
        let mut flags = all_enabled_flags();
        flags.fast_command_enabled = false;
        assert_eq!(has_builtin_prefix("fast", flags), false);
    }

    #[test]
    fn popup_commands_hide_aliases_and_debug_commands() {
        let popup_commands = builtins_for_popup(all_enabled_flags());
        let command_names: Vec<&str> = popup_commands.iter().map(|(name, _)| *name).collect();

        assert!(
            !command_names.iter().any(|name| name.starts_with("debug")),
            "expected no /debug* command in popup list, got {command_names:?}"
        );
        assert!(
            !command_names.contains(&"quit") && !command_names.contains(&"approvals"),
            "expected popup list to hide alias commands, got {command_names:?}"
        );
        assert!(command_names.contains(&"help"));
    }

    #[test]
    fn fork_command_is_hidden_when_disabled() {
        let mut flags = all_enabled_flags();
        flags.fork_command_enabled = false;
        assert_eq!(find_builtin_command("fork", flags), None);
    }

    #[test]
    fn terminal_commands_are_hidden_when_disabled() {
        let mut flags = all_enabled_flags();
        flags.terminal_commands_enabled = false;
        assert_eq!(find_builtin_command("ps", flags), None);
        assert_eq!(find_builtin_command("stop", flags), None);
    }
}
