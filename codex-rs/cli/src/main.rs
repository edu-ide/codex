use clap::Args;
use clap::CommandFactory;
use clap::Parser;
use clap_complete::Shell;
use clap_complete::generate;
use codex_arg0::Arg0DispatchPaths;
use codex_arg0::arg0_dispatch_or_else;
use codex_chatgpt::apply_command::ApplyCommand;
use codex_chatgpt::apply_command::run_apply_command;
use codex_cli::LandlockCommand;
use codex_cli::SeatbeltCommand;
use codex_cli::WindowsCommand;
use codex_cli::read_access_token_from_stdin;
use codex_cli::read_api_key_from_stdin;
use codex_cli::run_login_status;
use codex_cli::run_login_with_access_token;
use codex_cli::run_login_with_api_key;
use codex_cli::run_login_with_chatgpt;
use codex_cli::run_login_with_device_code;
use codex_cli::run_logout;
use codex_cloud_tasks::Cli as CloudTasksCli;
use codex_exec::Cli as ExecCli;
use codex_exec::Command as ExecCommand;
use codex_exec::ReviewArgs;
use codex_execpolicy::ExecPolicyCheckCommand;
use codex_responses_api_proxy::Args as ResponsesApiProxyArgs;
use codex_rollout_trace::REDUCED_STATE_FILE_NAME;
use codex_rollout_trace::replay_bundle;
use codex_state::StateRuntime;
use codex_state::state_db_path;
use codex_tui::AppExitInfo;
use codex_tui::Cli as TuiCli;
use codex_tui::ExitReason;
use codex_tui::UpdateAction;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_cli::CliConfigOverrides;
use codex_utils_cli::resume_command;
use owo_colors::OwoColorize;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::Arc;
use supports_color::Stream;

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod app_cmd;
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod desktop_app;
mod marketplace_cmd;
mod mcp_cmd;
mod plugin_cmd;
#[cfg(not(windows))]
mod wsl_paths;

use crate::mcp_cmd::McpCli;
use crate::plugin_cmd::PluginCli;
use crate::plugin_cmd::PluginSubcommand;

use codex_core::build_models_manager;
use codex_core::config::Config;
use codex_core::config::ConfigBuilder;
use codex_core::config::ConfigOverrides;
use codex_core::config::edit::ConfigEditsBuilder;
use codex_core::config::find_codex_home;
use codex_features::FEATURES;
use codex_features::Stage;
use codex_features::is_known_feature_key;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_login::read_codex_access_token_from_env;
use codex_memories_write::clear_memory_roots_contents;
use codex_models_manager::bundled_models_response;
use codex_models_manager::manager::RefreshStrategy;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::user_input::UserInput;
use codex_terminal_detection::TerminalName;

/// Codex CLI
///
/// If no subcommand is specified, options will be forwarded to the interactive CLI.
#[derive(Debug, Parser)]
#[clap(
    author,
    version,
    // If a sub‑command is given, ignore requirements of the default args.
    subcommand_negates_reqs = true,
    // The executable is sometimes invoked via a platform‑specific name like
    // `codex-x86_64-unknown-linux-musl`, but the help output should always use
    // the generic `codex` command name that users run.
    bin_name = "codex",
    override_usage = "codex [OPTIONS] [PROMPT]\n       codex [OPTIONS] <COMMAND> [ARGS]"
)]
struct MultitoolCli {
    #[clap(flatten)]
    pub config_overrides: CliConfigOverrides,

    #[clap(flatten)]
    pub feature_toggles: FeatureToggles,

    #[clap(flatten)]
    remote: InteractiveRemoteOptions,

    #[clap(flatten)]
    interactive: TuiCli,

    #[clap(subcommand)]
    subcommand: Option<Subcommand>,
}

#[derive(Debug, clap::Subcommand)]
enum Subcommand {
    /// Run Codex non-interactively.
    #[clap(visible_alias = "e")]
    Exec(ExecCli),

    /// Run a code review non-interactively.
    Review(ReviewArgs),

    /// Manage login.
    Login(LoginCommand),

    /// Remove stored authentication credentials.
    Logout(LogoutCommand),

    /// Manage external MCP servers for Codex.
    Mcp(McpCli),

    /// Manage Codex plugins.
    Plugin(PluginCli),

    /// Start Codex as an MCP server (stdio).
    McpServer,

    /// [experimental] Run the app server or related tooling.
    AppServer(AppServerCommand),

    /// Launch the Ilhae proxy server (Telegram, Discord, UI daemon) natively.
    #[cfg(feature = "ilhae")]
    #[clap(name = "proxy", visible_aliases = ["desktop", "ilhae-proxy"])]
    Proxy,

    /// Stop the native Ilhae model server (e.g. llama-server).
    #[cfg(feature = "ilhae")]
    Stop,

    /// Manage Ilhae runtime profiles.
    #[cfg(feature = "ilhae")]
    Profile(ProfileCommand),

    /// Manage Ilhae identity authentication.
    #[cfg(feature = "ilhae")]
    Auth(IlhaeAuthCommand),

    /// Manage the local model server.
    #[cfg(feature = "ilhae")]
    #[clap(subcommand)]
    LocalServer(LocalServerCommand),

    /// Start the local model server and the agent.
    #[cfg(feature = "ilhae")]
    Start,

    /// Manage the local GPU queue daemon.
    #[cfg(feature = "ilhae")]
    Gpu(GpuCommand),

    /// Launch the Codex desktop app (opens the app installer if missing).
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    App(app_cmd::AppCommand),

    /// Generate shell completion scripts.
    Completion(CompletionCommand),

    /// Update Codex to the latest version.
    Update,

    /// Run commands within a Codex-provided sandbox.
    Sandbox(SandboxArgs),

    /// Debugging tools.
    Debug(DebugCommand),

    /// Execpolicy tooling.
    #[clap(hide = true)]
    Execpolicy(ExecpolicyCommand),

    /// Apply the latest diff produced by Codex agent as a `git apply` to your local working tree.
    #[clap(visible_alias = "a")]
    Apply(ApplyCommand),

    /// Resume a previous interactive session (picker by default; use --last to continue the most recent).
    Resume(ResumeCommand),

    /// Fork a previous interactive session (picker by default; use --last to fork the most recent).
    Fork(ForkCommand),

    /// [EXPERIMENTAL] Browse tasks from Codex Cloud and apply changes locally.
    #[clap(name = "cloud", alias = "cloud-tasks")]
    Cloud(CloudTasksCli),

    /// Internal: run the responses API proxy.
    #[clap(hide = true)]
    ResponsesApiProxy(ResponsesApiProxyArgs),

    /// Internal: relay stdio to a Unix domain socket.
    #[clap(hide = true, name = "stdio-to-uds")]
    StdioToUds(StdioToUdsCommand),

    /// Internal tooling.
    #[clap(hide = true)]
    Internal(InternalArgs),

    /// [EXPERIMENTAL] Run the standalone exec-server service.
    ExecServer(ExecServerCommand),

    /// Inspect feature flags.
    Features(FeaturesCli),
}

#[derive(Debug, Parser)]
struct CompletionCommand {
    /// Shell to generate completions for
    #[clap(value_enum, default_value_t = Shell::Bash)]
    shell: Shell,
}

#[derive(Debug, Parser)]
struct DebugCommand {
    #[command(subcommand)]
    subcommand: DebugSubcommand,
}

#[derive(Debug, clap::Subcommand)]
enum DebugSubcommand {
    /// Render the raw model catalog as JSON.
    Models(DebugModelsCommand),

    /// Tooling: helps debug the app server.
    AppServer(DebugAppServerCommand),

    /// Render the model-visible prompt input list as JSON.
    PromptInput(DebugPromptInputCommand),

    /// Replay a rollout trace bundle and write reduced state JSON.
    #[clap(hide = true)]
    TraceReduce(DebugTraceReduceCommand),

    /// Internal: reset local memory state for a fresh start.
    #[clap(hide = true)]
    ClearMemories,
}

#[derive(Debug, Parser)]
struct DebugAppServerCommand {
    #[command(subcommand)]
    subcommand: DebugAppServerSubcommand,
}

#[derive(Debug, clap::Subcommand)]
enum DebugAppServerSubcommand {
    // Send message to app server V2.
    SendMessageV2(DebugAppServerSendMessageV2Command),
}

#[derive(Debug, Parser)]
struct DebugAppServerSendMessageV2Command {
    #[arg(value_name = "USER_MESSAGE", required = true)]
    user_message: String,
}

#[derive(Debug, Parser)]
struct DebugPromptInputCommand {
    /// Optional user prompt to append after session context.
    #[arg(value_name = "PROMPT")]
    prompt: Option<String>,

    /// Optional image(s) to attach to the user prompt.
    #[arg(long = "image", short = 'i', value_name = "FILE", value_delimiter = ',', num_args = 1..)]
    images: Vec<PathBuf>,
}

#[derive(Debug, Parser)]
struct DebugModelsCommand {
    /// Skip refresh and dump only the bundled catalog shipped with this binary.
    #[arg(long = "bundled", default_value_t = false)]
    bundled: bool,
}

#[derive(Debug, Parser)]
struct DebugTraceReduceCommand {
    /// Trace bundle directory containing manifest.json and trace.jsonl.
    #[arg(value_name = "TRACE_BUNDLE")]
    trace_bundle: PathBuf,

    /// Output path for reduced RolloutTrace JSON. Defaults to TRACE_BUNDLE/state.json.
    #[arg(long = "output", short = 'o', value_name = "FILE")]
    output: Option<PathBuf>,
}

#[derive(Debug, Parser)]
struct ResumeCommand {
    /// Conversation/session id (UUID) or thread name. UUIDs take precedence if it parses.
    /// If omitted, use --last to pick the most recent recorded session.
    #[arg(value_name = "SESSION_ID")]
    session_id: Option<String>,

    /// Continue the most recent session without showing the picker.
    #[arg(long = "last", default_value_t = false)]
    last: bool,

    /// Show all sessions (disables cwd filtering and shows CWD column).
    #[arg(long = "all", default_value_t = false)]
    all: bool,

    /// Include non-interactive sessions in the resume picker and --last selection.
    #[arg(long = "include-non-interactive", default_value_t = false)]
    include_non_interactive: bool,

    #[clap(flatten)]
    remote: InteractiveRemoteOptions,

    #[clap(flatten)]
    config_overrides: TuiCli,
}

#[derive(Debug, Parser)]
struct ForkCommand {
    /// Conversation/session id (UUID). When provided, forks this session.
    /// If omitted, use --last to pick the most recent recorded session.
    #[arg(value_name = "SESSION_ID")]
    session_id: Option<String>,

    /// Fork the most recent session without showing the picker.
    #[arg(long = "last", default_value_t = false, conflicts_with = "session_id")]
    last: bool,

    /// Show all sessions (disables cwd filtering and shows CWD column).
    #[arg(long = "all", default_value_t = false)]
    all: bool,

    #[clap(flatten)]
    remote: InteractiveRemoteOptions,

    #[clap(flatten)]
    config_overrides: TuiCli,
}

#[derive(Debug, Parser)]
struct SandboxArgs {
    #[command(subcommand)]
    cmd: SandboxCommand,
}

#[derive(Debug, clap::Subcommand)]
enum SandboxCommand {
    /// Run a command under Seatbelt (macOS only).
    #[clap(visible_alias = "seatbelt")]
    Macos(SeatbeltCommand),

    /// Run a command under the Linux sandbox (bubblewrap by default).
    #[clap(visible_alias = "landlock")]
    Linux(LandlockCommand),

    /// Run a command under Windows restricted token (Windows only).
    Windows(WindowsCommand),
}

#[derive(Debug, Parser)]
struct ExecpolicyCommand {
    #[command(subcommand)]
    sub: ExecpolicySubcommand,
}

#[derive(Debug, Parser)]
struct InternalArgs {
    #[command(subcommand)]
    cmd: InternalCommand,
}

#[derive(Debug, clap::Subcommand)]
enum InternalCommand {
    /// Extract session context dynamically for SessionStart hook.
    #[clap(name = "get-session-context")]
    GetSessionContext,
}

#[derive(Debug, clap::Subcommand)]
enum ExecpolicySubcommand {
    /// Check execpolicy files against a command.
    #[clap(name = "check")]
    Check(ExecPolicyCheckCommand),
}

#[derive(Debug, Parser)]
struct LoginCommand {
    #[clap(skip)]
    config_overrides: CliConfigOverrides,

    #[arg(
        long = "with-api-key",
        help = "Read the API key from stdin (e.g. `printenv OPENAI_API_KEY | codex login --with-api-key`)"
    )]
    with_api_key: bool,

    #[arg(
        long = "with-access-token",
        help = "Read the access token from stdin (e.g. `printenv CODEX_ACCESS_TOKEN | codex login --with-access-token`)"
    )]
    with_access_token: bool,

    #[arg(
        long = "api-key",
        num_args = 0..=1,
        default_missing_value = "",
        value_name = "API_KEY",
        help = "(deprecated) Previously accepted the API key directly; now exits with guidance to use --with-api-key",
        hide = true
    )]
    api_key: Option<String>,

    #[arg(long = "device-auth")]
    use_device_code: bool,

    /// EXPERIMENTAL: Use custom OAuth issuer base URL (advanced)
    /// Override the OAuth issuer base URL (advanced)
    #[arg(long = "experimental_issuer", value_name = "URL", hide = true)]
    issuer_base_url: Option<String>,

    /// EXPERIMENTAL: Use custom OAuth client ID (advanced)
    #[arg(long = "experimental_client-id", value_name = "CLIENT_ID", hide = true)]
    client_id: Option<String>,

    #[command(subcommand)]
    action: Option<LoginSubcommand>,
}

#[derive(Debug, clap::Subcommand)]
enum LoginSubcommand {
    /// Show login status.
    Status,
}

#[derive(Debug, Parser)]
struct LogoutCommand {
    #[clap(skip)]
    config_overrides: CliConfigOverrides,
}

#[cfg(feature = "ilhae")]
#[derive(Debug, Parser)]
struct IlhaeAuthCommand {
    #[command(subcommand)]
    subcommand: IlhaeAuthSubcommand,
}

#[cfg(feature = "ilhae")]
#[derive(Debug, clap::Subcommand)]
enum IlhaeAuthSubcommand {
    /// Sign in with the Ilhae identity server.
    Login {
        /// Identity issuer URL. Defaults to https://auth.ugot.uk.
        #[arg(long)]
        issuer: Option<String>,

        /// OAuth client id. Defaults to ilhae-cli.
        #[arg(long = "client-id")]
        client_id: Option<String>,

        /// Print the login URL instead of opening a browser.
        #[arg(long = "no-browser", default_value_t = false)]
        no_browser: bool,

        /// Print machine-readable status after login.
        #[arg(long)]
        json: bool,
    },

    /// Show Ilhae identity login status.
    Status {
        #[arg(long)]
        json: bool,
    },

    /// Remove stored Ilhae identity credentials.
    Logout {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Parser)]
struct AppServerCommand {
    /// Omit to run the app server; specify a subcommand for tooling.
    #[command(subcommand)]
    subcommand: Option<AppServerSubcommand>,

    /// Error out when config.toml contains fields that are not recognized by this version of Codex.
    #[arg(long = "strict-config", default_value_t = false)]
    strict_config: bool,

    /// Transport endpoint URL. Supported values: `stdio://` (default),
    /// `unix://`, `unix://PATH`, `ws://IP:PORT`, `off`.
    #[arg(
        long = "listen",
        value_name = "URL",
        default_value = codex_app_server::AppServerTransport::DEFAULT_LISTEN_URL
    )]
    listen: codex_app_server::AppServerTransport,

    /// Controls whether analytics are enabled by default.
    ///
    /// Analytics are disabled by default for app-server. Users have to explicitly opt in
    /// via the `analytics` section in the config.toml file.
    ///
    /// However, for first-party use cases like the VSCode IDE extension, we default analytics
    /// to be enabled by default by setting this flag. Users can still opt out by setting this
    /// in their config.toml:
    ///
    /// ```toml
    /// [analytics]
    /// enabled = false
    /// ```
    ///
    /// See https://developers.openai.com/codex/config-advanced/#metrics for more details.
    #[arg(long = "analytics-default-enabled")]
    analytics_default_enabled: bool,

    #[command(flatten)]
    auth: codex_app_server::AppServerWebsocketAuthArgs,
}

#[cfg(feature = "ilhae")]
#[derive(Debug, Parser)]
struct ProfileCommand {
    #[command(subcommand)]
    subcommand: ProfileSubcommand,
}

#[cfg(feature = "ilhae")]
#[derive(Debug, clap::Subcommand)]
enum ProfileSubcommand {
    /// List configured Ilhae profiles.
    List {
        #[arg(long)]
        json: bool,
    },

    /// Activate a profile and switch its managed local runtime.
    Set {
        profile_id: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Parser)]
struct ExecServerCommand {
    /// Transport endpoint URL. Supported values: `ws://IP:PORT` (default), `stdio`, `stdio://`.
    #[arg(long = "listen", value_name = "URL", conflicts_with = "remote")]
    listen: Option<String>,

    /// Register this exec-server as a remote executor using the given base URL.
    #[arg(long = "remote", value_name = "URL", requires = "executor_id")]
    remote: Option<String>,

    /// Executor id to attach to when registering remotely.
    #[arg(long = "executor-id", value_name = "ID")]
    executor_id: Option<String>,

    /// Human-readable executor name.
    #[arg(long = "name", value_name = "NAME")]
    name: Option<String>,

    /// Use Agent Identity auth from CODEX_ACCESS_TOKEN for remote registration.
    #[arg(long = "use-agent-identity-auth", requires = "remote")]
    use_agent_identity_auth: bool,
}

