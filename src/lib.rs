mod app;
mod cli;
mod config;
mod control;
mod input;
mod layout;
mod persistence;
mod project;
mod render;
mod scrollback;
mod session;
mod shell;
mod terminal;
mod theme;

#[cfg(feature = "benchmarks")]
#[doc(hidden)]
pub mod benchmarking;

use std::env;
use std::error::Error;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use cli::{Cli, Command};
use config::{Config, DEFAULT_CONFIG_TOML, config_path};
use crossterm::{
    cursor::Show,
    execute,
    terminal::{disable_raw_mode, enable_raw_mode},
};
use session::{attach_or_create, available_sessions, kill_session, list_sessions, run_server};

const PREFIX: u8 = 0x02;
const SCROLLBACK_LINES: usize = 5_000;
const ESCAPE_SEQUENCE_TIMEOUT: Duration = Duration::from_millis(20);
const FRAME_INTERVAL: Duration = Duration::from_millis(6);
const EARLY_DISCONNECT_RETRY: Duration = Duration::from_millis(100);
const MAX_PTY_READS_PER_TICK: usize = 32;
const MOUSE_SCROLL_LINES: usize = 3;
const ENCODED_PREFIXES: [&[u8]; 2] = [b"\x1b[98;5u", b"\x1b[27;5;98~"];
const CLIENT_INPUT: u8 = b'I';
const CLIENT_RESIZE: u8 = b'R';
const CLIENT_SHUTDOWN: u8 = b'Q';
const CLIENT_DELETE_SESSION: u8 = b'X';
const CLIENT_QUERY_STATUS: u8 = b'S';
const CLIENT_DISCONNECT: u8 = b'D';
const CLIENT_RENAME_SESSION: u8 = b'N';
const SERVER_SESSION_BUSY: &[u8] = b"\x1b]777;rustmux-session-busy\x07";
const SERVER_SWITCH_SESSION_PREFIX: &[u8] = b"\x1b]777;rustmux-switch-session=";
const MAX_CLIENT_MESSAGE_BYTES: usize = 1024 * 1024;
const CLIPBOARD_STATUS: &str = "copied to system clipboard";
const CLIPBOARD_STATUS_DURATION: Duration = Duration::from_secs(2);
const NOTIFICATION_DURATION: Duration = Duration::from_secs(3);
const RUSTMUX_ENV: &str = "RUSTMUX";
const NESTED_SESSION_WARNING: &str = "nested Rustmux sessions are not supported";
const TERMINAL_ENTER_SEQUENCE: &[u8] = b"\x1b[?1049h\x1b[>0u\x1b[?1004h\x1b[?1003h\x1b[?1006h";
const TERMINAL_EXIT_SEQUENCE: &[u8] = b"\x1b[?2026l\x1b[0 q\x1b[0m\x1b]112\x1b\\\x1b]22;\x1b\\\x1b[?1l\x1b[?2004l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1004l\x1b[?1006l\x1b[?5522l\x1b[<u\x1b>\x1b[?1049l";

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        let guard = Self;
        let mut stdout = io::stdout().lock();
        stdout.write_all(TERMINAL_ENTER_SEQUENCE)?;
        stdout.flush()?;
        Ok(guard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = io::stdout().write_all(TERMINAL_EXIT_SEQUENCE);
        let _ = execute!(io::stdout(), Show);
        let _ = disable_raw_mode();
    }
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    if let Some(values) = cli.server {
        let (socket, values) = values.split_first().ok_or("missing server socket")?;
        return run_server(PathBuf::from(socket), values, cli.startup_layout.as_deref());
    }
    if starts_session(&cli) && env::var_os(RUSTMUX_ENV).is_some() {
        eprintln!("warning: {NESTED_SESSION_WARNING}");
        return Ok(());
    }
    if let Some(session) = cli.session {
        return attach_or_create(&session, true);
    }

    match cli.command {
        None => attach_or_create("default", true),
        Some(Command::NewSession(arguments)) => session::new_session(
            arguments.name(),
            arguments.detached,
            arguments.layout.as_deref(),
        ),
        Some(Command::Attach(arguments)) => attach_or_create(arguments.name(), arguments.create),
        Some(Command::ListSessions) => list_sessions(),
        Some(Command::Control(command)) => control::run(command),
        Some(Command::KillSession(arguments)) => kill_session(arguments.name()),
        Some(Command::KillAllSessions(arguments)) => kill_all_sessions(arguments.yes),
        Some(Command::DefaultConfig) => {
            print!("{DEFAULT_CONFIG_TOML}");
            Ok(())
        }
        Some(Command::CheckConfig) => check_config(),
        Some(Command::Setup(arguments)) if arguments.dump_config => {
            print!("{DEFAULT_CONFIG_TOML}");
            Ok(())
        }
        Some(Command::Setup(arguments)) if arguments.check => check_config(),
        Some(Command::Setup(_)) => unreachable!("clap requires one setup operation"),
    }
}

fn starts_session(cli: &Cli) -> bool {
    cli.session.is_some()
        || match &cli.command {
            None | Some(Command::Attach(_)) => true,
            Some(Command::NewSession(arguments)) => !arguments.detached,
            _ => false,
        }
}

fn kill_all_sessions(skip_confirmation: bool) -> Result<()> {
    let sessions = available_sessions()?;
    if sessions.is_empty() {
        println!("no sessions");
        return Ok(());
    }

    if !skip_confirmation {
        eprint!("Kill all {} running session(s)? [y/N] ", sessions.len());
        io::stderr().flush()?;
        let mut response = String::new();
        io::stdin().read_line(&mut response)?;
        if !matches!(response.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("aborted");
            return Ok(());
        }
    }

    let mut count = 0;
    let mut errors = Vec::new();
    for session in sessions {
        match kill_session(&session) {
            Ok(()) => count += 1,
            Err(error) => errors.push(format!("{session}: {error}")),
        }
    }
    println!("killed {count} session(s)");
    if !errors.is_empty() {
        return Err(format!("failed to stop session(s):\n{}", errors.join("\n")).into());
    }
    Ok(())
}

fn check_config() -> Result<()> {
    Config::load().map_err(|error| format!("configuration error: {error}"))?;
    let path = config_path();
    if path.exists() {
        println!("{}: ok", path.display());
    } else {
        println!("{}: not found; built-in defaults are valid", path.display());
    }
    Ok(())
}

#[cfg(feature = "fuzzing")]
pub mod fuzzing;

#[cfg(test)]
mod tests;
