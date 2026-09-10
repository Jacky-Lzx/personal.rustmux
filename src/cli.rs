use clap::{ArgGroup, Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "rustmux",
    version,
    about = "A small terminal multiplexer written in Rust",
    args_conflicts_with_subcommands = true
)]
pub(super) struct Cli {
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

    fn parse(arguments: &[&str]) -> Cli {
        Cli::try_parse_from(arguments).unwrap()
    }

    #[test]
    fn parses_zellij_style_session_option() {
        let cli = parse(&["rustmux", "--session", "work"]);
        assert_eq!(cli.session.as_deref(), Some("work"));
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