#[derive(Debug, clap::Subcommand)]
#[allow(clippy::enum_variant_names)]
enum AppServerSubcommand {
    /// Proxy stdio bytes to the running app-server control socket.
    Proxy(AppServerProxyCommand),

    /// [experimental] Generate TypeScript bindings for the app server protocol.
    GenerateTs(GenerateTsCommand),

    /// [experimental] Generate JSON Schema for the app server protocol.
    GenerateJsonSchema(GenerateJsonSchemaCommand),

    /// [internal] Generate internal JSON Schema artifacts for Codex tooling.
    #[clap(hide = true)]
    GenerateInternalJsonSchema(GenerateInternalJsonSchemaCommand),
}

#[derive(Debug, Args)]
struct AppServerProxyCommand {
    /// Path to the app-server Unix domain socket to connect to.
    #[arg(long = "sock", value_name = "SOCKET_PATH", value_parser = parse_socket_path)]
    socket_path: Option<AbsolutePathBuf>,
}

#[derive(Debug, Args)]
struct GenerateTsCommand {
    /// Output directory where .ts files will be written
    #[arg(short = 'o', long = "out", value_name = "DIR")]
    out_dir: PathBuf,

    /// Optional path to the Prettier executable to format generated files
    #[arg(short = 'p', long = "prettier", value_name = "PRETTIER_BIN")]
    prettier: Option<PathBuf>,

    /// Include experimental methods and fields in the generated output
    #[arg(long = "experimental", default_value_t = false)]
    experimental: bool,
}

#[derive(Debug, Args)]
struct GenerateJsonSchemaCommand {
    /// Output directory where the schema bundle will be written
    #[arg(short = 'o', long = "out", value_name = "DIR")]
    out_dir: PathBuf,

    /// Include experimental methods and fields in the generated output
    #[arg(long = "experimental", default_value_t = false)]
    experimental: bool,
}

#[derive(Debug, Args)]
struct GenerateInternalJsonSchemaCommand {
    /// Output directory where internal JSON Schema artifacts will be written
    #[arg(short = 'o', long = "out", value_name = "DIR")]
    out_dir: PathBuf,
}

#[derive(Debug, Parser)]
struct StdioToUdsCommand {
    /// Path to the Unix domain socket to connect to.
    #[arg(value_name = "SOCKET_PATH", value_parser = parse_socket_path)]
    socket_path: AbsolutePathBuf,
}

fn parse_socket_path(raw: &str) -> Result<AbsolutePathBuf, String> {
    AbsolutePathBuf::relative_to_current_dir(raw)
        .map_err(|err| format!("failed to resolve socket path `{raw}`: {err}"))
}

fn format_exit_messages(exit_info: AppExitInfo, color_enabled: bool) -> Vec<String> {
    let AppExitInfo {
        token_usage,
        thread_id: conversation_id,
        ..
    } = exit_info;

    let mut lines = Vec::new();
    if !token_usage.is_zero() {
        lines.push(token_usage.to_string());
    }

    if let Some(resume_cmd) = resume_command(/*thread_name*/ None, conversation_id) {
        let command = if color_enabled {
            resume_cmd.cyan().to_string()
        } else {
            resume_cmd
        };
        lines.push(format!("To continue this session, run {command}"));
    }

    lines
}

/// Handle the app exit and print the results. Optionally run the update action.
fn handle_app_exit(exit_info: AppExitInfo) -> anyhow::Result<()> {
    match exit_info.exit_reason {
        ExitReason::Fatal(message) => {
            eprintln!("ERROR: {message}");
            std::process::exit(1);
        }
        ExitReason::UserRequested => { /* normal exit */ }
    }

    let update_action = exit_info.update_action;
    let color_enabled = supports_color::on(Stream::Stdout).is_some();
    for line in format_exit_messages(exit_info, color_enabled) {
        println!("{line}");
    }
    if let Some(action) = update_action {
        run_update_action(action)?;
    }
    Ok(())
}

/// Run the update action and print the result.
fn run_update_action(action: UpdateAction) -> anyhow::Result<()> {
    println!();
    let cmd_str = action.command_str();
    println!("Updating Codex via `{cmd_str}`...");

    let status = {
        #[cfg(windows)]
        {
            if action == UpdateAction::StandaloneWindows {
                let (cmd, args) = action.command_args();
                // Run the standalone PowerShell installer with PowerShell
                // itself. Routing this through `cmd.exe /C` would parse
                // PowerShell metacharacters like `|` before PowerShell sees
                // the installer command.
                std::process::Command::new(cmd).args(args).status()?
            } else {
                // On Windows, run via cmd.exe so .CMD/.BAT are correctly resolved (PATHEXT semantics).
                std::process::Command::new("cmd")
                    .args(["/C", &cmd_str])
                    .status()?
            }
        }
        #[cfg(not(windows))]
        {
            let (cmd, args) = action.command_args();
            let command_path = crate::wsl_paths::normalize_for_wsl(cmd);
            let normalized_args: Vec<String> = args
                .iter()
                .map(crate::wsl_paths::normalize_for_wsl)
                .collect();
            std::process::Command::new(&command_path)
                .args(&normalized_args)
                .status()?
        }
    };
    if !status.success() {
        anyhow::bail!("`{cmd_str}` failed with status {status}");
    }
    println!("\n🎉 Update ran successfully! Please restart Codex.");
    Ok(())
}

fn run_update_command() -> anyhow::Result<()> {
    #[cfg(debug_assertions)]
    {
        anyhow::bail!(
            "`codex update` is not available in debug builds. Install a release build of Codex to use this command."
        );
    }

    #[cfg(not(debug_assertions))]
    {
        let Some(action) = codex_tui::get_update_action() else {
            anyhow::bail!(
                "Could not detect the Codex installation method. Please update manually: https://developers.openai.com/codex/cli/"
            );
        };
        run_update_action(action)
    }
}

fn run_execpolicycheck(cmd: ExecPolicyCheckCommand) -> anyhow::Result<()> {
    cmd.run()
}

async fn run_debug_app_server_command(cmd: DebugAppServerCommand) -> anyhow::Result<()> {
    match cmd.subcommand {
        DebugAppServerSubcommand::SendMessageV2(cmd) => {
            let codex_bin = std::env::current_exe()?;
            codex_app_server_test_client::send_message_v2(&codex_bin, &[], cmd.user_message, &None)
                .await
        }
    }
}

#[derive(Debug, Default, Parser, Clone)]
struct FeatureToggles {
    /// Enable a feature (repeatable). Equivalent to `-c features.<name>=true`.
    #[arg(long = "enable", value_name = "FEATURE", action = clap::ArgAction::Append, global = true)]
    enable: Vec<String>,

    /// Disable a feature (repeatable). Equivalent to `-c features.<name>=false`.
    #[arg(long = "disable", value_name = "FEATURE", action = clap::ArgAction::Append, global = true)]
    disable: Vec<String>,
}

#[derive(Debug, Default, Parser, Clone)]
struct InteractiveRemoteOptions {
    /// Connect the TUI to a remote app server websocket endpoint.
    ///
    /// Accepted forms: `ws://host:port` or `wss://host:port`.
    #[arg(long = "remote", value_name = "ADDR")]
    remote: Option<String>,

    /// Name of the environment variable containing the bearer token to send to
    /// a remote app server websocket.
    #[arg(long = "remote-auth-token-env", value_name = "ENV_VAR")]
    remote_auth_token_env: Option<String>,
}

impl FeatureToggles {
    fn to_overrides(&self) -> anyhow::Result<Vec<String>> {
        let mut v = Vec::new();
        for feature in &self.enable {
            Self::validate_feature(feature)?;
            v.push(format!("features.{feature}=true"));
        }
        for feature in &self.disable {
            Self::validate_feature(feature)?;
            v.push(format!("features.{feature}=false"));
        }
        Ok(v)
    }

    fn validate_feature(feature: &str) -> anyhow::Result<()> {
        if is_known_feature_key(feature) {
            Ok(())
        } else {
            anyhow::bail!("Unknown feature flag: {feature}")
        }
    }
}

#[derive(Debug, Parser)]
struct FeaturesCli {
    #[command(subcommand)]
    sub: FeaturesSubcommand,
}

#[derive(Debug, Parser)]
enum FeaturesSubcommand {
    /// List known features with their stage and effective state.
    List,
    /// Enable a feature in config.toml.
    Enable(FeatureSetArgs),
    /// Disable a feature in config.toml.
    Disable(FeatureSetArgs),
}

#[derive(Debug, Parser)]
struct FeatureSetArgs {
    /// Feature key to update (for example: unified_exec).
    feature: String,
}

fn stage_str(stage: Stage) -> &'static str {
    match stage {
        Stage::UnderDevelopment => "under development",
        Stage::Experimental { .. } => "experimental",
        Stage::Stable => "stable",
        Stage::Deprecated => "deprecated",
        Stage::Removed => "removed",
    }
}

fn main() -> anyhow::Result<()> {
    arg0_dispatch_or_else(|arg0_paths: Arg0DispatchPaths| async move {
        cli_main(arg0_paths).await?;
        Ok(())
    })
}

#[cfg(feature = "ilhae")]
fn is_invoked_as_ilhae_cli() -> bool {
    if std::env::var("ILHAE_APP_SERVER").ok().as_deref() == Some("1")
        || std::env::var("ILHAE_RUNTIME").ok().as_deref() == Some("1")
    {
        return true;
    }

    std::env::args_os()
        .next()
        .and_then(|arg0| {
            std::path::Path::new(&arg0)
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .is_some_and(|name| matches!(name.as_str(), "ilhae" | "codex-ilhae" | "codex-ilhae-cli"))
}

#[cfg(feature = "ilhae")]
fn thread_goal_loop_event_from_ilhae_lifecycle(
    notification: codex_ilhae::IlhaeLoopLifecycleNotification,
) -> codex_state::ThreadGoalLoopEvent {
    match notification {
        codex_ilhae::IlhaeLoopLifecycleNotification::Started { item, .. }
        | codex_ilhae::IlhaeLoopLifecycleNotification::Completed { item, .. }
        | codex_ilhae::IlhaeLoopLifecycleNotification::Failed { item, .. } => {
            thread_goal_loop_event_from_ilhae_item(item)
        }
        codex_ilhae::IlhaeLoopLifecycleNotification::Progress {
            item_id,
            kind,
            summary,
            detail,
            ..
        } => codex_state::ThreadGoalLoopEvent {
            id: item_id.clone(),
            phase: thread_goal_loop_phase_from_ilhae_parts(&item_id, "", kind),
            status: codex_state::ThreadGoalLoopStatus::InProgress,
            title: "Loop progress".to_string(),
            summary,
            detail,
            error: None,
        },
    }
}

#[cfg(feature = "ilhae")]
fn thread_goal_loop_event_from_ilhae_item(
    item: codex_ilhae::LoopLifecycleItem,
) -> codex_state::ThreadGoalLoopEvent {
    codex_state::ThreadGoalLoopEvent {
        id: item.id.clone(),
        phase: thread_goal_loop_phase_from_ilhae_parts(&item.id, &item.title, item.kind),
        status: thread_goal_loop_status_from_ilhae(item.status),
        title: item.title,
        summary: item.summary,
        detail: item.detail,
        error: item.error,
    }
}

#[cfg(feature = "ilhae")]
fn thread_goal_loop_phase_from_ilhae_parts(
    id: &str,
    title: &str,
    kind: codex_ilhae::LoopLifecycleKind,
) -> codex_state::ThreadGoalLoopPhase {
    let id = id.to_ascii_lowercase();
    let title = title.to_ascii_lowercase();
    if id.contains("kairos") || title.contains("kairos") {
        return codex_state::ThreadGoalLoopPhase::KairosLoop;
    }
    if id.contains("knowledge_loop") || title.contains("knowledge") {
        return codex_state::ThreadGoalLoopPhase::KnowledgeLoop;
    }
    if id.contains("cleanup") || title.contains("cleanup") || title.contains("hygiene") {
        return codex_state::ThreadGoalLoopPhase::CleanupLoop;
    }
    match kind {
        codex_ilhae::LoopLifecycleKind::SuperLoop => codex_state::ThreadGoalLoopPhase::SuperLoop,
        codex_ilhae::LoopLifecycleKind::ExecutionLoop => {
            codex_state::ThreadGoalLoopPhase::ExecutionLoop
        }
        codex_ilhae::LoopLifecycleKind::ImprovementLoop => {
            codex_state::ThreadGoalLoopPhase::ImprovementLoop
        }
        codex_ilhae::LoopLifecycleKind::CleanupLoop => {
            codex_state::ThreadGoalLoopPhase::CleanupLoop
        }
        codex_ilhae::LoopLifecycleKind::ContextInjection => {
            codex_state::ThreadGoalLoopPhase::ContextInjection
        }
    }
}

#[cfg(feature = "ilhae")]
fn thread_goal_loop_status_from_ilhae(
    status: codex_ilhae::LoopLifecycleStatus,
) -> codex_state::ThreadGoalLoopStatus {
    match status {
        codex_ilhae::LoopLifecycleStatus::InProgress => {
            codex_state::ThreadGoalLoopStatus::InProgress
        }
        codex_ilhae::LoopLifecycleStatus::Completed => codex_state::ThreadGoalLoopStatus::Completed,
        codex_ilhae::LoopLifecycleStatus::Failed => codex_state::ThreadGoalLoopStatus::Failed,
    }
}

#[cfg(feature = "ilhae")]
async fn collect_ilhae_foreground_loop_events(
    goal_continuation: bool,
) -> Vec<codex_state::ThreadGoalLoopEvent> {
    let result = if goal_continuation {
        codex_ilhae::run_active_goal_foreground_loop_cycle_collecting_lifecycle().await
    } else {
        codex_ilhae::run_active_foreground_loop_cycle_collecting_lifecycle().await
    };
    match result {
        Ok(notifications) => notifications
            .into_iter()
            .map(thread_goal_loop_event_from_ilhae_lifecycle)
            .collect(),
        Err(err) => {
            let stage = if goal_continuation {
                "goal continuation"
            } else {
                "app-server turn"
            };
            tracing::warn!(
                error = ?err,
                "ilhae foreground loop cycle failed before {stage}"
            );
            Vec::new()
        }
    }
}

#[cfg(feature = "ilhae")]
fn ilhae_foreground_loop_hook(goal_continuation: bool) -> codex_app_server::AppServerTurnStartHook {
    Arc::new(move || {
        Box::pin(async move {
            let thread_goal_loop_events =
                collect_ilhae_foreground_loop_events(goal_continuation).await;
            codex_app_server::AppServerTurnStartHookResult {
                thread_goal_loop_events,
            }
        })
    })
}

#[cfg(feature = "ilhae")]
fn ilhae_app_server_runtime_hooks() -> codex_app_server::AppServerRuntimeHooks {
    codex_app_server::AppServerRuntimeHooks {
        before_turn_start: Some(ilhae_foreground_loop_hook(/*goal_continuation*/ false)),
        before_goal_continuation: Some(ilhae_foreground_loop_hook(/*goal_continuation*/ true)),
    }
}

#[cfg(feature = "ilhae")]
fn prepare_ilhae_cli_environment_if_needed() -> anyhow::Result<Option<std::path::PathBuf>> {
    if !is_invoked_as_ilhae_cli() {
        return Ok(None);
    }

    let codex_home = codex_ilhae::config::prepare_ilhae_codex_home().map_err(anyhow::Error::msg)?;
    Ok(Some(codex_home))
}

#[cfg(feature = "ilhae")]
fn apply_ilhae_codex_home_loader_overrides(
    loader_overrides: &mut codex_config::LoaderOverrides,
    codex_home: &std::path::Path,
) {
    loader_overrides.managed_config_path = Some(codex_home.join("managed_config.toml"));
}

#[cfg(feature = "ilhae")]
fn ilhae_profile_engine_id(profile: &codex_ilhae::config::IlhaeProfileConfig) -> String {
    profile
        .agent
        .engine_id
        .clone()
        .or_else(|| {
            profile
                .agent
                .command
                .as_deref()
                .map(codex_ilhae::helpers::infer_agent_id_from_command)
        })
        .unwrap_or_else(|| "ilhae".to_string())
}

#[cfg(feature = "ilhae")]
fn native_runtime_provider_name(
    runtime: &codex_ilhae::config::IlhaeProfileNativeRuntimeConfig,
) -> String {
    runtime
        .provider
        .clone()
        .filter(|provider| !provider.trim().is_empty())
        .unwrap_or_else(|| "llama-server".to_string())
}

#[cfg(feature = "ilhae")]
fn ilhae_profile_provider_id(profile: &codex_ilhae::config::IlhaeProfileConfig) -> String {
    if profile.native_runtime.enabled {
        native_runtime_provider_name(&profile.native_runtime)
    } else {
        ilhae_profile_engine_id(profile)
    }
}

#[cfg(feature = "ilhae")]
fn native_runtime_oss_provider(profile_id: Option<&str>) -> Option<String> {
    codex_ilhae::config::get_native_runtime_config(profile_id)
        .map(|(_, runtime)| native_runtime_provider_name(&runtime))
}

#[cfg(feature = "ilhae")]
fn toml_bool(value: &toml::Value) -> Option<bool> {
    value.as_bool().or_else(|| {
        match value
            .as_str()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("true" | "1" | "on" | "enabled") => Some(true),
            Some("false" | "0" | "off" | "disabled") => Some(false),
            _ => None,
        }
    })
}

#[cfg(feature = "ilhae")]
fn toml_string(value: &toml::Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[cfg(feature = "ilhae")]
fn apply_ilhae_agent_cli_overrides(
    settings: &mut codex_ilhae::settings_types::Settings,
    overrides: &[(String, toml::Value)],
) {
    for (key, value) in overrides {
        match key.as_str() {
            "agent.kairos_enabled" | "agent.kairos" => {
                if let Some(enabled) = toml_bool(value) {
                    settings.agent.kairos_enabled = enabled;
                }
            }
            "agent.self_improvement_enabled" | "agent.self_improvement" => {
                if let Some(enabled) = toml_bool(value) {
                    settings.agent.self_improvement_enabled = enabled;
                }
            }
            "agent.self_improvement_preset" => {
                if let Some(preset) = toml_string(value) {
                    settings.agent.self_improvement_preset = preset;
                }
            }
            "agent.autonomous_mode" | "agent.autonomous" => {
                if let Some(enabled) = toml_bool(value) {
                    settings.agent.autonomous_mode = enabled;
                }
            }
            "agent.knowledge_mode" => {
                if let Some(mode) = toml_string(value) {
                    settings.agent.knowledge_mode = mode;
                }
            }
            "agent.hygiene_mode" => {
                if let Some(mode) = toml_string(value) {
                    settings.agent.hygiene_mode = mode;
                }
            }
            _ => {}
        }
    }
}

#[cfg(feature = "ilhae")]
fn ilhae_exec_runtime_settings_from_overrides(
    overrides: &CliConfigOverrides,
) -> Option<codex_ilhae::settings_types::Settings> {
    let parsed = overrides.parse_overrides().ok()?;
    let ilhae_dir = codex_ilhae::config::resolve_ilhae_data_dir();
    let store = codex_ilhae::settings_store::SettingsStore::new(&ilhae_dir);
    if let Err(err) = codex_ilhae::config::apply_active_ilhae_profile_projection(&store) {
        tracing::warn!(?err, "failed to project active Ilhae profile for exec");
    }
    let mut settings = store.get();
    apply_ilhae_agent_cli_overrides(&mut settings, &parsed);
    Some(settings)
}

#[cfg(feature = "ilhae")]
fn ilhae_exec_loop_developer_instructions_from_settings(
    settings: &codex_ilhae::settings_types::Settings,
) -> Option<String> {
    codex_ilhae::session_context_service::build_runtime_loop_developer_instructions(settings)
}

#[cfg(feature = "ilhae")]
#[cfg(test)]
fn ilhae_exec_loop_developer_instructions_from_overrides(
    overrides: &CliConfigOverrides,
) -> Option<String> {
    let settings = ilhae_exec_runtime_settings_from_overrides(overrides)?;
    ilhae_exec_loop_developer_instructions_from_settings(&settings)
}

#[cfg(feature = "ilhae")]
fn ilhae_exec_should_run_foreground_loops(
    settings: &codex_ilhae::settings_types::Settings,
) -> bool {
    ilhae_exec_loop_developer_instructions_from_settings(settings).is_some()
}

#[cfg(feature = "ilhae")]
fn ilhae_profile_display_name(
    profile_id: &str,
    profile: &codex_ilhae::config::IlhaeProfileConfig,
) -> String {
    if profile.native_runtime.enabled
        && let Some(model) = codex_ilhae::config::native_runtime_model_name_from_path(
            &profile.native_runtime.model_path,
        )
    {
        return model;
    }

    profile
        .agent
        .engine_id
        .clone()
        .or_else(|| profile.agent.command.clone())
        .unwrap_or_else(|| profile_id.to_string())
}

#[cfg(feature = "ilhae")]
async fn run_ilhae_profile_command(cmd: ProfileCommand) -> anyhow::Result<()> {
    match cmd.subcommand {
        ProfileSubcommand::List { json } => {
            let config = codex_ilhae::config::load_ilhae_toml_config();
            if json {
                let profiles = config
                    .profiles
                    .iter()
                    .map(|(id, profile)| {
                        serde_json::json!({
                            "id": id,
                            "name": ilhae_profile_display_name(id, profile),
                            "provider": ilhae_profile_provider_id(profile),
                            "nativeRuntime": profile.native_runtime.enabled,
                            "baseUrl": profile.native_runtime.base_url,
                            "healthUrl": profile.native_runtime.health_url,
                        })
                    })
                    .collect::<Vec<_>>();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "activeProfile": config.profile.active,
                        "profiles": profiles,
                    }))?
                );
            } else {
                let active = config.profile.active.as_deref().unwrap_or("none");
                println!("Active profile: {active}");
                for (id, profile) in &config.profiles {
                    let marker = if Some(id) == config.profile.active.as_ref() {
                        "*"
                    } else {
                        " "
                    };
                    println!(
                        "{marker} {id}\t{}\t{}",
                        ilhae_profile_display_name(id, profile),
                        ilhae_profile_provider_id(profile)
                    );
                }
            }
        }
        ProfileSubcommand::Set { profile_id, json } => {
            let previous_active = codex_ilhae::config::load_ilhae_toml_config().profile.active;
            let profile = codex_ilhae::config::set_active_ilhae_profile(&profile_id)
                .map_err(anyhow::Error::msg)?;
            let ilhae_dir = codex_ilhae::config::resolve_ilhae_data_dir();
            let settings = codex_ilhae::settings_store::SettingsStore::new(&ilhae_dir);
            codex_ilhae::config::apply_ilhae_profile_projection(&settings, &profile)
                .map_err(anyhow::Error::msg)?;
            codex_ilhae::config::prepare_ilhae_codex_home().map_err(anyhow::Error::msg)?;
            codex_ilhae::switch_native_runtime_for_cli(
                previous_active.as_deref(),
                Some(profile.id.as_str()),
            )
            .await?;

            if json {
                let active_profile = profile.id.clone();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "ok": true,
                        "activeProfile": active_profile,
                        "profile": profile,
                    }))?
                );
            } else {
                println!("Active profile: {}", profile.id);
            }
        }
    }

    Ok(())
}

