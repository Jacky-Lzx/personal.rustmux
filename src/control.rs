use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};

use crate::Result;

pub(super) const REQUEST: u8 = b'C';
const MAX_FRAME: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Args, Serialize, Deserialize)]
pub(super) struct Target {
    /// Target session
    #[arg(short = 's', long, default_value = "default")]
    pub session: String,
}

#[derive(Clone, Debug, Args, Serialize, Deserialize)]
pub(super) struct PaneTarget {
    #[command(flatten)]
    #[serde(flatten)]
    pub target: Target,
    /// Stable pane ID from list-panes (defaults to the active pane)
    #[arg(short = 'p', long)]
    pub pane: Option<usize>,
}

#[derive(Clone, Debug, Subcommand, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub(super) enum ControlCommand {
    /// List pane IDs, window numbers, titles, and directories
    ListPanes {
        #[command(flatten)]
        #[serde(flatten)]
        target: Target,
        /// Print machine-readable TOML
        #[arg(long)]
        toml: bool,
    },
    /// Create a window and print its pane ID
    NewWindow {
        #[command(flatten)]
        #[serde(flatten)]
        target: Target,
        #[arg(short = 'n', long)]
        name: Option<String>,
    },
    /// Split a pane to the right (or below with --down), and print the new ID
    SplitPane {
        #[command(flatten)]
        #[serde(flatten)]
        target: PaneTarget,
        #[arg(long)]
        down: bool,
    },
    /// Send named keys or literal text to a pane
    SendKeys {
        #[command(flatten)]
        #[serde(flatten)]
        target: PaneTarget,
        /// Treat the arguments as text, joined by spaces
        #[arg(short = 'l', long)]
        literal: bool,
        /// Append Enter after the supplied input
        #[arg(long)]
        enter: bool,
        #[arg(required = true, num_args = 1..)]
        keys: Vec<String>,
    },
    /// Print visible pane contents, or all scrollback with --history
    CapturePane {
        #[command(flatten)]
        #[serde(flatten)]
        target: PaneTarget,
        #[arg(long)]
        history: bool,
    },
    /// Move a pane beside another pane, preserving its process and ID
    JoinPane {
        #[command(flatten)]
        #[serde(flatten)]
        target: PaneTarget,
        #[arg(long)]
        to_pane: usize,
        #[arg(long)]
        down: bool,
    },
    /// Move a pane into its own window, preserving its process and ID
    BreakPane {
        #[command(flatten)]
        #[serde(flatten)]
        target: PaneTarget,
        #[arg(short = 'n', long)]
        name: Option<String>,
    },
    /// Save the current layout immediately
    SaveSession {
        #[command(flatten)]
        #[serde(flatten)]
        target: Target,
    },
}

impl ControlCommand {
    fn session(&self) -> &str {
        match self {
            Self::ListPanes { target, .. }
            | Self::NewWindow { target, .. }
            | Self::SaveSession { target } => &target.session,
            Self::SplitPane { target, .. }
            | Self::SendKeys { target, .. }
            | Self::CapturePane { target, .. }
            | Self::JoinPane { target, .. }
            | Self::BreakPane { target, .. } => &target.target.session,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub(super) struct Response {
    pub ok: bool,
    pub output: String,
}

pub(super) fn read_frame(stream: &mut UnixStream) -> Result<String> {
    let mut length = [0; 4];
    stream.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_FRAME {
        return Err("control message exceeds 16 MiB limit".into());
    }
    let mut contents = vec![0; length];
    stream.read_exact(&mut contents)?;
    Ok(String::from_utf8(contents)?)
}

pub(super) fn write_frame(stream: &mut UnixStream, contents: &str) -> Result<()> {
    if contents.len() > MAX_FRAME {
        return Err("control message exceeds 16 MiB limit".into());
    }
    stream.write_all(&(contents.len() as u32).to_be_bytes())?;
    stream.write_all(contents.as_bytes())?;
    Ok(())
}

pub(super) fn run(command: ControlCommand) -> Result<()> {
    let socket = crate::session::session_socket(command.session())?;
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(&[REQUEST])?;
    write_frame(&mut stream, &toml::to_string(&command)?)?;
    let response: Response = toml::from_str(&read_frame(&mut stream)?)?;
    if !response.ok {
        return Err(response.output.into());
    }
    print!("{}", response.output);
    Ok(())
}
