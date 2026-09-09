mod app;
mod config;
mod input;
mod layout;
mod render;
mod session;
mod terminal;

use std::env;
use std::error::Error;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;

use config::{Config, DEFAULT_CONFIG_TOML, config_path};
use crossterm::{
    cursor::Show,
    execute,
    terminal::{disable_raw_mode, enable_raw_mode},
};
use session::{attach_or_create, kill_session, list_sessions, run_server};

const PREFIX: u8 = 0x02;
const SCROLLBACK_LINES: usize = 1_000;
const ESCAPE_SEQUENCE_TIMEOUT: Duration = Duration::from_millis(50);
const FRAME_INTERVAL: Duration = Duration::from_millis(8);
const EARLY_DISCONNECT_RETRY: Duration = Duration::from_millis(100);
const MAX_PTY_READS_PER_TICK: usize = 32;
const MOUSE_SCROLL_LINES: usize = 3;
const ENCODED_PREFIXES: [&[u8]; 2] = [b"\x1b[98;5u", b"\x1b[27;5;98~"];
const CLIENT_INPUT: u8 = b'I';
const CLIENT_RESIZE: u8 = b'R';
const CLIENT_SHUTDOWN: u8 = b'Q';
const CLIENT_QUERY_STATUS: u8 = b'S';
const CLIENT_DISCONNECT: u8 = b'D';
const CLIENT_RENAME_SESSION: u8 = b'N';
const SERVER_SWITCH_SESSION_PREFIX: &[u8] = b"\x1b]777;rustmux-switch-session=";
const MAX_CLIENT_MESSAGE_BYTES: usize = 1024 * 1024;
const CLIPBOARD_STATUS: &str = "copied to system clipboard";
const CLIPBOARD_STATUS_DURATION: Duration = Duration::from_secs(2);
const NOTIFICATION_DURATION: Duration = Duration::from_secs(3);
const TERMINAL_ENTER_SEQUENCE: &[u8] = b"\x1b[?1049h\x1b[?1002h\x1b[?1006h";
const TERMINAL_EXIT_SEQUENCE: &[u8] = b"\x1b[?2026l\x1b[0 q\x1b[0m\x1b[?1l\x1b[?2004l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b>\x1b[?1049l";

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

fn print_help() {
    println!(
        "rustmux {}\n\nA minimal terminal multiplexer.\n\nUSAGE:\n    rustmux [new-session [-s NAME]]\n    rustmux attach-session [-t NAME]\n    rustmux list-sessions\n    rustmux kill-session [-t NAME]\n    rustmux check-config\n    rustmux default-config\n\nConfig: {}\n",
        env!("CARGO_PKG_VERSION"),
        config_path().display(),
    );
}

pub fn run() -> Result<()> {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    match arguments.as_slice() {
        [] => attach_or_create("default", true),
        [flag] if flag == "-h" || flag == "--help" => {
            print_help();
            Ok(())
        }
        [flag] if flag == "-V" || flag == "--version" => {
            println!("rustmux {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        [command] if command == "default-config" => {
            print!("{DEFAULT_CONFIG_TOML}");
            Ok(())
        }
        [command] if command == "check-config" => {
            Config::load().map_err(|error| format!("configuration error: {error}"))?;
            let path = config_path();
            if path.exists() {
                println!("{}: ok", path.display());
            } else {
                println!("{}: not found; built-in defaults are valid", path.display());
            }
            Ok(())
        }
        [command] if command == "new" || command == "new-session" => {
            attach_or_create("default", true)
        }
        [command, flag, name] if (command == "new" || command == "new-session") && flag == "-s" => {
            attach_or_create(name, true)
        }
        [command] if command == "attach" || command == "attach-session" => {
            attach_or_create("default", false)
        }
        [command, flag, name]
            if (command == "attach" || command == "attach-session") && flag == "-t" =>
        {
            attach_or_create(name, false)
        }
        [command] if command == "ls" || command == "list-sessions" => list_sessions(),
        [command] if command == "kill-session" => kill_session("default"),
        [command, flag, name] if command == "kill-session" && flag == "-t" => kill_session(name),
        [command, socket, values @ ..] if command == "--server" => {
            run_server(PathBuf::from(socket), values)
        }
        _ => Err("invalid arguments (try --help)".into()),
    }
}

#[cfg(feature = "fuzzing")]
pub mod fuzzing;

#[cfg(test)]
mod tests;