#[cfg(feature = "ilhae")]
async fn run_ilhae_auth_command(cmd: IlhaeAuthCommand) -> anyhow::Result<()> {
    match cmd.subcommand {
        IlhaeAuthSubcommand::Login {
            issuer,
            client_id,
            no_browser,
            json,
        } => {
            let status = run_ilhae_identity_login(issuer, client_id, !no_browser).await?;
            print_ilhae_auth_status(&status, json)?;
        }
        IlhaeAuthSubcommand::Status { json } => {
            let status = codex_ilhae::auth::status()?;
            print_ilhae_auth_status(&status, json)?;
        }
        IlhaeAuthSubcommand::Logout { json } => {
            let removed = codex_ilhae::auth::logout()?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "ok": true,
                        "removed": removed,
                    }))?
                );
            } else if removed {
                println!("Signed out of Ilhae identity.");
            } else {
                println!("No Ilhae identity credentials were stored.");
            }
        }
    }

    Ok(())
}

#[cfg(feature = "ilhae")]
async fn run_ilhae_login_compat_command(login_cli: LoginCommand) -> anyhow::Result<()> {
    if login_cli.with_api_key
        || login_cli.with_access_token
        || login_cli.api_key.is_some()
        || login_cli.use_device_code
    {
        anyhow::bail!(
            "Ilhae login uses the identity server. Use `codex login` for OpenAI credentials."
        );
    }

    match login_cli.action {
        Some(LoginSubcommand::Status) => {
            let status = codex_ilhae::auth::status()?;
            print_ilhae_auth_status(&status, /*json*/ false)?;
        }
        None => {
            let status = run_ilhae_identity_login(
                login_cli.issuer_base_url,
                login_cli.client_id,
                /*open_browser*/ true,
            )
            .await?;
            print_ilhae_auth_status(&status, /*json*/ false)?;
        }
    }

    Ok(())
}

#[cfg(feature = "ilhae")]
async fn run_ilhae_identity_login(
    issuer: Option<String>,
    client_id: Option<String>,
    open_browser: bool,
) -> anyhow::Result<codex_ilhae::auth::IdentityAuthStatus> {
    codex_ilhae::auth::login(codex_ilhae::auth::IdentityLoginOptions {
        issuer: issuer.unwrap_or_else(|| codex_ilhae::auth::DEFAULT_ISSUER.to_string()),
        client_id: client_id.unwrap_or_else(|| codex_ilhae::auth::DEFAULT_CLIENT_ID.to_string()),
        open_browser,
    })
    .await
}

#[cfg(feature = "ilhae")]
fn print_ilhae_auth_status(
    status: &codex_ilhae::auth::IdentityAuthStatus,
    json: bool,
) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(status)?);
        return Ok(());
    }

    if !status.authenticated {
        println!(
            "Not signed in to Ilhae identity. Run `ilhae auth login` or `ilhae login` to sign in."
        );
        return Ok(());
    }

    let account = status
        .email
        .as_deref()
        .or(status.preferred_username.as_deref())
        .or(status.name.as_deref())
        .or(status.subject.as_deref())
        .unwrap_or("unknown account");
    let issuer = status
        .issuer
        .as_deref()
        .unwrap_or(codex_ilhae::auth::DEFAULT_ISSUER);
    let expired_suffix = if status.expired { " (expired)" } else { "" };
    println!("Signed in to Ilhae identity as {account}{expired_suffix}");
    println!("Issuer: {issuer}");
    println!("Auth file: {}", status.auth_file.display());

    Ok(())
}

#[cfg(feature = "ilhae")]
#[derive(Debug, clap::Subcommand)]
enum LocalServerCommand {
    /// Start the local model server.
    Start,
    /// Stop the local model server.
    Stop,
    /// Get the status of the local model server.
    Status,
}

#[cfg(feature = "ilhae")]
#[derive(Debug, Parser)]
struct GpuCommand {
    /// GPU queue daemon address or base URL.
    #[arg(long = "addr", global = true, value_name = "ADDR")]
    addr: Option<String>,

    #[command(subcommand)]
    subcommand: GpuSubcommand,
}

#[cfg(feature = "ilhae")]
#[derive(Debug, clap::Subcommand)]
enum GpuSubcommand {
    /// Run the GPU queue daemon in the foreground.
    Daemon(GpuDaemonCommand),

    /// Print GPU queue daemon status.
    Status {
        #[arg(long)]
        json: bool,
    },

    /// Acquire a GPU lease.
    Acquire(GpuAcquireCommand),

    /// Release a GPU lease.
    Release { lease_id: String },

    /// Run a command while holding a GPU lease.
    Run(GpuRunCommand),

    /// Control the local LLM runtime through the GPU queue daemon.
    Llm(GpuLlmCommand),

    /// Run a GPU-queued ComfyUI API proxy.
    #[clap(name = "comfy-proxy", visible_alias = "comfy-gateway")]
    ComfyProxy(GpuComfyProxyCommand),
}

#[cfg(feature = "ilhae")]
#[derive(Debug, Parser)]
struct GpuDaemonCommand {
    /// Listen address for the daemon.
    #[arg(long = "listen", value_name = "ADDR")]
    listen: Option<String>,
}

#[cfg(feature = "ilhae")]
#[derive(Debug, Parser)]
struct GpuAcquireCommand {
    /// Lease owner label.
    #[arg(long, default_value = "codex-cli")]
    owner: String,

    /// Lease kind, for example video or image.
    #[arg(long, default_value = "video")]
    kind: String,

    /// Request a shared lease instead of the default exclusive lease.
    #[arg(long)]
    shared: bool,

    /// Stop the local LLM runtime before granting the lease if it is running.
    #[arg(long = "preempt-llm")]
    preempt_llm: bool,

    /// Lease TTL in seconds.
    #[arg(long = "ttl-seconds", default_value_t = 3600)]
    ttl_seconds: u64,

    /// Wait for a pending lease for this many seconds.
    #[arg(long = "wait-timeout-seconds")]
    wait_timeout_seconds: Option<u64>,

    /// Print the response as JSON.
    #[arg(long)]
    json: bool,
}

#[cfg(feature = "ilhae")]
#[derive(Debug, Parser)]
struct GpuRunCommand {
    /// Lease owner label.
    #[arg(long, default_value = "codex-cli")]
    owner: String,

    /// Lease kind, for example video or image.
    #[arg(long, default_value = "video")]
    kind: String,

    /// Request a shared lease instead of the default exclusive lease.
    #[arg(long)]
    shared: bool,

    /// Stop the local LLM runtime before granting the lease if it is running.
    #[arg(long = "preempt-llm")]
    preempt_llm: bool,

    /// Lease TTL in seconds.
    #[arg(long = "ttl-seconds", default_value_t = 3600)]
    ttl_seconds: u64,

    /// Wait for a pending lease for this many seconds.
    #[arg(long = "wait-timeout-seconds", default_value_t = 900)]
    wait_timeout_seconds: u64,

    /// Command to execute while holding the lease.
    #[arg(last = true, required = true)]
    command: Vec<String>,
}

#[cfg(feature = "ilhae")]
#[derive(Debug, Parser)]
struct GpuLlmCommand {
    #[command(subcommand)]
    subcommand: GpuLlmSubcommand,
}

#[cfg(feature = "ilhae")]
#[derive(Debug, Parser)]
struct GpuComfyProxyCommand {
    /// Listen address for the ComfyUI API proxy.
    #[arg(long = "listen", value_name = "ADDR")]
    listen: Option<String>,

    /// Actual ComfyUI backend URL.
    #[arg(long = "backend-url", value_name = "URL")]
    backend_url: Option<String>,

    /// ComfyUI root directory for serving /view files while the backend is stopped.
    #[arg(long = "comfy-root", value_name = "PATH")]
    comfy_root: Option<PathBuf>,

    /// Lease owner label.
    #[arg(long)]
    owner: Option<String>,

    /// Shell command used to start the actual ComfyUI backend.
    #[arg(long = "start-command", value_name = "COMMAND")]
    start_command: Option<String>,

    /// Shell command used to stop the actual ComfyUI backend after a prompt finishes.
    #[arg(long = "stop-command", value_name = "COMMAND")]
    stop_command: Option<String>,

    /// Lease TTL in seconds.
    #[arg(long = "ttl-seconds")]
    ttl_seconds: Option<u64>,

    /// Wait for a pending lease for this many seconds.
    #[arg(long = "wait-timeout-seconds")]
    wait_timeout_seconds: Option<u64>,

    /// Poll interval for ComfyUI /history/{prompt_id}.
    #[arg(long = "prompt-poll-interval-ms")]
    prompt_poll_interval_ms: Option<u64>,

    /// Maximum seconds to hold a prompt lease while waiting for completion.
    #[arg(long = "prompt-timeout-seconds")]
    prompt_timeout_seconds: Option<u64>,

    /// Keep ComfyUI running after prompt completion.
    #[arg(long = "no-stop-after-prompt")]
    no_stop_after_prompt: bool,

    /// Auto-start ComfyUI for non-/prompt passthrough API calls.
    #[arg(
        long = "start-backend-for-passthrough",
        conflicts_with = "no_start_backend_for_passthrough"
    )]
    start_backend_for_passthrough: bool,

    /// Do not auto-start ComfyUI for non-/prompt passthrough API calls.
    #[arg(long = "no-start-backend-for-passthrough")]
    no_start_backend_for_passthrough: bool,
}

#[cfg(feature = "ilhae")]
#[derive(Debug, clap::Subcommand)]
enum GpuLlmSubcommand {
    Start,
    Stop,
    Restart,
}

