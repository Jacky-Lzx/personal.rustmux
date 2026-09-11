use clap::builder::styling::{Color, RgbColor, Style, Styles};
use clap::{ArgGroup, Args, Parser, Subcommand};

const fn mocha(rgb: RgbColor) -> Style {
    Style::new().fg_color(Some(Color::Rgb(rgb)))
}

const CATPPUCCIN_MOCHA_HELP: Styles = Styles::styled()
    .header(mocha(RgbColor(166, 227, 161)).bold())
    .usage(mocha(RgbColor(203, 166, 247)).bold())
    .literal(mocha(RgbColor(137, 180, 250)).bold())
    .placeholder(mocha(RgbColor(250, 179, 135)))
    .error(mocha(RgbColor(243, 139, 168)).bold())
    .valid(mocha(RgbColor(166, 227, 161)))
    .invalid(mocha(RgbColor(249, 226, 175)))
    .context(mocha(RgbColor(166, 173, 200)))
    .context_value(mocha(RgbColor(180, 190, 254)));

#[derive(Debug, Parser)]
#[command(
    name = "rustmux",
    version,
    about = "A small terminal multiplexer written in Rust",
    args_conflicts_with_subcommands = true,
    styles = CATPPUCCIN_MOCHA_HELP
)]
pub(super) struct Cli {
    #[arg(long, hide = true, requires = "server")]
    pub(super) startup_layout: Option<std::path::PathBuf>,
    /// Create or attach to a session with this name
    #[arg(short = 's', long, value_name = "SESSION")]
    pub(super) session: Option<String>,

    #[arg(
        long,
        hide = true,
        num_args = 5,
        value_names = ["SOCKET", "COLUMNS", "ROWS", "PIXEL_WIDTH", "PIXEL_HEIGHT"]
    )]
    pub(super) server: Option<Vec<String>>,

    #[command(subcommand)]
    pub(super) command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub(super) enum Command {
    #[command(flatten)]
    Control(crate::control::ControlCommand),
    /// Create or attach to a session
    #[command(visible_alias = "new")]
    NewSession(NewSessionArgs),

    /// Attach to an existing session
    #[command(visible_aliases = ["a", "attach-session"])]
    Attach(AttachArgs),

    /// List available sessions
    #[command(visible_alias = "ls")]
    ListSessions,

    /// Stop a session and its processes
    #[command(visible_alias = "k")]
    KillSession(TargetSessionArgs),

    /// Stop all running sessions and their processes
    #[command(visible_alias = "ka")]
    KillAllSessions(KillAllSessionsArgs),

    /// Inspect or generate configuration
    Setup(SetupArgs),

    /// Print the built-in configuration
    #[command(visible_alias = "dump-config")]
    DefaultConfig,

    /// Validate the active configuration
    CheckConfig,
}

#[derive(Debug, Args)]
pub(super) struct NewSessionArgs {
    /// Create the session without attaching a terminal
    #[arg(short = 'd', long)]
    pub(super) detached: bool,
    /// Start a new session from a project layout file
    #[arg(long)]
    pub(super) layout: Option<std::path::PathBuf>,
    /// Session name
    #[arg(value_name = "SESSION", conflicts_with = "session")]
    name: Option<String>,

    /// Session name (compatible with the previous Rustmux CLI)
    #[arg(short = 's', long = "session", value_name = "SESSION")]
    session: Option<String>,
}

impl NewSessionArgs {
    pub(super) fn name(&self) -> &str {
        self.name
            .as_deref()
            .or(self.session.as_deref())
            .unwrap_or("default")
    }
}

#[derive(Debug, Args)]
pub(super) struct AttachArgs {
    /// Session name
    #[arg(value_name = "SESSION", conflicts_with = "target")]
    name: Option<String>,

    /// Create the session when it does not exist
    #[arg(short = 'c', long)]
    pub(super) create: bool,

    /// Session name (compatible with the previous Rustmux CLI)
    #[arg(short = 't', long = "target", value_name = "SESSION")]
    target: Option<String>,
}

impl AttachArgs {
    pub(super) fn name(&self) -> &str {
        self.name
            .as_deref()
            .or(self.target.as_deref())
            .unwrap_or("default")
    }
}

#[derive(Debug, Args)]
pub(super) struct TargetSessionArgs {
    /// Session name
    #[arg(value_name = "SESSION", conflicts_with = "target")]
    name: Option<String>,