async fn cli_main(arg0_paths: Arg0DispatchPaths) -> anyhow::Result<()> {
    #[cfg(feature = "ilhae")]
    let prepared_ilhae_codex_home = prepare_ilhae_cli_environment_if_needed()?;

    let MultitoolCli {
        config_overrides: mut root_config_overrides,
        feature_toggles,
        remote,
        mut interactive,
        subcommand,
    } = MultitoolCli::parse();

    // Fold --enable/--disable into config overrides so they flow to all subcommands.
    let toggle_overrides = feature_toggles.to_overrides()?;
    root_config_overrides.raw_overrides.extend(toggle_overrides);
    let root_remote = remote.remote;
    let root_remote_auth_token_env = remote.remote_auth_token_env;

    match subcommand {
        None => {
            #[cfg(feature = "ilhae")]
            if root_remote.is_none() {
                codex_ilhae::ensure_native_runtime_for_cli(interactive.config_profile.as_deref())
                    .await?;
            }
            prepend_config_flags(
                &mut interactive.config_overrides,
                root_config_overrides.clone(),
            );
            let exit_info = run_interactive_tui(
                interactive,
                root_remote.clone(),
                root_remote_auth_token_env.clone(),
                arg0_paths.clone(),
            )
            .await?;
            handle_app_exit(exit_info)?;
        }
        #[cfg(feature = "ilhae")]
        Some(Subcommand::Proxy) => {
            codex_ilhae::run_ilhae_proxy().await?;
        }
        #[cfg(feature = "ilhae")]
        Some(Subcommand::Stop) => {
            codex_ilhae::stop_native_runtime_for_cli(interactive.config_profile.as_deref()).await?;
        }
        #[cfg(feature = "ilhae")]
        Some(Subcommand::Profile(profile_cli)) => {
            run_ilhae_profile_command(profile_cli).await?;
        }
        #[cfg(feature = "ilhae")]
        Some(Subcommand::Auth(auth_cli)) => {
            reject_remote_mode_for_subcommand(
                root_remote.as_deref(),
                root_remote_auth_token_env.as_deref(),
                "auth",
            )?;
            run_ilhae_auth_command(auth_cli).await?;
        }
        Some(Subcommand::Exec(mut exec_cli)) => {
            reject_remote_mode_for_subcommand(
                root_remote.as_deref(),
                root_remote_auth_token_env.as_deref(),
                "exec",
            )?;
            exec_cli
                .shared
                .inherit_exec_root_options(&interactive.shared);
            #[cfg(feature = "ilhae")]
            {
                codex_ilhae::ensure_native_runtime_for_cli(exec_cli.config_profile.as_deref())
                    .await?;
                if let Some(provider) =
                    native_runtime_oss_provider(exec_cli.config_profile.as_deref())
                {
                    exec_cli.oss = true;
                    if exec_cli.oss_provider.is_none() {
                        exec_cli.oss_provider = Some(provider);
                    }
                }
            }
            prepend_config_flags(
                &mut exec_cli.config_overrides,
                root_config_overrides.clone(),
            );
            #[cfg(feature = "ilhae")]
            {
                let runtime_settings =
                    ilhae_exec_runtime_settings_from_overrides(&exec_cli.config_overrides);
                let external_notifications = is_invoked_as_ilhae_cli()
                    .then(codex_ilhae::spawn_app_server_external_notification_bridge);
                if let Some(settings) = runtime_settings.clone()
                    && ilhae_exec_should_run_foreground_loops(&settings)
                {
                    tokio::spawn(async move {
                        if let Err(err) =
                            codex_ilhae::run_exec_foreground_loop_cycle(settings).await
                        {
                            tracing::warn!(
                                error = ?err,
                                "ilhae exec foreground loop cycle failed"
                            );
                        }
                    });
                }
                codex_exec::run_main_with_external_notifications(
                    exec_cli,
                    arg0_paths.clone(),
                    external_notifications,
                )
                .await?;
            }
            #[cfg(not(feature = "ilhae"))]
            codex_exec::run_main(exec_cli, arg0_paths.clone()).await?;
        }
        Some(Subcommand::Review(review_args)) => {
            reject_remote_mode_for_subcommand(
                root_remote.as_deref(),
                root_remote_auth_token_env.as_deref(),
                "review",
            )?;
            let mut exec_cli = ExecCli::try_parse_from(["codex", "exec"])?;
            exec_cli
                .shared
                .inherit_exec_root_options(&interactive.shared);
            #[cfg(feature = "ilhae")]
            {
                codex_ilhae::ensure_native_runtime_for_cli(exec_cli.config_profile.as_deref())
                    .await?;
                if let Some(provider) =
                    native_runtime_oss_provider(exec_cli.config_profile.as_deref())
                {
                    exec_cli.oss = true;
                    exec_cli.oss_provider = Some(provider);
                }
            }
            exec_cli.command = Some(ExecCommand::Review(review_args));
            prepend_config_flags(
                &mut exec_cli.config_overrides,
                root_config_overrides.clone(),
            );
            codex_exec::run_main(exec_cli, arg0_paths.clone()).await?;
        }
        Some(Subcommand::McpServer) => {
            reject_remote_mode_for_subcommand(
                root_remote.as_deref(),
                root_remote_auth_token_env.as_deref(),
                "mcp-server",
            )?;
            codex_mcp_server::run_main(
                arg0_paths.clone(),
                root_config_overrides,
                interactive.strict_config,
            )
            .await?;
        }
        Some(Subcommand::Mcp(mut mcp_cli)) => {
            reject_remote_mode_for_subcommand(
                root_remote.as_deref(),
                root_remote_auth_token_env.as_deref(),
                "mcp",
            )?;
            // Propagate any root-level config overrides (e.g. `-c key=value`).
            prepend_config_flags(&mut mcp_cli.config_overrides, root_config_overrides.clone());
            mcp_cli.run().await?;
        }
        Some(Subcommand::Plugin(plugin_cli)) => {
            reject_remote_mode_for_subcommand(
                root_remote.as_deref(),
                root_remote_auth_token_env.as_deref(),
                "plugin",
            )?;
            let PluginCli {
                mut config_overrides,
                subcommand,
            } = plugin_cli;
            prepend_config_flags(&mut config_overrides, root_config_overrides.clone());
            match subcommand {
                PluginSubcommand::Add(args) => {
                    let overrides = config_overrides
                        .parse_overrides()
                        .map_err(anyhow::Error::msg)?;
                    plugin_cmd::run_plugin_add(overrides, args).await?;
                }
                PluginSubcommand::List(args) => {
                    let overrides = config_overrides
                        .parse_overrides()
                        .map_err(anyhow::Error::msg)?;
                    plugin_cmd::run_plugin_list(overrides, args).await?;
                }
                PluginSubcommand::Marketplace(mut marketplace_cli) => {
                    prepend_config_flags(&mut marketplace_cli.config_overrides, config_overrides);
                    marketplace_cli.run().await?;
                }
                PluginSubcommand::Remove(args) => {
                    let overrides = config_overrides
                        .parse_overrides()
                        .map_err(anyhow::Error::msg)?;
                    plugin_cmd::run_plugin_remove(overrides, args).await?;
                }
            }
        }
        Some(Subcommand::AppServer(app_server_cli)) => {
            let AppServerCommand {
                subcommand,
                strict_config,
                listen,
                analytics_default_enabled,
                auth,
            } = app_server_cli;
            reject_remote_mode_for_app_server_subcommand(
                root_remote.as_deref(),
                root_remote_auth_token_env.as_deref(),
                subcommand.as_ref(),
            )?;
            match subcommand {
                None => {
                    let transport = listen;
                    let auth = auth.try_into_settings()?;
                    let mut loader_overrides = codex_config::LoaderOverrides::default();
                    #[cfg(feature = "ilhae")]
                    let is_ilhae_app_server = is_invoked_as_ilhae_cli();
                    #[cfg(feature = "ilhae")]
                    let external_notifications = if is_ilhae_app_server {
                        Some(codex_ilhae::spawn_app_server_external_notification_bridge())
                    } else {
                        None
                    };
                    #[cfg(not(feature = "ilhae"))]
                    let external_notifications = None;
                    #[cfg(feature = "ilhae")]
                    let runtime_hooks = if is_ilhae_app_server {
                        ilhae_app_server_runtime_hooks()
                    } else {
                        codex_app_server::AppServerRuntimeHooks::default()
                    };
                    #[cfg(not(feature = "ilhae"))]
                    let runtime_hooks = codex_app_server::AppServerRuntimeHooks::default();
                    #[cfg(feature = "ilhae")]
                    if is_ilhae_app_server {
                        let codex_home = match prepared_ilhae_codex_home.as_ref() {
                            Some(codex_home) => codex_home.clone(),
                            None => codex_ilhae::config::prepare_ilhae_codex_home()
                                .map_err(anyhow::Error::msg)?,
                        };
                        apply_ilhae_codex_home_loader_overrides(&mut loader_overrides, &codex_home);
                        codex_ilhae::ensure_native_runtime_for_cli(
                            interactive.config_profile.as_deref(),
                        )
                        .await?;
                        let _ = codex_ilhae::bootstrap_ilhae_runtime().await?;
                    }
                    let runtime_options = codex_app_server::AppServerRuntimeOptions {
                        external_notifications,
                        runtime_hooks,
                        ..Default::default()
                    };
                    codex_app_server::run_main_with_transport_options(
                        arg0_paths.clone(),
                        root_config_overrides,
                        loader_overrides,
                        interactive.strict_config || strict_config,
                        analytics_default_enabled,
                        transport,
                        codex_protocol::protocol::SessionSource::VSCode,
                        auth,
                        runtime_options,
                    )
                    .await?;
                }
                Some(AppServerSubcommand::Proxy(proxy_cli)) => {
                    let socket_path = match proxy_cli.socket_path {
                        Some(socket_path) => socket_path,
                        None => {
                            let codex_home = find_codex_home()?;
                            codex_app_server::app_server_control_socket_path(&codex_home)?
                        }
                    };
                    codex_stdio_to_uds::run(socket_path.as_path()).await?;
                }
                Some(AppServerSubcommand::GenerateTs(gen_cli)) => {
                    let options = codex_app_server_protocol::GenerateTsOptions {
                        experimental_api: gen_cli.experimental,
                        ..Default::default()
                    };
                    codex_app_server_protocol::generate_ts_with_options(
                        &gen_cli.out_dir,
                        gen_cli.prettier.as_deref(),
                        options,
                    )?;
                }
                Some(AppServerSubcommand::GenerateJsonSchema(gen_cli)) => {
                    codex_app_server_protocol::generate_json_with_experimental(
                        &gen_cli.out_dir,
                        gen_cli.experimental,
                    )?;
                }
                Some(AppServerSubcommand::GenerateInternalJsonSchema(gen_cli)) => {
                    codex_app_server_protocol::generate_internal_json_schema(&gen_cli.out_dir)?;
                }
            }
        }
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        Some(Subcommand::App(app_cli)) => {
            reject_remote_mode_for_subcommand(
                root_remote.as_deref(),
                root_remote_auth_token_env.as_deref(),
                "app",
            )?;
            app_cmd::run_app(app_cli).await?;
        }
        Some(Subcommand::Resume(ResumeCommand {
            session_id,
            last,
            all,
            include_non_interactive,
            remote,
            config_overrides,
        })) => {
            #[cfg(feature = "ilhae")]
            {
                codex_ilhae::ensure_native_runtime_for_cli(interactive.config_profile.as_deref())
                    .await?;
            }
            interactive = finalize_resume_interactive(
                interactive,
                root_config_overrides.clone(),
                session_id,
                last,
                all,
                include_non_interactive,
                config_overrides,
            );
            let exit_info = run_interactive_tui(
                interactive,
                remote.remote.or(root_remote.clone()),
                remote
                    .remote_auth_token_env
                    .or(root_remote_auth_token_env.clone()),
                arg0_paths.clone(),
            )
            .await?;
            handle_app_exit(exit_info)?;
        }
        Some(Subcommand::Fork(ForkCommand {
            session_id,
            last,
            all,
            remote,
            config_overrides,
        })) => {
            #[cfg(feature = "ilhae")]
            {
                codex_ilhae::ensure_native_runtime_for_cli(interactive.config_profile.as_deref())
                    .await?;
            }
            interactive = finalize_fork_interactive(
                interactive,
                root_config_overrides.clone(),
                session_id,
                last,
                all,
                config_overrides,
            );
            let exit_info = run_interactive_tui(
                interactive,
                remote.remote.or(root_remote.clone()),
                remote
                    .remote_auth_token_env
                    .or(root_remote_auth_token_env.clone()),
                arg0_paths.clone(),
            )
            .await?;
            handle_app_exit(exit_info)?;
        }

        #[cfg(feature = "ilhae")]
        Some(Subcommand::LocalServer(local_cmd)) => match local_cmd {
            LocalServerCommand::Start => {
                codex_ilhae::ensure_native_runtime_for_cli(interactive.config_profile.as_deref())
                    .await?;
            }
            LocalServerCommand::Stop => {
                codex_ilhae::stop_native_runtime_for_cli(interactive.config_profile.as_deref())
                    .await?;
            }
            LocalServerCommand::Status => {
                if let Some((profile_id, config)) = codex_ilhae::config::get_native_runtime_config(
                    interactive.config_profile.as_deref(),
                ) {
                    let healthy =
                        codex_ilhae::startup_main::native_runtime_healthcheck(&config.health_url)
                            .await;
                    println!("Profile: {profile_id}");
                    println!("Enabled: {}", config.enabled);
                    println!("Health URL: {}", config.health_url);
                    println!("Status: {}", if healthy { "HEALTHY" } else { "DOWN" });
                } else {
                    println!("No active native runtime profile found.");
                }
            }
        },

        #[cfg(feature = "ilhae")]
        Some(Subcommand::Gpu(gpu_cmd)) => {
            reject_remote_mode_for_subcommand(
                root_remote.as_deref(),
                root_remote_auth_token_env.as_deref(),
                "gpu",
            )?;
            run_gpu_command(gpu_cmd, interactive.config_profile.as_deref()).await?;
        }

        #[cfg(feature = "ilhae")]
        Some(Subcommand::Start) => {
            codex_ilhae::ensure_native_runtime_for_cli(interactive.config_profile.as_deref())
                .await?;
            let exit_info = run_interactive_tui(
                interactive,
                root_remote.clone(),
                root_remote_auth_token_env.clone(),
                arg0_paths.clone(),
            )
            .await?;
            handle_app_exit(exit_info)?;
        }
        Some(Subcommand::Login(mut login_cli)) => {
            reject_remote_mode_for_subcommand(
                root_remote.as_deref(),
                root_remote_auth_token_env.as_deref(),
                "login",
            )?;
            #[cfg(feature = "ilhae")]
            let use_ilhae_identity_login = is_invoked_as_ilhae_cli();
            #[cfg(not(feature = "ilhae"))]
            let use_ilhae_identity_login = false;

            if use_ilhae_identity_login {
                #[cfg(feature = "ilhae")]
                run_ilhae_login_compat_command(login_cli).await?;
            } else {
                prepend_config_flags(
                    &mut login_cli.config_overrides,
                    root_config_overrides.clone(),
                );
                match login_cli.action {
                    Some(LoginSubcommand::Status) => {
                        run_login_status(login_cli.config_overrides).await;
                    }
                    None => {
                        if login_cli.with_api_key && login_cli.with_access_token {
                            eprintln!(
                                "Choose one login credential source: --with-api-key or --with-access-token."
                            );
                            std::process::exit(1);
                        } else if login_cli.use_device_code {
                            run_login_with_device_code(
                                login_cli.config_overrides,
                                login_cli.issuer_base_url,
                                login_cli.client_id,
                            )
                            .await;
                        } else if login_cli.api_key.is_some() {
                            eprintln!(
                                "The --api-key flag is no longer supported. Pipe the key instead, e.g. `printenv OPENAI_API_KEY | codex login --with-api-key`."
                            );
                            std::process::exit(1);
                        } else if login_cli.with_api_key {
                            let api_key = read_api_key_from_stdin();
                            run_login_with_api_key(login_cli.config_overrides, api_key).await;
                        } else if login_cli.with_access_token {
                            let access_token = read_access_token_from_stdin();
                            run_login_with_access_token(login_cli.config_overrides, access_token)
                                .await;
                        } else {
                            run_login_with_chatgpt(login_cli.config_overrides).await;
                        }
                    }
                }
            }
        }
        Some(Subcommand::Logout(mut logout_cli)) => {
            reject_remote_mode_for_subcommand(
                root_remote.as_deref(),
                root_remote_auth_token_env.as_deref(),
                "logout",
            )?;
            #[cfg(feature = "ilhae")]
            let use_ilhae_identity_logout = is_invoked_as_ilhae_cli();
            #[cfg(not(feature = "ilhae"))]
            let use_ilhae_identity_logout = false;

            if use_ilhae_identity_logout {
                #[cfg(feature = "ilhae")]
                run_ilhae_auth_command(IlhaeAuthCommand {
                    subcommand: IlhaeAuthSubcommand::Logout { json: false },
                })
                .await?;
            } else {
                prepend_config_flags(
                    &mut logout_cli.config_overrides,
                    root_config_overrides.clone(),
                );
                run_logout(logout_cli.config_overrides).await;
            }
        }
        Some(Subcommand::Completion(completion_cli)) => {
            reject_remote_mode_for_subcommand(
                root_remote.as_deref(),
                root_remote_auth_token_env.as_deref(),
                "completion",
            )?;
            print_completion(completion_cli);
        }
        Some(Subcommand::Update) => {
            reject_remote_mode_for_subcommand(
                root_remote.as_deref(),
                root_remote_auth_token_env.as_deref(),
                "update",
            )?;
            run_update_command()?;
        }
        Some(Subcommand::Cloud(mut cloud_cli)) => {
            reject_remote_mode_for_subcommand(
                root_remote.as_deref(),
                root_remote_auth_token_env.as_deref(),
                "cloud",
            )?;
            if interactive.strict_config {
                anyhow::bail!("`--strict-config` is not supported for `codex cloud`");
            }
            prepend_config_flags(
                &mut cloud_cli.config_overrides,
                root_config_overrides.clone(),
            );
            codex_cloud_tasks::run_main(cloud_cli, arg0_paths.codex_linux_sandbox_exe.clone())
                .await?;
        }
        Some(Subcommand::Sandbox(sandbox_args)) => match sandbox_args.cmd {
            SandboxCommand::Macos(mut seatbelt_cli) => {
                reject_remote_mode_for_subcommand(
                    root_remote.as_deref(),
                    root_remote_auth_token_env.as_deref(),
                    "sandbox macos",
                )?;
                prepend_config_flags(
                    &mut seatbelt_cli.config_overrides,
                    root_config_overrides.clone(),
                );
                codex_cli::run_command_under_seatbelt(
                    seatbelt_cli,
                    arg0_paths.codex_linux_sandbox_exe.clone(),
                )
                .await?;
            }
            SandboxCommand::Linux(mut landlock_cli) => {
                reject_remote_mode_for_subcommand(
                    root_remote.as_deref(),
                    root_remote_auth_token_env.as_deref(),
                    "sandbox linux",
                )?;
                prepend_config_flags(
                    &mut landlock_cli.config_overrides,
                    root_config_overrides.clone(),
                );
                codex_cli::run_command_under_landlock(
                    landlock_cli,
                    arg0_paths.codex_linux_sandbox_exe.clone(),
                )
                .await?;
            }
            SandboxCommand::Windows(mut windows_cli) => {
                reject_remote_mode_for_subcommand(
                    root_remote.as_deref(),
                    root_remote_auth_token_env.as_deref(),
                    "sandbox windows",
                )?;
                prepend_config_flags(
                    &mut windows_cli.config_overrides,
                    root_config_overrides.clone(),
                );
                codex_cli::run_command_under_windows(
                    windows_cli,
                    arg0_paths.codex_linux_sandbox_exe.clone(),
                )
                .await?;
            }
        },
        Some(Subcommand::Debug(DebugCommand { subcommand })) => match subcommand {
            DebugSubcommand::Models(cmd) => {
                reject_remote_mode_for_subcommand(
                    root_remote.as_deref(),
                    root_remote_auth_token_env.as_deref(),
                    "debug models",
                )?;
                run_debug_models_command(cmd, root_config_overrides).await?;
            }
            DebugSubcommand::AppServer(cmd) => {
                reject_remote_mode_for_subcommand(
                    root_remote.as_deref(),
                    root_remote_auth_token_env.as_deref(),
                    "debug app-server",
                )?;
                run_debug_app_server_command(cmd).await?;
            }
            DebugSubcommand::PromptInput(cmd) => {
                reject_remote_mode_for_subcommand(
                    root_remote.as_deref(),
                    root_remote_auth_token_env.as_deref(),
                    "debug prompt-input",
                )?;
                run_debug_prompt_input_command(
                    cmd,
                    root_config_overrides,
                    interactive,
                    arg0_paths.clone(),
                )
                .await?;
            }
            DebugSubcommand::TraceReduce(cmd) => {
                reject_remote_mode_for_subcommand(
                    root_remote.as_deref(),
                    root_remote_auth_token_env.as_deref(),
                    "debug trace-reduce",
                )?;
                run_debug_trace_reduce_command(cmd).await?;
            }
            DebugSubcommand::ClearMemories => {
                reject_remote_mode_for_subcommand(
                    root_remote.as_deref(),
                    root_remote_auth_token_env.as_deref(),
                    "debug clear-memories",
                )?;
                run_debug_clear_memories_command(&root_config_overrides, &interactive).await?;
            }
        },
        Some(Subcommand::Execpolicy(ExecpolicyCommand { sub })) => match sub {
            ExecpolicySubcommand::Check(cmd) => {
                reject_remote_mode_for_subcommand(
                    root_remote.as_deref(),
                    root_remote_auth_token_env.as_deref(),
                    "execpolicy check",
                )?;
                run_execpolicycheck(cmd)?
            }
        },
        Some(Subcommand::Internal(InternalArgs { cmd })) => match cmd {
            InternalCommand::GetSessionContext => {
                #[cfg(feature = "ilhae")]
                {
                    tokio::task::spawn_blocking(move || {
                        codex_ilhae::context_proxy::run_get_session_context()
                    })
                    .await??;
                }
            }
        },
        Some(Subcommand::Apply(mut apply_cli)) => {
            reject_remote_mode_for_subcommand(
                root_remote.as_deref(),
                root_remote_auth_token_env.as_deref(),
                "apply",
            )?;
            prepend_config_flags(
                &mut apply_cli.config_overrides,
                root_config_overrides.clone(),
            );
            run_apply_command(apply_cli, /*cwd*/ None).await?;
        }
        Some(Subcommand::ResponsesApiProxy(args)) => {
            reject_remote_mode_for_subcommand(
                root_remote.as_deref(),
                root_remote_auth_token_env.as_deref(),
                "responses-api-proxy",
            )?;
            tokio::task::spawn_blocking(move || codex_responses_api_proxy::run_main(args))
                .await??;
        }
        Some(Subcommand::StdioToUds(cmd)) => {
            reject_remote_mode_for_subcommand(
                root_remote.as_deref(),
                root_remote_auth_token_env.as_deref(),
                "stdio-to-uds",
            )?;
            let socket_path = cmd.socket_path;
            codex_stdio_to_uds::run(socket_path.as_path()).await?;
        }
        Some(Subcommand::ExecServer(cmd)) => {
            reject_remote_mode_for_subcommand(
                root_remote.as_deref(),
                root_remote_auth_token_env.as_deref(),
                "exec-server",
            )?;
            run_exec_server_command(
                cmd,
                &arg0_paths,
                &root_config_overrides,
                interactive.config_profile.clone(),
            )
            .await?;
        }
        Some(Subcommand::Features(FeaturesCli { sub })) => match sub {
            FeaturesSubcommand::List => {
                reject_remote_mode_for_subcommand(
                    root_remote.as_deref(),
                    root_remote_auth_token_env.as_deref(),
                    "features list",
                )?;
                // Respect root-level `-c` overrides plus top-level flags like `--profile`.
                let mut cli_kv_overrides = root_config_overrides
                    .parse_overrides()
                    .map_err(anyhow::Error::msg)?;

                // Honor `--search` via the canonical web_search mode.
                if interactive.web_search {
                    cli_kv_overrides.push((
                        "web_search".to_string(),
                        toml::Value::String("live".to_string()),
                    ));
                }

                // Thread through relevant top-level flags (at minimum, `--profile`).
                let overrides = ConfigOverrides {
                    config_profile: interactive.config_profile.clone(),
                    ..Default::default()
                };

                let config = Config::load_with_cli_overrides_and_harness_overrides(
                    cli_kv_overrides,
                    overrides,
                )
                .await?;
                let mut rows = Vec::with_capacity(FEATURES.len());
                let mut name_width = 0;
                let mut stage_width = 0;
                for def in FEATURES {
                    let name = def.key;
                    let stage = stage_str(def.stage);
                    let enabled = config.features.enabled(def.id);
                    name_width = name_width.max(name.len());
                    stage_width = stage_width.max(stage.len());
                    rows.push((name, stage, enabled));
                }
                rows.sort_unstable_by_key(|(name, _, _)| *name);

                for (name, stage, enabled) in rows {
                    println!("{name:<name_width$}  {stage:<stage_width$}  {enabled}");
                }
            }
            FeaturesSubcommand::Enable(FeatureSetArgs { feature }) => {
                reject_remote_mode_for_subcommand(
                    root_remote.as_deref(),
                    root_remote_auth_token_env.as_deref(),
                    "features enable",
                )?;
                enable_feature_in_config(&interactive, &feature).await?;
            }
            FeaturesSubcommand::Disable(FeatureSetArgs { feature }) => {
                reject_remote_mode_for_subcommand(
                    root_remote.as_deref(),
                    root_remote_auth_token_env.as_deref(),
                    "features disable",
                )?;
                disable_feature_in_config(&interactive, &feature).await?;
            }
        },
    }

    Ok(())
}

#[cfg(feature = "ilhae")]
async fn run_gpu_command(cmd: GpuCommand, profile_id: Option<&str>) -> anyhow::Result<()> {
    let addr_override = cmd.addr;
    let addr = addr_override
        .clone()
        .unwrap_or_else(codex_ilhae::gpu_queue::api::default_listen_addr);
    match cmd.subcommand {
        GpuSubcommand::Daemon(daemon) => {
            let listen = daemon.listen.unwrap_or(addr);
            codex_ilhae::gpu_queue::daemon::run_daemon(
                &listen,
                profile_id.map(ToString::to_string),
            )
            .await?;
        }
        GpuSubcommand::Status { json } => {
            let status = gpu_client(&addr).status().await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                println!("LLM: {:?}", status.llm_state);
                match status.active_lease {
                    Some(lease) => {
                        println!(
                            "Active: {} owner={} kind={} state={:?} expiresAt={:?}",
                            lease.lease_id, lease.owner, lease.kind, lease.state, lease.expires_at
                        );
                    }
                    None => println!("Active: none"),
                }
                println!("Pending: {}", status.pending_leases.len());
            }
        }
        GpuSubcommand::Acquire(acquire) => {
            let request = gpu_lease_request(
                acquire.owner,
                acquire.kind,
                acquire.shared,
                acquire.preempt_llm,
                acquire.ttl_seconds,
                acquire.wait_timeout_seconds,
            );
            let response = gpu_client(&addr).acquire_lease(&request).await?;
            if acquire.json {
                println!("{}", serde_json::to_string_pretty(&response)?);
            } else {
                println!(
                    "Lease: {} state={:?} llmWasPreempted={}",
                    response.lease_id, response.state, response.llm_was_preempted
                );
            }
        }
        GpuSubcommand::Release { lease_id } => {
            let response = gpu_client(&addr).release_lease(&lease_id).await?;
            println!(
                "Released: {} llmRestarted={}",
                response.released.lease_id, response.llm_restarted
            );
            if let Some(promoted) = response.promoted {
                println!("Promoted: {}", promoted.lease_id);
            }
        }
        GpuSubcommand::Run(run) => {
            run_gpu_wrapped_command(&addr, run).await?;
        }
        GpuSubcommand::Llm(llm) => {
            let client = gpu_client(&addr);
            let response = match llm.subcommand {
                GpuLlmSubcommand::Start => client.llm_start().await?,
                GpuLlmSubcommand::Stop => client.llm_stop().await?,
                GpuLlmSubcommand::Restart => client.llm_restart().await?,
            };
            println!("LLM: {:?}", response.state);
        }
        GpuSubcommand::ComfyProxy(proxy) => {
            let overrides = comfy_proxy_overrides(proxy, addr_override);
            let config = codex_ilhae::gpu_queue::comfy_proxy::ComfyProxyConfig::from_sources(
                None, overrides,
            );
            codex_ilhae::gpu_queue::comfy_proxy::run(config).await?;
        }
    }
    Ok(())
}

#[cfg(feature = "ilhae")]
fn comfy_proxy_overrides(
    proxy: GpuComfyProxyCommand,
    addr_override: Option<String>,
) -> codex_ilhae::gpu_queue::comfy_proxy::ComfyProxyConfigOverrides {
    let start_backend_for_passthrough = if proxy.start_backend_for_passthrough {
        Some(true)
    } else if proxy.no_start_backend_for_passthrough {
        Some(false)
    } else {
        None
    };

    codex_ilhae::gpu_queue::comfy_proxy::ComfyProxyConfigOverrides {
        listen: proxy.listen,
        backend_url: proxy.backend_url,
        comfy_root: proxy.comfy_root,
        gpu_queue_addr: addr_override,
        owner: proxy.owner,
        start_command: proxy.start_command,
        stop_command: proxy.stop_command,
        ttl_seconds: proxy.ttl_seconds,
        wait_timeout_seconds: proxy.wait_timeout_seconds,
        prompt_poll_interval_ms: proxy.prompt_poll_interval_ms,
        prompt_timeout_seconds: proxy.prompt_timeout_seconds,
        stop_after_prompt: proxy.no_stop_after_prompt.then_some(false),
        start_backend_for_passthrough,
    }
}

#[cfg(feature = "ilhae")]
async fn run_gpu_wrapped_command(addr: &str, run: GpuRunCommand) -> anyhow::Result<()> {
    let request = gpu_lease_request(
        run.owner,
        run.kind,
        run.shared,
        run.preempt_llm,
        run.ttl_seconds,
        Some(run.wait_timeout_seconds),
    );
    let client = gpu_client(addr);
    let response = client.acquire_lease(&request).await?;
    if response.state != codex_ilhae::gpu_queue::api::LeaseState::Granted {
        anyhow::bail!("GPU lease `{}` was not granted", response.lease_id);
    }

    let mut command = tokio::process::Command::new(&run.command[0]);
    command.args(&run.command[1..]);
    let status = command.status().await;
    let release_result = client.release_lease(&response.lease_id).await;
    if let Err(err) = release_result {
        eprintln!("Failed to release GPU lease `{}`: {err}", response.lease_id);
    }

    let status = status?;
    if !status.success() {
        anyhow::bail!("GPU wrapped command exited with {status}");
    }
    Ok(())
}

#[cfg(feature = "ilhae")]
fn gpu_client(addr: &str) -> codex_ilhae::gpu_queue::client::GpuQueueClient {
    codex_ilhae::gpu_queue::client::GpuQueueClient::from_addr(addr)
}

#[cfg(feature = "ilhae")]
fn gpu_lease_request(
    owner: String,
    kind: String,
    shared: bool,
    preempt_llm: bool,
    ttl_seconds: u64,
    wait_timeout_seconds: Option<u64>,
) -> codex_ilhae::gpu_queue::api::LeaseRequest {
    codex_ilhae::gpu_queue::api::LeaseRequest {
        owner,
        kind,
        mode: if shared {
            codex_ilhae::gpu_queue::api::LeaseMode::Shared
        } else {
            codex_ilhae::gpu_queue::api::LeaseMode::Exclusive
        },
        preempt_llm,
        ttl_seconds,
        wait_timeout_seconds,
    }
}

async fn run_exec_server_command(
    cmd: ExecServerCommand,
    arg0_paths: &Arg0DispatchPaths,
    root_config_overrides: &CliConfigOverrides,
    config_profile: Option<String>,
) -> anyhow::Result<()> {
    let codex_self_exe = arg0_paths
        .codex_self_exe
        .clone()
        .ok_or_else(|| anyhow::anyhow!("Codex executable path is not configured"))?;
    let runtime_paths = codex_exec_server::ExecServerRuntimePaths::new(
        codex_self_exe,
        arg0_paths.codex_linux_sandbox_exe.clone(),
    )?;
    if let Some(base_url) = cmd.remote {
        let executor_id = cmd
            .executor_id
            .ok_or_else(|| anyhow::anyhow!("--executor-id is required when --remote is set"))?;
        let auth_provider = load_exec_server_remote_auth_provider(
            root_config_overrides,
            config_profile,
            cmd.use_agent_identity_auth,
        )
        .await?;
        let mut remote_config =
            codex_exec_server::RemoteExecutorConfig::new(base_url, executor_id, auth_provider)?;
        if let Some(name) = cmd.name {
            remote_config.name = name;
        }
        codex_exec_server::run_remote_executor(remote_config, runtime_paths).await?;
        return Ok(());
    }
    let listen_url = cmd
        .listen
        .as_deref()
        .unwrap_or(codex_exec_server::DEFAULT_LISTEN_URL);
    codex_exec_server::run_main(listen_url, runtime_paths)
        .await
        .map_err(anyhow::Error::from_boxed)
}

async fn load_exec_server_remote_auth_provider(
    root_config_overrides: &CliConfigOverrides,
    config_profile: Option<String>,
    use_agent_identity_auth: bool,
) -> anyhow::Result<codex_api::SharedAuthProvider> {
    let config = load_exec_server_remote_config(root_config_overrides, config_profile).await?;
    if use_agent_identity_auth {
        let agent_identity_jwt = read_codex_access_token_from_env().ok_or_else(|| {
            anyhow::anyhow!("CODEX_ACCESS_TOKEN is required when --use-agent-identity-auth is set")
        })?;
        let auth =
            CodexAuth::from_agent_identity_jwt(&agent_identity_jwt, Some(&config.chatgpt_base_url))
                .await?;
        return Ok(codex_model_provider::auth_provider_from_auth(&auth));
    }

    let auth = load_exec_server_remote_auth(
        &config,
        "remote exec-server registration requires ChatGPT authentication; run `codex login` first",
    )
    .await?;

    if !auth.is_chatgpt_auth() {
        anyhow::bail!(
            "remote exec-server registration requires ChatGPT authentication; API key and Agent Identity auth are not supported"
        );
    }

    Ok(codex_model_provider::auth_provider_from_auth(&auth))
}

async fn load_exec_server_remote_config(
    root_config_overrides: &CliConfigOverrides,
    config_profile: Option<String>,
) -> anyhow::Result<codex_core::config::Config> {
    let cli_kv_overrides = root_config_overrides
        .parse_overrides()
        .map_err(anyhow::Error::msg)?;
    Ok(ConfigBuilder::default()
        .cli_overrides(cli_kv_overrides)
        .harness_overrides(ConfigOverrides {
            config_profile,
            ..Default::default()
        })
        .build()
        .await?)
}

async fn load_exec_server_remote_auth(
    config: &codex_core::config::Config,
    missing_auth_error: &'static str,
) -> anyhow::Result<codex_login::CodexAuth> {
    let auth_manager =
        AuthManager::shared_from_config(config, /*enable_codex_api_key_env*/ true).await;

    let auth = match auth_manager.auth().await {
        Some(auth) => auth,
        None => {
            auth_manager.reload().await;
            auth_manager
                .auth()
                .await
                .ok_or_else(|| anyhow::anyhow!(missing_auth_error))?
        }
    };

    Ok(auth)
}

async fn enable_feature_in_config(interactive: &TuiCli, feature: &str) -> anyhow::Result<()> {
    FeatureToggles::validate_feature(feature)?;
    let codex_home = find_codex_home()?;
    ConfigEditsBuilder::new(&codex_home)
        .with_profile(interactive.config_profile.as_deref())
        .set_feature_enabled(feature, /*enabled*/ true)
        .apply()
        .await?;
    println!("Enabled feature `{feature}` in config.toml.");
    maybe_print_under_development_feature_warning(&codex_home, interactive, feature);
    Ok(())
}

async fn disable_feature_in_config(interactive: &TuiCli, feature: &str) -> anyhow::Result<()> {
    FeatureToggles::validate_feature(feature)?;
    let codex_home = find_codex_home()?;
    ConfigEditsBuilder::new(&codex_home)
        .with_profile(interactive.config_profile.as_deref())
        .set_feature_enabled(feature, /*enabled*/ false)
        .apply()
        .await?;
    println!("Disabled feature `{feature}` in config.toml.");
    Ok(())
}

fn maybe_print_under_development_feature_warning(
    codex_home: &std::path::Path,
    interactive: &TuiCli,
    feature: &str,
) {
    if interactive.config_profile.is_some() {
        return;
    }

    let Some(spec) = FEATURES.iter().find(|spec| spec.key == feature) else {
        return;
    };
    if !matches!(spec.stage, Stage::UnderDevelopment) {
        return;
    }

    let config_path = codex_home.join(codex_config::CONFIG_TOML_FILE);
    eprintln!(
        "Under-development features enabled: {feature}. Under-development features are incomplete and may behave unpredictably. To suppress this warning, set `suppress_unstable_features_warning = true` in {}.",
        config_path.display()
    );
}