    /// Session name (compatible with the previous Rustmux CLI)
    #[arg(short = 't', long = "target", value_name = "SESSION")]
    target: Option<String>,
}

impl TargetSessionArgs {
    pub(super) fn name(&self) -> &str {
        self.name
            .as_deref()
            .or(self.target.as_deref())
            .unwrap_or("default")
    }
}

#[derive(Debug, Args)]
pub(super) struct KillAllSessionsArgs {
    /// Skip the confirmation prompt
    #[arg(short = 'y', long)]
    pub(super) yes: bool,
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("operation")
        .required(true)
        .multiple(false)
        .args(["dump_config", "check"])
))]
pub(super) struct SetupArgs {
    /// Print the built-in configuration
    #[arg(long)]
    pub(super) dump_config: bool,

    /// Validate the active configuration
    #[arg(long)]
    pub(super) check: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{ColorChoice, CommandFactory};

    fn parse(arguments: &[&str]) -> Cli {
        Cli::try_parse_from(arguments).unwrap()
    }

    #[test]
    fn parses_zellij_style_session_option() {
        let cli = parse(&["rustmux", "--session", "work"]);
        assert_eq!(cli.session.as_deref(), Some("work"));
    }

    #[test]
    fn help_uses_catppuccin_mocha_truecolor_styles() {
        let help = Cli::command()
            .color(ColorChoice::Always)
            .render_help()
            .ansi()
            .to_string();

        assert!(help.contains("\x1b[38;2;166;227;161m"));
        assert!(help.contains("\x1b[38;2;203;166;247m"));
        assert!(help.contains("\x1b[38;2;137;180;250m"));
        assert!(help.contains("\x1b[38;2;250;179;135m"));
    }

    #[test]
    fn parses_attach_alias_with_positional_name_and_create() {
        let cli = parse(&["rustmux", "a", "work", "--create"]);
        let Some(Command::Attach(arguments)) = cli.command else {
            panic!("expected attach command");
        };
        assert_eq!(arguments.name(), "work");
        assert!(arguments.create);
    }

    #[test]
    fn parses_session_management_aliases() {
        assert!(matches!(
            parse(&["rustmux", "ls"]).command,
            Some(Command::ListSessions)
        ));
        let Some(Command::KillSession(arguments)) = parse(&["rustmux", "k", "work"]).command else {
            panic!("expected kill-session command");
        };
        assert_eq!(arguments.name(), "work");

        let Some(Command::KillAllSessions(arguments)) = parse(&["rustmux", "ka", "--yes"]).command
        else {
            panic!("expected kill-all-sessions command");
        };
        assert!(arguments.yes);
    }

    #[test]
    fn preserves_legacy_session_flags() {
        let Some(Command::NewSession(arguments)) =
            parse(&["rustmux", "new-session", "-s", "work"]).command
        else {
            panic!("expected new-session command");
        };
        assert_eq!(arguments.name(), "work");

        let Some(Command::Attach(arguments)) =
            parse(&["rustmux", "attach-session", "-t", "work"]).command
        else {
            panic!("expected attach command");
        };
        assert_eq!(arguments.name(), "work");
    }

    #[test]
    fn rejects_conflicting_session_names() {
        assert!(Cli::try_parse_from(["rustmux", "attach", "work", "--target", "other"]).is_err());
    }

    #[test]
    fn setup_requires_exactly_one_operation() {
        let cli = parse(&["rustmux", "setup", "--dump-config"]);
        let Some(Command::Setup(arguments)) = cli.command else {
            panic!("expected setup command");
        };
        assert!(arguments.dump_config);
        assert!(!arguments.check);

        assert!(Cli::try_parse_from(["rustmux", "setup"]).is_err());
        assert!(Cli::try_parse_from(["rustmux", "setup", "--dump-config", "--check"]).is_err());
    }

    #[test]
    fn parses_hidden_server_arguments() {
        let cli = parse(&[
            "rustmux",
            "--server",
            "/tmp/rustmux.sock",
            "120",
            "40",
            "960",
            "640",
        ]);
        assert_eq!(
            cli.server,
            Some(vec![
                "/tmp/rustmux.sock".to_owned(),
                "120".to_owned(),
                "40".to_owned(),
                "960".to_owned(),
                "640".to_owned(),
            ])
        );
    }
}