async fn run_debug_trace_reduce_command(cmd: DebugTraceReduceCommand) -> anyhow::Result<()> {
    let output = cmd
        .output
        .unwrap_or_else(|| cmd.trace_bundle.join(REDUCED_STATE_FILE_NAME));

    let trace = replay_bundle(&cmd.trace_bundle)?;
    let reduced_json = serde_json::to_vec_pretty(&trace)?;
    tokio::fs::write(&output, reduced_json).await?;
    println!("{}", output.display());

    Ok(())
}

async fn run_debug_prompt_input_command(
    cmd: DebugPromptInputCommand,
    root_config_overrides: CliConfigOverrides,
    interactive: TuiCli,
    arg0_paths: Arg0DispatchPaths,
) -> anyhow::Result<()> {
    let shared = interactive.shared.into_inner();
    let mut cli_kv_overrides = root_config_overrides
        .parse_overrides()
        .map_err(anyhow::Error::msg)?;
    if interactive.web_search {
        cli_kv_overrides.push((
            "web_search".to_string(),
            toml::Value::String("live".to_string()),
        ));
    }

    let approval_policy = if shared.dangerously_bypass_approvals_and_sandbox {
        Some(AskForApproval::Never)
    } else {
        interactive.approval_policy.map(Into::into)
    };
    let sandbox_mode = if shared.dangerously_bypass_approvals_and_sandbox {
        Some(codex_protocol::config_types::SandboxMode::DangerFullAccess)
    } else {
        shared.sandbox_mode.map(Into::into)
    };
    let overrides = ConfigOverrides {
        model: shared.model,
        config_profile: shared.config_profile,
        approval_policy,
        sandbox_mode,
        cwd: shared.cwd,
        codex_self_exe: arg0_paths.codex_self_exe,
        codex_linux_sandbox_exe: arg0_paths.codex_linux_sandbox_exe,
        main_execve_wrapper_exe: arg0_paths.main_execve_wrapper_exe,
        show_raw_agent_reasoning: shared.oss.then_some(true),
        ephemeral: Some(true),
        additional_writable_roots: shared.add_dir,
        ..Default::default()
    };
    let config =
        Config::load_with_cli_overrides_and_harness_overrides(cli_kv_overrides, overrides).await?;

    let mut input = shared
        .images
        .into_iter()
        .chain(cmd.images)
        .map(|path| UserInput::LocalImage { path, detail: None })
        .collect::<Vec<_>>();
    if let Some(prompt) = cmd.prompt.or(interactive.prompt) {
        input.push(UserInput::Text {
            text: prompt.replace("\r\n", "\n").replace('\r', "\n"),
            text_elements: Vec::new(),
        });
    }

    let prompt_input = codex_core::build_prompt_input(config, input, /*state_db*/ None).await?;
    println!("{}", serde_json::to_string_pretty(&prompt_input)?);

    Ok(())
}

async fn run_debug_models_command(
    cmd: DebugModelsCommand,
    root_config_overrides: CliConfigOverrides,
) -> anyhow::Result<()> {
    let catalog = if cmd.bundled {
        bundled_models_response()?
    } else {
        let cli_overrides = root_config_overrides
            .parse_overrides()
            .map_err(anyhow::Error::msg)?;
        let config = Config::load_with_cli_overrides(cli_overrides).await?;
        let auth_manager =
            AuthManager::shared_from_config(&config, /*enable_codex_api_key_env*/ true).await;
        let models_manager = build_models_manager(&config, auth_manager);
        models_manager
            .raw_model_catalog(RefreshStrategy::OnlineIfUncached)
            .await
    };

    serde_json::to_writer(std::io::stdout(), &catalog)?;
    println!();
    Ok(())
}

async fn run_debug_clear_memories_command(
    root_config_overrides: &CliConfigOverrides,
    interactive: &TuiCli,
) -> anyhow::Result<()> {
    let cli_kv_overrides = root_config_overrides
        .parse_overrides()
        .map_err(anyhow::Error::msg)?;
    let overrides = ConfigOverrides {
        config_profile: interactive.config_profile.clone(),
        ..Default::default()
    };
    let config =
        Config::load_with_cli_overrides_and_harness_overrides(cli_kv_overrides, overrides).await?;

    let state_path = state_db_path(config.sqlite_home.as_path());
    let mut cleared_state_db = false;
    if tokio::fs::try_exists(&state_path).await? {
        let state_db =
            StateRuntime::init(config.sqlite_home.clone(), config.model_provider_id.clone())
                .await?;
        state_db.clear_memory_data().await?;
        cleared_state_db = true;
    }

    clear_memory_roots_contents(&config.codex_home).await?;

    let mut message = if cleared_state_db {
        format!("Cleared memory state from {}.", state_path.display())
    } else {
        format!("No state db found at {}.", state_path.display())
    };
    message.push_str(&format!(
        " Cleared memory directories under {}.",
        config.codex_home.display()
    ));

    println!("{message}");

    Ok(())
}

/// Prepend root-level overrides so they have lower precedence than
/// CLI-specific ones specified after the subcommand (if any).
fn prepend_config_flags(
    subcommand_config_overrides: &mut CliConfigOverrides,
    cli_config_overrides: CliConfigOverrides,
) {
    subcommand_config_overrides
        .raw_overrides
        .splice(0..0, cli_config_overrides.raw_overrides);
}

fn reject_remote_mode_for_subcommand(
    remote: Option<&str>,
    remote_auth_token_env: Option<&str>,
    subcommand: &str,
) -> anyhow::Result<()> {
    if let Some(remote) = remote {
        anyhow::bail!(
            "`--remote {remote}` is only supported for interactive TUI commands, not `codex {subcommand}`"
        );
    }
    if remote_auth_token_env.is_some() {
        anyhow::bail!(
            "`--remote-auth-token-env` is only supported for interactive TUI commands, not `codex {subcommand}`"
        );
    }
    Ok(())
}

fn reject_remote_mode_for_app_server_subcommand(
    remote: Option<&str>,
    remote_auth_token_env: Option<&str>,
    subcommand: Option<&AppServerSubcommand>,
) -> anyhow::Result<()> {
    let subcommand_name = match subcommand {
        None => "app-server",
        Some(AppServerSubcommand::Proxy(_)) => "app-server proxy",
        Some(AppServerSubcommand::GenerateTs(_)) => "app-server generate-ts",
        Some(AppServerSubcommand::GenerateJsonSchema(_)) => "app-server generate-json-schema",
        Some(AppServerSubcommand::GenerateInternalJsonSchema(_)) => {
            "app-server generate-internal-json-schema"
        }
    };
    reject_remote_mode_for_subcommand(remote, remote_auth_token_env, subcommand_name)
}

fn read_remote_auth_token_from_env_var_with<F>(
    env_var_name: &str,
    get_var: F,
) -> anyhow::Result<String>
where
    F: FnOnce(&str) -> Result<String, std::env::VarError>,
{
    let auth_token = get_var(env_var_name)
        .map_err(|_| anyhow::anyhow!("environment variable `{env_var_name}` is not set"))?;
    let auth_token = auth_token.trim().to_string();
    if auth_token.is_empty() {
        anyhow::bail!("environment variable `{env_var_name}` is empty");
    }
    Ok(auth_token)
}

fn read_remote_auth_token_from_env_var(env_var_name: &str) -> anyhow::Result<String> {
    read_remote_auth_token_from_env_var_with(env_var_name, |name| std::env::var(name))
}

async fn run_interactive_tui(
    mut interactive: TuiCli,
    remote: Option<String>,
    remote_auth_token_env: Option<String>,
    arg0_paths: Arg0DispatchPaths,
) -> std::io::Result<AppExitInfo> {
    let mut loader_overrides = codex_config::LoaderOverrides::default();
    #[cfg(feature = "ilhae")]
    if remote.is_none() && is_invoked_as_ilhae_cli() {
        let codex_home =
            codex_ilhae::config::prepare_ilhae_codex_home().map_err(std::io::Error::other)?;
        apply_ilhae_codex_home_loader_overrides(&mut loader_overrides, &codex_home);

        let _ = codex_ilhae::bootstrap_ilhae_runtime()
            .await
            .map_err(std::io::Error::other)?;

        codex_ilhae::ensure_native_runtime_for_cli(interactive.config_profile.as_deref())
            .await
            .map_err(std::io::Error::other)?;
    }

    if let Some(prompt) = interactive.prompt.take() {
        // Normalize CRLF/CR to LF so CLI-provided text can't leak `\r` into TUI state.
        interactive.prompt = Some(prompt.replace("\r\n", "\n").replace('\r', "\n"));
    }

    let terminal_info = codex_terminal_detection::terminal_info();
    if terminal_info.name == TerminalName::Dumb {
        if !(std::io::stdin().is_terminal() && std::io::stderr().is_terminal()) {
            return Ok(AppExitInfo::fatal(
                "TERM is set to \"dumb\". Refusing to start the interactive TUI because no terminal is available for a confirmation prompt (stdin/stderr is not a TTY). Run in a supported terminal or unset TERM.",
            ));
        }

        eprintln!(
            "WARNING: TERM is set to \"dumb\". Codex's interactive TUI may not work in this terminal."
        );
        if !confirm("Continue anyway? [y/N]: ")? {
            return Ok(AppExitInfo::fatal(
                "Refusing to start the interactive TUI because TERM is set to \"dumb\". Run in a supported terminal or unset TERM.",
            ));
        }
    }

    let mut normalized_remote = remote
        .as_deref()
        .map(codex_tui::resolve_remote_addr)
        .transpose()
        .map_err(std::io::Error::other)?;
    if let Some(env_var_name) = remote_auth_token_env {
        let Some(endpoint) = normalized_remote.as_mut() else {
            return Ok(AppExitInfo::fatal(
                "`--remote-auth-token-env` requires `--remote`.",
            ));
        };
        if !codex_tui::remote_addr_supports_auth_token(endpoint) {
            return Ok(AppExitInfo::fatal(
                "`--remote-auth-token-env` is only supported for loopback ws:// and wss:// remote app servers.",
            ));
        }
        let token = match read_remote_auth_token_from_env_var(&env_var_name) {
            Ok(token) => token,
            Err(err) => return Ok(AppExitInfo::fatal(err.to_string())),
        };
        match endpoint {
            codex_tui::RemoteAppServerEndpoint::WebSocket { auth_token, .. } => {
                *auth_token = Some(token);
            }
            codex_tui::RemoteAppServerEndpoint::UnixSocket { .. } => {
                return Ok(AppExitInfo::fatal(
                    "`--remote-auth-token-env` is only supported for websocket remote app servers.",
                ));
            }
        }
    }
    #[cfg(feature = "ilhae")]
    let external_notifications =
        is_invoked_as_ilhae_cli().then(codex_ilhae::spawn_app_server_external_notification_bridge);
    #[cfg(not(feature = "ilhae"))]
    let external_notifications = None;
    #[cfg(feature = "ilhae")]
    let runtime_hooks = if normalized_remote.is_none() && is_invoked_as_ilhae_cli() {
        ilhae_app_server_runtime_hooks()
    } else {
        codex_app_server::AppServerRuntimeHooks::default()
    };
    #[cfg(not(feature = "ilhae"))]
    let runtime_hooks = codex_app_server::AppServerRuntimeHooks::default();
    codex_tui::run_main(
        interactive,
        arg0_paths,
        loader_overrides,
        normalized_remote,
        external_notifications,
        runtime_hooks,
    )
    .await
}

fn confirm(prompt: &str) -> std::io::Result<bool> {
    eprintln!("{prompt}");

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let answer = input.trim();
    Ok(answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes"))
}

/// Build the final `TuiCli` for a `codex resume` invocation.
fn finalize_resume_interactive(
    mut interactive: TuiCli,
    root_config_overrides: CliConfigOverrides,
    session_id: Option<String>,
    last: bool,
    show_all: bool,
    include_non_interactive: bool,
    resume_cli: TuiCli,
) -> TuiCli {
    // Start with the parsed interactive CLI so resume shares the same
    // configuration surface area as `codex` without additional flags.
    let resume_session_id = session_id;
    interactive.resume_picker = resume_session_id.is_none() && !last;
    interactive.resume_last = last;
    interactive.resume_session_id = resume_session_id;
    interactive.resume_show_all = show_all;
    interactive.resume_include_non_interactive = include_non_interactive;

    // Merge resume-scoped flags and overrides with highest precedence.
    merge_interactive_cli_flags(&mut interactive, resume_cli);

    // Propagate any root-level config overrides (e.g. `-c key=value`).
    prepend_config_flags(&mut interactive.config_overrides, root_config_overrides);

    interactive
}

/// Build the final `TuiCli` for a `codex fork` invocation.
fn finalize_fork_interactive(
    mut interactive: TuiCli,
    root_config_overrides: CliConfigOverrides,
    session_id: Option<String>,
    last: bool,
    show_all: bool,
    fork_cli: TuiCli,
) -> TuiCli {
    // Start with the parsed interactive CLI so fork shares the same
    // configuration surface area as `codex` without additional flags.
    let fork_session_id = session_id;
    interactive.fork_picker = fork_session_id.is_none() && !last;
    interactive.fork_last = last;
    interactive.fork_session_id = fork_session_id;
    interactive.fork_show_all = show_all;

    // Merge fork-scoped flags and overrides with highest precedence.
    merge_interactive_cli_flags(&mut interactive, fork_cli);

    // Propagate any root-level config overrides (e.g. `-c key=value`).
    prepend_config_flags(&mut interactive.config_overrides, root_config_overrides);

    interactive
}

/// Merge flags provided to `codex resume`/`codex fork` so they take precedence over any
/// root-level flags. Only overrides fields explicitly set on the subcommand-scoped
/// CLI. Also appends `-c key=value` overrides with highest precedence.
fn merge_interactive_cli_flags(interactive: &mut TuiCli, subcommand_cli: TuiCli) {
    let TuiCli {
        shared,
        approval_policy,
        web_search,
        prompt,
        config_overrides,
        ..
    } = subcommand_cli;
    interactive
        .shared
        .apply_subcommand_overrides(shared.into_inner());
    if let Some(approval) = approval_policy {
        interactive.approval_policy = Some(approval);
    }
    if web_search {
        interactive.web_search = true;
    }
    if let Some(prompt) = prompt {
        // Normalize CRLF/CR to LF so CLI-provided text can't leak `\r` into TUI state.
        interactive.prompt = Some(prompt.replace("\r\n", "\n").replace('\r', "\n"));
    }

    interactive
        .config_overrides
        .raw_overrides
        .extend(config_overrides.raw_overrides);
}

fn print_completion(cmd: CompletionCommand) {
    let mut app = MultitoolCli::command();
    let name = "codex";
    generate(cmd.shell, &mut app, name, &mut std::io::stdout());
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use codex_protocol::ThreadId;
    use codex_tui::TokenUsage;
    use pretty_assertions::assert_eq;
    use std::ffi::OsString;

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let previous = std::env::var_os(key);
            // SAFETY: tests scope process env mutations and restore values on drop.
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, previous }
        }

        fn unset(key: &'static str) -> Self {
            let previous = std::env::var_os(key);
            // SAFETY: tests scope process env mutations and restore values on drop.
            unsafe {
                std::env::remove_var(key);
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            // SAFETY: tests restore process env to its previous value before exiting scope.
            unsafe {
                if let Some(previous) = self.previous.as_ref() {
                    std::env::set_var(self.key, previous);
                } else {
                    std::env::remove_var(self.key);
                }
            }
        }
    }

    fn finalize_resume_from_args(args: &[&str]) -> TuiCli {
        let cli = MultitoolCli::try_parse_from(args).expect("parse");
        let MultitoolCli {
            interactive,
            config_overrides: root_overrides,
            subcommand,
            feature_toggles: _,
            remote: _,
        } = cli;

        let Subcommand::Resume(ResumeCommand {
            session_id,
            last,
            all,
            include_non_interactive,
            remote: _,
            config_overrides: resume_cli,
        }) = subcommand.expect("resume present")
        else {
            unreachable!()
        };

        finalize_resume_interactive(
            interactive,
            root_overrides,
            session_id,
            last,
            all,
            include_non_interactive,
            resume_cli,
        )
    }

    fn finalize_fork_from_args(args: &[&str]) -> TuiCli {
        let cli = MultitoolCli::try_parse_from(args).expect("parse");
        let MultitoolCli {
            interactive,
            config_overrides: root_overrides,
            subcommand,
            feature_toggles: _,
            remote: _,
        } = cli;

        let Subcommand::Fork(ForkCommand {
            session_id,
            last,
            all,
            remote: _,
            config_overrides: fork_cli,
        }) = subcommand.expect("fork present")
        else {
            unreachable!()
        };

        finalize_fork_interactive(interactive, root_overrides, session_id, last, all, fork_cli)
    }

    #[cfg(feature = "ilhae")]
    #[test]
    fn native_runtime_provider_name_prefers_explicit_provider() {
        let runtime = codex_ilhae::config::IlhaeProfileNativeRuntimeConfig {
            provider: Some("sglang".to_string()),
            ..Default::default()
        };
        assert_eq!(native_runtime_provider_name(&runtime), "sglang");
    }

    #[cfg(feature = "ilhae")]
    #[test]
    fn native_runtime_provider_name_defaults_to_llama_server() {
        let runtime = codex_ilhae::config::IlhaeProfileNativeRuntimeConfig::default();
        assert_eq!(native_runtime_provider_name(&runtime), "llama-server");
    }

    #[cfg(feature = "ilhae")]
    #[test]
    fn ilhae_cli_startup_prepares_codex_home_before_config_load() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_dir = tmp.path().join(".ilhae");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::write(
            config_dir.join("config.toml"),
            r#"
[profile]
active = "qwen-local"

[profiles.qwen-local.agent]
command = "ilhae"

[profiles.qwen-local.native_runtime]
enabled = true
provider = "llama-server"
base_url = "http://127.0.0.1:8082/v1"
model_path = "/models/Qwen3.6-27B-UD-Q4_K_XL.gguf"
args = ["--ctx-size", "131072"]
"#,
        )
        .expect("write ilhae config");

        let _config_dir_guard = EnvVarGuard::set("ILHAE_CONFIG_DIR", &config_dir);
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path().join("data"));
        let _runtime_guard = EnvVarGuard::set("ILHAE_RUNTIME", "1");
        let _codex_home_guard = EnvVarGuard::unset("CODEX_HOME");

        prepare_ilhae_cli_environment_if_needed().expect("prepare ilhae cli env");

        let codex_home = config_dir.join("codex-home");
        assert_eq!(
            std::env::var_os("CODEX_HOME"),
            Some(codex_home.clone().into())
        );
        let managed = std::fs::read_to_string(codex_home.join("managed_config.toml"))
            .expect("managed config written");
        assert!(managed.contains(r#"profile = "qwen-local""#));
        assert!(managed.contains(r#"model = "Qwen3.6-27B-UD-Q4_K_XL""#));

        let mut loader_overrides = codex_config::LoaderOverrides::default();
        apply_ilhae_codex_home_loader_overrides(&mut loader_overrides, &codex_home);
        assert_eq!(
            loader_overrides.managed_config_path,
            Some(codex_home.join("managed_config.toml"))
        );
    }

    #[cfg(feature = "ilhae")]
    #[test]
    fn ilhae_exec_loop_developer_instructions_honor_agent_cli_overrides() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path());
        let overrides = CliConfigOverrides {
            raw_overrides: vec![
                "agent.kairos_enabled=true".to_string(),
                "agent.self_improvement_enabled=true".to_string(),
                "agent.self_improvement_preset=\"foreground\"".to_string(),
            ],
        };

        let instructions = ilhae_exec_loop_developer_instructions_from_overrides(&overrides)
            .expect("loop instructions should be generated");

        assert!(instructions.contains("ILHAE RUNTIME LOOP STATE"));
        assert!(instructions.contains("Super Loop: enabled"));
        assert!(instructions.contains("Self-improvement: enabled"));
        assert!(instructions.contains("Preset: foreground"));
        assert!(instructions.contains("Keep self-improvement work visible"));
    }

    #[cfg(feature = "ilhae")]
    #[test]
    fn ilhae_exec_foreground_loop_settings_honor_agent_cli_overrides() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _data_dir_guard = EnvVarGuard::set("ILHAE_DATA_DIR", tmp.path());
        let overrides = CliConfigOverrides {
            raw_overrides: vec![
                "agent.kairos_enabled=true".to_string(),
                "agent.self_improvement_enabled=true".to_string(),
                "agent.self_improvement_preset=\"foreground\"".to_string(),
                "agent.knowledge_mode=\"both\"".to_string(),
                "agent.hygiene_mode=\"both\"".to_string(),
            ],
        };

        let settings = ilhae_exec_runtime_settings_from_overrides(&overrides)
            .expect("runtime settings should be generated");

        assert!(settings.agent.kairos_enabled);
        assert!(settings.agent.self_improvement_enabled);
        assert_eq!(settings.agent.self_improvement_preset, "foreground");
        assert_eq!(settings.agent.knowledge_mode, "both");
        assert_eq!(settings.agent.hygiene_mode, "both");
        assert!(ilhae_exec_should_run_foreground_loops(&settings));
    }

    #[cfg(feature = "ilhae")]
    #[test]
    fn ilhae_goal_loop_phase_marks_kairos_as_kairos_loop() {
        let phase = thread_goal_loop_phase_from_ilhae_parts(
            "super_loop:kairos:1779027374405",
            "Running Super Loop",
            codex_ilhae::LoopLifecycleKind::SuperLoop,
        );

        assert_eq!(phase, codex_state::ThreadGoalLoopPhase::KairosLoop);
    }

    #[test]
    fn exec_resume_last_accepts_prompt_positional() {
        let cli =
            MultitoolCli::try_parse_from(["codex", "exec", "--json", "resume", "--last", "2+2"])
                .expect("parse should succeed");

        let Some(Subcommand::Exec(exec)) = cli.subcommand else {
            panic!("expected exec subcommand");
        };
        let Some(codex_exec::Command::Resume(args)) = exec.command else {
            panic!("expected exec resume");
        };

        assert!(args.last);
        assert_eq!(args.session_id, None);
        assert_eq!(args.prompt.as_deref(), Some("2+2"));
    }

    #[test]
    fn exec_resume_accepts_output_last_message_flag_after_subcommand() {
        let cli = MultitoolCli::try_parse_from([
            "codex",
            "exec",
            "resume",
            "session-123",
            "-o",
            "/tmp/resume-output.md",
            "re-review",
        ])
        .expect("parse should succeed");

        let Some(Subcommand::Exec(exec)) = cli.subcommand else {
            panic!("expected exec subcommand");
        };
        let Some(codex_exec::Command::Resume(args)) = exec.command else {
            panic!("expected exec resume");
        };

        assert_eq!(
            exec.last_message_file,
            Some(std::path::PathBuf::from("/tmp/resume-output.md"))
        );
        assert_eq!(args.session_id.as_deref(), Some("session-123"));
        assert_eq!(args.prompt.as_deref(), Some("re-review"));
    }

    #[test]
    fn dangerous_bypass_conflicts_with_approval_policy() {
        let err = MultitoolCli::try_parse_from([
            "codex",
            "--dangerously-bypass-approvals-and-sandbox",
            "--ask-for-approval",
            "on-request",
        ])
        .expect_err("conflicting permission flags should be rejected");

        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    fn app_server_from_args(args: &[&str]) -> AppServerCommand {
        let cli = MultitoolCli::try_parse_from(args).expect("parse");
        let Subcommand::AppServer(app_server) = cli.subcommand.expect("app-server present") else {
            unreachable!()
        };
        app_server
    }

    fn default_app_server_socket_path() -> AbsolutePathBuf {
        let codex_home = find_codex_home().expect("codex home");
        codex_app_server::app_server_control_socket_path(&codex_home)
            .expect("default app-server socket path")
    }

    #[test]
    fn debug_prompt_input_parses_prompt_and_images() {
        let cli = MultitoolCli::try_parse_from([
            "codex",
            "debug",
            "prompt-input",
            "hello",
            "--image",
            "/tmp/a.png,/tmp/b.png",
        ])
        .expect("parse");

        let Some(Subcommand::Debug(DebugCommand {
            subcommand: DebugSubcommand::PromptInput(cmd),
        })) = cli.subcommand
        else {
            panic!("expected debug prompt-input subcommand");
        };

        assert_eq!(cmd.prompt.as_deref(), Some("hello"));
        assert_eq!(
            cmd.images,
            vec![PathBuf::from("/tmp/a.png"), PathBuf::from("/tmp/b.png")]
        );
    }

    #[test]
    fn debug_models_parses_bundled_flag() {
        let cli =
            MultitoolCli::try_parse_from(["codex", "debug", "models", "--bundled"]).expect("parse");

        let Some(Subcommand::Debug(DebugCommand {
            subcommand: DebugSubcommand::Models(cmd),
        })) = cli.subcommand
        else {
            panic!("expected debug models subcommand");
        };

        assert!(cmd.bundled);
    }

    #[test]
    fn responses_subcommand_is_not_registered() {
        let command = MultitoolCli::command();
        assert!(
            command
                .get_subcommands()
                .all(|subcommand| subcommand.get_name() != "responses")
        );
    }

    fn help_from_args(args: &[&str]) -> String {
        let err = MultitoolCli::try_parse_from(args).expect_err("help should short-circuit");
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
        err.to_string()
    }

    #[test]
    fn plugin_marketplace_help_uses_plugin_namespace() {
        let help = help_from_args(&["codex", "plugin", "marketplace", "--help"]);
        assert!(
            help.contains("Usage: codex plugin marketplace [OPTIONS] <COMMAND>"),
            "{help}"
        );

        for (subcommand, usage) in [
            ("add", "Usage: codex plugin marketplace add"),
            ("upgrade", "Usage: codex plugin marketplace upgrade"),
            ("remove", "Usage: codex plugin marketplace remove"),
        ] {
            let help = help_from_args(&["codex", "plugin", "marketplace", subcommand, "--help"]);
            assert!(help.contains(usage), "{help}");
        }
    }

    #[test]
    fn plugin_marketplace_add_parses_under_plugin() {
        let cli =
            MultitoolCli::try_parse_from(["codex", "plugin", "marketplace", "add", "owner/repo"])
                .expect("parse");

        assert!(matches!(cli.subcommand, Some(Subcommand::Plugin(_))));
    }

    #[test]
    fn plugin_marketplace_upgrade_parses_under_plugin() {
        let cli =
            MultitoolCli::try_parse_from(["codex", "plugin", "marketplace", "upgrade", "debug"])
                .expect("parse");

        assert!(matches!(cli.subcommand, Some(Subcommand::Plugin(_))));
    }

    #[test]
    fn update_parses_as_update_subcommand() {
        let cli = MultitoolCli::try_parse_from(["codex", "update"]).expect("parse");
        assert!(matches!(cli.subcommand, Some(Subcommand::Update)));
    }

    #[test]
    fn sandbox_macos_parses_permissions_profile() {
        let cli = MultitoolCli::try_parse_from([
            "codex",
            "sandbox",
            "macos",
            "--permissions-profile",
            ":workspace",
            "--",
            "echo",
        ])
        .expect("parse");

        let Some(Subcommand::Sandbox(SandboxArgs {
            cmd: SandboxCommand::Macos(command),
        })) = cli.subcommand
        else {
            panic!("expected sandbox macos command");
        };

        assert_eq!(command.permissions_profile.as_deref(), Some(":workspace"));
        assert_eq!(command.command, vec!["echo"]);
    }

    #[test]
    fn sandbox_macos_rejects_explicit_profile_controls_without_profile() {
        let err = MultitoolCli::try_parse_from(["codex", "sandbox", "macos", "-C", "/tmp"])
            .expect_err("parse should fail");

        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn plugin_marketplace_remove_parses_under_plugin() {
        let cli =
            MultitoolCli::try_parse_from(["codex", "plugin", "marketplace", "remove", "debug"])
                .expect("parse");

        assert!(matches!(cli.subcommand, Some(Subcommand::Plugin(_))));
    }

    #[test]
    fn marketplace_no_longer_parses_at_top_level() {
        let add_result =
            MultitoolCli::try_parse_from(["codex", "marketplace", "add", "owner/repo"]);
        assert!(add_result.is_err());

        let upgrade_result =
            MultitoolCli::try_parse_from(["codex", "marketplace", "upgrade", "debug"]);
        assert!(upgrade_result.is_err());

        let remove_result =
            MultitoolCli::try_parse_from(["codex", "marketplace", "remove", "debug"]);
        assert!(remove_result.is_err());
    }

    #[test]
    fn full_auto_no_longer_parses_at_top_level() {
        let result = MultitoolCli::try_parse_from(["codex", "--full-auto"]);

        assert!(result.is_err());
    }

    #[test]
    fn exec_full_auto_reports_migration_path() {
        let cli = MultitoolCli::try_parse_from(["codex", "exec", "--full-auto", "summarize"])
            .expect("exec should accept removed flag long enough to report a migration path");
        let Some(Subcommand::Exec(exec)) = cli.subcommand else {
            panic!("expected exec subcommand");
        };

        assert_eq!(
            exec.removed_full_auto_warning(),
            Some("warning: `--full-auto` is deprecated; use `--sandbox workspace-write` instead.")
        );
    }

    #[test]
    fn sandbox_full_auto_no_longer_parses() {
        let result =
            MultitoolCli::try_parse_from(["codex", "sandbox", "linux", "--full-auto", "--"]);

        assert!(result.is_err());
    }

    fn sample_exit_info(conversation_id: Option<&str>, thread_name: Option<&str>) -> AppExitInfo {
        let token_usage = TokenUsage {
            output_tokens: 2,
            total_tokens: 2,
            ..Default::default()
        };
        AppExitInfo {
            token_usage,
            thread_id: conversation_id
                .map(ThreadId::from_string)
                .map(Result::unwrap),
            thread_name: thread_name.map(str::to_string),
            update_action: None,
            exit_reason: ExitReason::UserRequested,
        }
    }

    #[test]
    fn format_exit_messages_skips_zero_usage() {
        let exit_info = AppExitInfo {
            token_usage: TokenUsage::default(),
            thread_id: None,
            thread_name: None,
            update_action: None,
            exit_reason: ExitReason::UserRequested,
        };
        let lines = format_exit_messages(exit_info, /*color_enabled*/ false);
        assert!(lines.is_empty());
    }

    #[test]
    fn format_exit_messages_includes_resume_hint_without_color() {
        let exit_info = sample_exit_info(
            Some("123e4567-e89b-12d3-a456-426614174000"),
            /*thread_name*/ None,
        );
        let lines = format_exit_messages(exit_info, /*color_enabled*/ false);
        assert_eq!(
            lines,
            vec![
                "Token usage: total=2 input=0 output=2".to_string(),
                "To continue this session, run codex resume 123e4567-e89b-12d3-a456-426614174000"
                    .to_string(),
            ]
        );
    }

    #[test]
    fn format_exit_messages_applies_color_when_enabled() {
        let exit_info = sample_exit_info(
            Some("123e4567-e89b-12d3-a456-426614174000"),
            /*thread_name*/ None,
        );
        let lines = format_exit_messages(exit_info, /*color_enabled*/ true);
        assert_eq!(lines.len(), 2);
        assert!(lines[1].contains("\u{1b}[36m"));
    }

    #[test]
    fn format_exit_messages_uses_id_even_when_thread_has_name() {
        let exit_info = sample_exit_info(
            Some("123e4567-e89b-12d3-a456-426614174000"),
            Some("my-thread"),
        );
        let lines = format_exit_messages(exit_info, /*color_enabled*/ false);
        assert_eq!(
            lines,
            vec![
                "Token usage: total=2 input=0 output=2".to_string(),
                "To continue this session, run codex resume 123e4567-e89b-12d3-a456-426614174000"
                    .to_string(),
            ]
        );
    }

    #[test]
    fn resume_model_flag_applies_when_no_root_flags() {
        let interactive =
            finalize_resume_from_args(["codex", "resume", "-m", "gpt-5.1-test"].as_ref());

        assert_eq!(interactive.model.as_deref(), Some("gpt-5.1-test"));
        assert!(interactive.resume_picker);
        assert!(!interactive.resume_last);
        assert_eq!(interactive.resume_session_id, None);
    }

    #[test]
    fn resume_picker_logic_none_and_not_last() {
        let interactive = finalize_resume_from_args(["codex", "resume"].as_ref());
        assert!(interactive.resume_picker);
        assert!(!interactive.resume_last);
        assert_eq!(interactive.resume_session_id, None);
        assert!(!interactive.resume_show_all);
    }

    #[test]
    fn resume_picker_logic_last() {
        let interactive = finalize_resume_from_args(["codex", "resume", "--last"].as_ref());
        assert!(!interactive.resume_picker);
        assert!(interactive.resume_last);
        assert_eq!(interactive.resume_session_id, None);
        assert!(!interactive.resume_show_all);
    }

    #[test]
    fn resume_picker_logic_with_session_id() {
        let interactive = finalize_resume_from_args(["codex", "resume", "1234"].as_ref());
        assert!(!interactive.resume_picker);
        assert!(!interactive.resume_last);
        assert_eq!(interactive.resume_session_id.as_deref(), Some("1234"));
        assert!(!interactive.resume_show_all);
    }

    #[test]
    fn resume_all_flag_sets_show_all() {
        let interactive = finalize_resume_from_args(["codex", "resume", "--all"].as_ref());
        assert!(interactive.resume_picker);
        assert!(interactive.resume_show_all);
    }

    #[test]
    fn resume_include_non_interactive_flag_sets_source_filter_override() {
        let interactive =
            finalize_resume_from_args(["codex", "resume", "--include-non-interactive"].as_ref());

        assert!(interactive.resume_picker);
        assert!(interactive.resume_include_non_interactive);
    }

    #[test]
    fn resume_merges_option_flags() {
        let interactive = finalize_resume_from_args(
            [
                "codex",
                "resume",
                "sid",
                "--oss",
                "--search",
                "--sandbox",
                "workspace-write",
                "--ask-for-approval",
                "on-request",
                "-m",
                "gpt-5.1-test",
                "-p",
                "my-profile",
                "-C",
                "/tmp",
                "-i",
                "/tmp/a.png,/tmp/b.png",
            ]
            .as_ref(),
        );

        assert_eq!(interactive.model.as_deref(), Some("gpt-5.1-test"));
        assert!(interactive.oss);
        assert_eq!(interactive.config_profile.as_deref(), Some("my-profile"));
        assert_matches!(
            interactive.sandbox_mode,
            Some(codex_utils_cli::SandboxModeCliArg::WorkspaceWrite)
        );
        assert_matches!(
            interactive.approval_policy,
            Some(codex_utils_cli::ApprovalModeCliArg::OnRequest)
        );
        assert_eq!(
            interactive.cwd.as_deref(),
            Some(std::path::Path::new("/tmp"))
        );
        assert!(interactive.web_search);
        let has_a = interactive
            .images
            .iter()
            .any(|p| p == std::path::Path::new("/tmp/a.png"));
        let has_b = interactive
            .images
            .iter()
            .any(|p| p == std::path::Path::new("/tmp/b.png"));
        assert!(has_a && has_b);
        assert!(!interactive.resume_picker);
        assert!(!interactive.resume_last);
        assert_eq!(interactive.resume_session_id.as_deref(), Some("sid"));
    }

    #[test]
    fn resume_merges_dangerously_bypass_flag() {
        let interactive = finalize_resume_from_args(
            [
                "codex",
                "resume",
                "--dangerously-bypass-approvals-and-sandbox",
            ]
            .as_ref(),
        );
        assert!(interactive.dangerously_bypass_approvals_and_sandbox);
        assert!(interactive.resume_picker);
        assert!(!interactive.resume_last);
        assert_eq!(interactive.resume_session_id, None);
    }

    #[test]
    fn fork_picker_logic_none_and_not_last() {
        let interactive = finalize_fork_from_args(["codex", "fork"].as_ref());
        assert!(interactive.fork_picker);
        assert!(!interactive.fork_last);
        assert_eq!(interactive.fork_session_id, None);
        assert!(!interactive.fork_show_all);
    }

    #[test]
    fn fork_picker_logic_last() {
        let interactive = finalize_fork_from_args(["codex", "fork", "--last"].as_ref());
        assert!(!interactive.fork_picker);
        assert!(interactive.fork_last);
        assert_eq!(interactive.fork_session_id, None);
        assert!(!interactive.fork_show_all);
    }

    #[test]
    fn fork_picker_logic_with_session_id() {
        let interactive = finalize_fork_from_args(["codex", "fork", "1234"].as_ref());
        assert!(!interactive.fork_picker);
        assert!(!interactive.fork_last);
        assert_eq!(interactive.fork_session_id.as_deref(), Some("1234"));
        assert!(!interactive.fork_show_all);
    }

    #[test]
    fn fork_all_flag_sets_show_all() {
        let interactive = finalize_fork_from_args(["codex", "fork", "--all"].as_ref());
        assert!(interactive.fork_picker);
        assert!(interactive.fork_show_all);
    }

    #[test]
    fn app_server_analytics_default_disabled_without_flag() {
        let app_server = app_server_from_args(["codex", "app-server"].as_ref());
        assert!(!app_server.analytics_default_enabled);
        assert_eq!(
            app_server.listen,
            codex_app_server::AppServerTransport::Stdio
        );
    }

    #[test]
    fn app_server_analytics_default_enabled_with_flag() {
        let app_server =
            app_server_from_args(["codex", "app-server", "--analytics-default-enabled"].as_ref());
        assert!(app_server.analytics_default_enabled);
    }

    #[test]
    fn remote_flag_parses_for_interactive_root() {
        let cli = MultitoolCli::try_parse_from(["codex", "--remote", "ws://127.0.0.1:4500"])
            .expect("parse");
        assert_eq!(cli.remote.remote.as_deref(), Some("ws://127.0.0.1:4500"));
    }

    #[test]
    fn remote_auth_token_env_flag_parses_for_interactive_root() {
        let cli = MultitoolCli::try_parse_from([
            "codex",
            "--remote-auth-token-env",
            "CODEX_REMOTE_AUTH_TOKEN",
            "--remote",
            "ws://127.0.0.1:4500",
        ])
        .expect("parse");
        assert_eq!(
            cli.remote.remote_auth_token_env.as_deref(),
            Some("CODEX_REMOTE_AUTH_TOKEN")
        );
    }

    #[test]
    fn remote_flag_parses_for_resume_subcommand() {
        let cli =
            MultitoolCli::try_parse_from(["codex", "resume", "--remote", "ws://127.0.0.1:4500"])
                .expect("parse");
        let Subcommand::Resume(ResumeCommand { remote, .. }) =
            cli.subcommand.expect("resume present")
        else {
            panic!("expected resume subcommand");
        };
        assert_eq!(remote.remote.as_deref(), Some("ws://127.0.0.1:4500"));
    }

    #[test]
    fn reject_remote_mode_for_non_interactive_subcommands() {
        let err = reject_remote_mode_for_subcommand(
            Some("127.0.0.1:4500"),
            /*remote_auth_token_env*/ None,
            "exec",
        )
        .expect_err("non-interactive subcommands should reject --remote");
        assert!(
            err.to_string()
                .contains("only supported for interactive TUI commands")
        );
    }

    #[test]
    fn reject_remote_auth_token_env_for_non_interactive_subcommands() {
        let err = reject_remote_mode_for_subcommand(
            /*remote*/ None,
            Some("CODEX_REMOTE_AUTH_TOKEN"),
            "exec",
        )
        .expect_err("non-interactive subcommands should reject --remote-auth-token-env");
        assert!(
            err.to_string()
                .contains("only supported for interactive TUI commands")
        );
    }

    #[test]
    fn reject_remote_auth_token_env_for_app_server_generate_internal_json_schema() {
        let subcommand =
            AppServerSubcommand::GenerateInternalJsonSchema(GenerateInternalJsonSchemaCommand {
                out_dir: PathBuf::from("/tmp/out"),
            });
        let err = reject_remote_mode_for_app_server_subcommand(
            /*remote*/ None,
            Some("CODEX_REMOTE_AUTH_TOKEN"),
            Some(&subcommand),
        )
        .expect_err("non-interactive app-server subcommands should reject --remote-auth-token-env");
        assert!(err.to_string().contains("generate-internal-json-schema"));
    }

    #[cfg(feature = "ilhae")]
    #[test]
    fn ilhae_auth_status_subcommand_parses() {
        let cli = MultitoolCli::try_parse_from(["codex", "auth", "status", "--json"])
            .expect("auth status parses");

        assert!(matches!(
            cli.subcommand,
            Some(Subcommand::Auth(IlhaeAuthCommand {
                subcommand: IlhaeAuthSubcommand::Status { json: true }
            }))
        ));
    }

    #[cfg(feature = "ilhae")]
    #[test]
    fn ilhae_auth_login_subcommand_parses() {
        let cli = MultitoolCli::try_parse_from([
            "codex",
            "auth",
            "login",
            "--issuer",
            "https://auth.example.test",
            "--client-id",
            "custom-cli",
            "--no-browser",
        ])
        .expect("auth login parses");

        let Some(Subcommand::Auth(IlhaeAuthCommand {
            subcommand:
                IlhaeAuthSubcommand::Login {
                    issuer,
                    client_id,
                    no_browser,
                    json,
                },
        })) = cli.subcommand
        else {
            panic!("expected auth login subcommand");
        };
        assert_eq!(issuer.as_deref(), Some("https://auth.example.test"));
        assert_eq!(client_id.as_deref(), Some("custom-cli"));
        assert!(no_browser);
        assert!(!json);
    }

    #[test]
    fn read_remote_auth_token_from_env_var_reports_missing_values() {
        let err = read_remote_auth_token_from_env_var_with("CODEX_REMOTE_AUTH_TOKEN", |_| {
            Err(std::env::VarError::NotPresent)
        })
        .expect_err("missing env vars should be rejected");
        assert!(err.to_string().contains("is not set"));
    }

    #[test]
    fn read_remote_auth_token_from_env_var_trims_values() {
        let auth_token =
            read_remote_auth_token_from_env_var_with("CODEX_REMOTE_AUTH_TOKEN", |_| {
                Ok("  bearer-token  ".to_string())
            })
            .expect("env var should parse");
        assert_eq!(auth_token, "bearer-token");
    }

    #[test]
    fn read_remote_auth_token_from_env_var_rejects_empty_values() {
        let err = read_remote_auth_token_from_env_var_with("CODEX_REMOTE_AUTH_TOKEN", |_| {
            Ok(" \n\t ".to_string())
        })
        .expect_err("empty env vars should be rejected");
        assert!(err.to_string().contains("is empty"));
    }

    #[test]
    fn app_server_listen_websocket_url_parses() {
        let app_server = app_server_from_args(
            ["codex", "app-server", "--listen", "ws://127.0.0.1:4500"].as_ref(),
        );
        assert_eq!(
            app_server.listen,
            codex_app_server::AppServerTransport::WebSocket {
                bind_address: "127.0.0.1:4500".parse().expect("valid socket address"),
            }
        );
    }

    #[test]
    fn app_server_listen_stdio_url_parses() {
        let app_server =
            app_server_from_args(["codex", "app-server", "--listen", "stdio://"].as_ref());
        assert_eq!(
            app_server.listen,
            codex_app_server::AppServerTransport::Stdio
        );
    }

    #[test]
    fn app_server_listen_unix_socket_url_parses() {
        let app_server =
            app_server_from_args(["codex", "app-server", "--listen", "unix://"].as_ref());
        assert_eq!(
            app_server.listen,
            codex_app_server::AppServerTransport::UnixSocket {
                socket_path: default_app_server_socket_path()
            }
        );
    }

    #[test]
    fn app_server_listen_unix_socket_path_parses() {
        let app_server = app_server_from_args(
            ["codex", "app-server", "--listen", "unix:///tmp/codex.sock"].as_ref(),
        );
        assert_eq!(
            app_server.listen,
            codex_app_server::AppServerTransport::UnixSocket {
                socket_path: AbsolutePathBuf::from_absolute_path("/tmp/codex.sock")
                    .expect("absolute path should parse")
            }
        );
    }

    #[test]
    fn app_server_listen_off_parses() {
        let app_server = app_server_from_args(["codex", "app-server", "--listen", "off"].as_ref());
        assert_eq!(app_server.listen, codex_app_server::AppServerTransport::Off);
    }

    #[test]
    fn app_server_listen_invalid_url_fails_to_parse() {
        let parse_result =
            MultitoolCli::try_parse_from(["codex", "app-server", "--listen", "http://foo"]);
        assert!(parse_result.is_err());
    }

    #[test]
    fn app_server_proxy_subcommand_parses() {
        let app_server = app_server_from_args(["codex", "app-server", "proxy"].as_ref());
        assert!(matches!(
            app_server.subcommand,
            Some(AppServerSubcommand::Proxy(AppServerProxyCommand {
                socket_path: None
            }))
        ));
    }

    #[test]
    fn app_server_proxy_sock_path_parses() {
        let app_server =
            app_server_from_args(["codex", "app-server", "proxy", "--sock", "codex.sock"].as_ref());
        let Some(AppServerSubcommand::Proxy(proxy)) = app_server.subcommand else {
            panic!("expected proxy subcommand");
        };
        assert_eq!(
            proxy.socket_path,
            Some(
                AbsolutePathBuf::relative_to_current_dir("codex.sock")
                    .expect("relative path should resolve")
            )
        );
    }

    #[test]
    fn reject_remote_auth_token_env_for_app_server_proxy() {
        let subcommand = AppServerSubcommand::Proxy(AppServerProxyCommand { socket_path: None });
        let err = reject_remote_mode_for_app_server_subcommand(
            /*remote*/ None,
            Some("CODEX_REMOTE_AUTH_TOKEN"),
            Some(&subcommand),
        )
        .expect_err("app-server proxy should reject --remote-auth-token-env");
        assert!(err.to_string().contains("app-server proxy"));
    }

    #[test]
    fn app_server_capability_token_flags_parse() {
        let app_server = app_server_from_args(
            [
                "codex",
                "app-server",
                "--ws-auth",
                "capability-token",
                "--ws-token-file",
                "/tmp/codex-token",
            ]
            .as_ref(),
        );
        assert_eq!(
            app_server.auth.ws_auth,
            Some(codex_app_server::WebsocketAuthCliMode::CapabilityToken)
        );
        assert_eq!(
            app_server.auth.ws_token_file,
            Some(PathBuf::from("/tmp/codex-token"))
        );
    }

    #[test]
    fn app_server_signed_bearer_flags_parse() {
        let app_server = app_server_from_args(
            [
                "codex",
                "app-server",
                "--ws-auth",
                "signed-bearer-token",
                "--ws-shared-secret-file",
                "/tmp/codex-secret",
                "--ws-issuer",
                "issuer",
                "--ws-audience",
                "audience",
                "--ws-max-clock-skew-seconds",
                "9",
            ]
            .as_ref(),
        );
        assert_eq!(
            app_server.auth.ws_auth,
            Some(codex_app_server::WebsocketAuthCliMode::SignedBearerToken)
        );
        assert_eq!(
            app_server.auth.ws_shared_secret_file,
            Some(PathBuf::from("/tmp/codex-secret"))
        );
        assert_eq!(app_server.auth.ws_issuer.as_deref(), Some("issuer"));
        assert_eq!(app_server.auth.ws_audience.as_deref(), Some("audience"));
        assert_eq!(app_server.auth.ws_max_clock_skew_seconds, Some(9));
    }

    #[test]
    fn app_server_rejects_removed_insecure_non_loopback_flag() {
        let parse_result = MultitoolCli::try_parse_from([
            "codex",
            "app-server",
            "--allow-unauthenticated-non-loopback-ws",
        ]);
        assert!(parse_result.is_err());
    }

    #[test]
    fn features_enable_parses_feature_name() {
        let cli = MultitoolCli::try_parse_from(["codex", "features", "enable", "unified_exec"])
            .expect("parse should succeed");
        let Some(Subcommand::Features(FeaturesCli { sub })) = cli.subcommand else {
            panic!("expected features subcommand");
        };
        let FeaturesSubcommand::Enable(FeatureSetArgs { feature }) = sub else {
            panic!("expected features enable");
        };
        assert_eq!(feature, "unified_exec");
    }

    #[test]
    fn features_disable_parses_feature_name() {
        let cli = MultitoolCli::try_parse_from(["codex", "features", "disable", "shell_tool"])
            .expect("parse should succeed");
        let Some(Subcommand::Features(FeaturesCli { sub })) = cli.subcommand else {
            panic!("expected features subcommand");
        };
        let FeaturesSubcommand::Disable(FeatureSetArgs { feature }) = sub else {
            panic!("expected features disable");
        };
        assert_eq!(feature, "shell_tool");
    }

    #[cfg(feature = "ilhae")]
    #[test]
    fn gpu_run_parses_command_after_separator() {
        let cli = MultitoolCli::try_parse_from([
            "codex",
            "gpu",
            "run",
            "--kind",
            "video",
            "--preempt-llm",
            "--",
            "bash",
            "-lc",
            "echo ok",
        ])
        .expect("parse should succeed");
        let Some(Subcommand::Gpu(GpuCommand {
            subcommand: GpuSubcommand::Run(cmd),
            ..
        })) = cli.subcommand
        else {
            panic!("expected gpu run subcommand");
        };

        assert_eq!(cmd.kind, "video");
        assert!(cmd.preempt_llm);
        assert_eq!(cmd.command, vec!["bash", "-lc", "echo ok"]);
    }

    #[cfg(feature = "ilhae")]
    #[test]
    fn gpu_llm_restart_parses() {
        let cli = MultitoolCli::try_parse_from(["codex", "gpu", "llm", "restart"])
            .expect("parse should succeed");
        let Some(Subcommand::Gpu(GpuCommand {
            subcommand:
                GpuSubcommand::Llm(GpuLlmCommand {
                    subcommand: GpuLlmSubcommand::Restart,
                }),
            ..
        })) = cli.subcommand
        else {
            panic!("expected gpu llm restart subcommand");
        };
    }

    #[cfg(feature = "ilhae")]
    #[test]
    fn gpu_comfy_proxy_parses() {
        let cli = MultitoolCli::try_parse_from([
            "codex",
            "gpu",
            "--addr",
            "127.0.0.1:43290",
            "comfy-proxy",
            "--listen",
            "127.0.0.1:8189",
            "--backend-url",
            "http://127.0.0.1:8188",
            "--comfy-root",
            "/tmp/comfy",
            "--no-stop-after-prompt",
            "--start-backend-for-passthrough",
        ])
        .expect("parse should succeed");
        let Some(Subcommand::Gpu(GpuCommand {
            addr,
            subcommand: GpuSubcommand::ComfyProxy(cmd),
        })) = cli.subcommand
        else {
            panic!("expected gpu comfy-proxy subcommand");
        };

        assert_eq!(addr.as_deref(), Some("127.0.0.1:43290"));
        assert_eq!(cmd.listen.as_deref(), Some("127.0.0.1:8189"));
        assert_eq!(cmd.backend_url.as_deref(), Some("http://127.0.0.1:8188"));
        assert_eq!(cmd.comfy_root, Some(PathBuf::from("/tmp/comfy")));
        assert!(cmd.no_stop_after_prompt);
        assert!(cmd.start_backend_for_passthrough);
    }

    #[test]
    fn feature_toggles_known_features_generate_overrides() {
        let toggles = FeatureToggles {
            enable: vec!["web_search_request".to_string()],
            disable: vec!["unified_exec".to_string()],
        };
        let overrides = toggles.to_overrides().expect("valid features");
        assert_eq!(
            overrides,
            vec![
                "features.web_search_request=true".to_string(),
                "features.unified_exec=false".to_string(),
            ]
        );
    }

    #[test]
    fn feature_toggles_accept_legacy_linux_sandbox_flag() {
        let toggles = FeatureToggles {
            enable: vec!["use_linux_sandbox_bwrap".to_string()],
            disable: Vec::new(),
        };
        let overrides = toggles.to_overrides().expect("valid features");
        assert_eq!(
            overrides,
            vec!["features.use_linux_sandbox_bwrap=true".to_string(),]
        );
    }

    #[test]
    fn feature_toggles_accept_removed_image_detail_original_flag() {
        let toggles = FeatureToggles {
            enable: vec!["image_detail_original".to_string()],
            disable: Vec::new(),
        };
        let overrides = toggles.to_overrides().expect("valid features");
        assert_eq!(
            overrides,
            vec!["features.image_detail_original=true".to_string(),]
        );
    }

    #[test]
    fn feature_toggles_unknown_feature_errors() {
        let toggles = FeatureToggles {
            enable: vec!["does_not_exist".to_string()],
            disable: Vec::new(),
        };
        let err = toggles
            .to_overrides()
            .expect_err("feature should be rejected");
        assert_eq!(err.to_string(), "Unknown feature flag: does_not_exist");
    }
}
