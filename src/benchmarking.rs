//! Stable entry points for the out-of-tree benchmark harness.

use std::fs::File;
use std::os::fd::OwnedFd;

use nix::unistd::Pid;

use crate::app::Window;
use crate::layout::{PaneRect, content_size_for};
use crate::render::Renderer;
use crate::terminal::{
    CursorStyleTracker, HyperlinkTracker, InputModeTracker, KittyDndEvent, KittyDndParser,
    KittyGraphicsParser, KittyIpcParser, SemanticOutputCapture, TerminalMetadata,
    TerminalOscTracker, base64_encode, terminal_parser_size,
};

const OUTER_SIZE: (u16, u16) = (82, 28);
const IMAGE_COLUMNS: usize = 80;
const IMAGE_ROWS: usize = 24;
const KITTY_CHUNK_BYTES: usize = 4096;

/// Build the bytes that a preview application sends after converting a source
/// file to a Kitty-compatible raster image. The source bytes seed a stable
/// payload so every file type exercises the same amount of Rustmux work.
pub fn image_preview_stream(source: &[u8], payload_bytes: usize) -> Vec<u8> {
    let source = if source.is_empty() { &[0][..] } else { source };
    let raster = source
        .iter()
        .copied()
        .cycle()
        .take(payload_bytes)
        .collect::<Vec<_>>();
    let encoded = base64_encode(&raster);
    let chunks = encoded.as_bytes().chunks(KITTY_CHUNK_BYTES);
    let chunk_count = chunks.len();
    let mut stream = Vec::with_capacity(encoded.len() + chunk_count * 64 + 16 * 1024);
    for (index, chunk) in chunks.enumerate() {
        if index == 0 {
            stream.extend_from_slice(
                format!(
                    "\x1b_Ga=T,f=100,q=2,i=42,s={},v={},m={};",
                    IMAGE_COLUMNS,
                    IMAGE_ROWS,
                    u8::from(index + 1 < chunk_count)
                )
                .as_bytes(),
            );
        } else {
            stream.extend_from_slice(
                format!("\x1b_Gm={};", u8::from(index + 1 < chunk_count)).as_bytes(),
            );
        }
        stream.extend_from_slice(chunk);
        stream.extend_from_slice(b"\x1b\\");
    }

    stream.extend_from_slice(b"\x1b[H\x1b[38;2;0;0;42m");
    let placeholder = "\u{10eeee}\u{0305}\u{0305}";
    for row in 0..IMAGE_ROWS {
        stream.extend_from_slice(placeholder.repeat(IMAGE_COLUMNS).as_bytes());
        if row + 1 < IMAGE_ROWS {
            stream.extend_from_slice(b"\r\n");
        }
    }
    stream
}

pub struct ImagePreviewRunner {
    window: Window,
}

impl ImagePreviewRunner {
    pub fn new() -> Self {
        let (columns, rows) = content_size_for(OUTER_SIZE, false);
        let (columns, rows) = terminal_parser_size(columns, rows);
        let master: OwnedFd = File::open("/dev/null")
            .expect("/dev/null is available")
            .into();
        Self {
            window: Window {
                id: 1,
                tab_id: 1,
                name: "preview".to_owned(),
                floating: false,
                pane_rect: PaneRect {
                    column: 0,
                    row: 0,
                    width: columns,
                    height: rows,
                },
                pane_framed: false,
                zoomed: false,
                master,
                child: Pid::from_raw(1),
                spawn_directory: None,
                startup_command: None,
                terminal: vt100::Parser::new_with_callbacks(
                    rows,
                    columns,
                    1_000,
                    TerminalMetadata::default(),
                ),
                cursor_style: CursorStyleTracker::default(),
                input_modes: InputModeTracker::default(),
                terminal_osc: TerminalOscTracker::default(),
                kitty_graphics: KittyGraphicsParser::default(),
                kitty_dnd: KittyDndParser::default(),
                kitty_ipc: KittyIpcParser::default(),
                hyperlinks: HyperlinkTracker::default(),
                dnd_drag_registration: None,
                dnd_drop_registration: None,
                pending_graphics: Vec::new(),
                history_mode: false,
                bell_pending: false,
                command_output: SemanticOutputCapture::default(),
                notification_applications: Vec::new(),
                temporary_file: None,
                return_to_window: None,
            },
        }
    }

    /// Parse one complete preview and render the frame that would be written to
    /// Kitty. The returned size is consumed by `black_box` in the harness.
    pub fn run(&mut self, stream: &[u8]) -> usize {
        let (columns, rows) = content_size_for(OUTER_SIZE, false);
        let (columns, rows) = terminal_parser_size(columns, rows);
        self.window.terminal =
            vt100::Parser::new_with_callbacks(rows, columns, 1_000, TerminalMetadata::default());
        self.window.cursor_style = CursorStyleTracker::default();
        self.window.input_modes = InputModeTracker::default();
        self.window.terminal_osc = TerminalOscTracker::default();
        self.window.kitty_dnd = KittyDndParser::default();
        self.window.kitty_ipc = KittyIpcParser::default();
        self.window.hyperlinks = HyperlinkTracker::default();
        self.window.command_output = SemanticOutputCapture::default();
        let mut graphics_parser = KittyGraphicsParser::default();
        let mut graphics = Vec::new();
        for chunk in stream.chunks(64 * 1024) {
            let parsed = graphics_parser.process(chunk);
            graphics.extend(parsed.commands);
            let dnd = self.window.kitty_dnd.process(&parsed.terminal);
            let terminal = dnd
                .events
                .into_iter()
                .filter_map(|event| match event {
                    KittyDndEvent::Terminal(bytes) => Some(bytes),
                    KittyDndEvent::Command(_) => None,
                })
                .flatten()
                .collect::<Vec<_>>();
            let ipc = self.window.kitty_ipc.process(&terminal);
            self.window.input_modes.process(&ipc.terminal);
            self.window
                .terminal_osc
                .set_alternate_screen(self.window.input_modes.alternate_screen());
            self.window.terminal_osc.process(&ipc.terminal);
            self.window.cursor_style.process(&ipc.terminal);
            self.window.command_output.process(&ipc.terminal);
            self.window
                .hyperlinks
                .process(&ipc.terminal, &mut self.window.terminal);
        }

        let mut renderer = Renderer::default();
        renderer.set_session_name("benchmark");
        renderer
            .render(
                std::slice::from_ref(&self.window),
                0,
                OUTER_SIZE,
                "locked",
                None,
                &graphics,
            )
            .len()
    }
}

impl Default for ImagePreviewRunner {
    fn default() -> Self {
        Self::new()
    }
}

/// Measurements of the production history capture and background save paths.
pub struct SessionSaveSample {
    pub capture_ms: f64,
    pub unchanged_ms: f64,
    pub encode_ms: f64,
    pub background_ms: f64,
    pub bytes: usize,
}

pub struct SessionSaveRunner {
    terminals: Vec<vt100::Parser<TerminalMetadata>>,
    lines: usize,
    save_history: bool,
    colored: bool,
    sequence: usize,
}

impl SessionSaveRunner {
    /// `density`: 0 = plain, 1 = 3 style runs/row, 2 = one style/cell.
    pub fn new(panes: usize, lines: usize, density: u8, save_history: bool, colored: bool) -> Self {
        let mut terminals = Vec::new();
        for _ in 0..panes {
            let mut terminal =
                vt100::Parser::new_with_callbacks(24, 120, lines, TerminalMetadata::default());
            for row in 0..lines + 24 {
                let mut output = String::new();
                for column in 0..120 {
                    if density == 2 || (density == 1 && column % 40 == 0) {
                        use std::fmt::Write;
                        write!(&mut output, "\x1b[38;5;{}m", (row + column) % 216 + 16).unwrap();
                    }
                    output.push(char::from(b'a' + ((row + column) % 26) as u8));
                }
                output.push_str("\x1b[0m\r\n");
                terminal.process(output.as_bytes());
            }
            terminals.push(terminal);
        }
        Self {
            terminals,
            lines,
            save_history,
            colored,
            sequence: 0,
        }
    }

    fn snapshot(&self) -> crate::session::SessionSnapshot {
        use crate::session::{ScrollbackFormat, SessionSnapshot, SnapshotPane, SnapshotTab};
        let tabs = self
            .terminals
            .iter()
            .enumerate()
            .map(|(index, terminal)| {
                let id = index + 1;
                SnapshotTab {
                    id,
                    name: format!("pane-{id}"),
                    root: crate::layout::PaneNode::Leaf(id),
                    panes: vec![SnapshotPane {
                        id,
                        cwd: Some(std::path::PathBuf::from("/tmp")),
                        command: None,
                        scrollback_format: if self.colored && self.save_history {
                            ScrollbackFormat::Ansi
                        } else {
                            ScrollbackFormat::Plain
                        },
                        scrollback: self.save_history.then(|| {
                            crate::app::snapshot_scrollback(
                                terminal.screen(),
                                self.lines,
                                self.colored,
                            )
                        }),
                    }],
                }
            })
            .collect();
        SessionSnapshot::new(tabs, None, 1)
    }

    pub fn measure(&mut self) -> SessionSaveSample {
        use std::hint::black_box;
        use std::sync::Arc;
        use std::time::Instant;
        self.sequence += 1;
        for terminal in &mut self.terminals {
            terminal.process(format!("update {}\r\n", self.sequence).as_bytes());
        }
        let started = Instant::now();
        let snapshot = Arc::new(self.snapshot());
        let capture_ms = started.elapsed().as_secs_f64() * 1000.0;
        let started = Instant::now();
        let candidate = self.snapshot();
        assert!(black_box(&candidate) == snapshot.as_ref());
        drop(candidate);
        let unchanged_ms = started.elapsed().as_secs_f64() * 1000.0;
        let started = Instant::now();
        let encoded = toml::to_string_pretty(snapshot.as_ref()).unwrap();
        let encode_ms = started.elapsed().as_secs_f64() * 1000.0;
        let bytes = encoded.len();
        drop(encoded);
        let mut writer = crate::persistence::SnapshotWriter::default();
        let started = Instant::now();
        writer
            .submit(crate::persistence::SaveRequest {
                name: "save-benchmark".to_owned(),
                snapshot,
                notify: false,
                replies: Vec::new(),
            })
            .unwrap();
        for completion in writer.flush() {
            completion.result.unwrap();
        }
        let background_ms = started.elapsed().as_secs_f64() * 1000.0;
        SessionSaveSample {
            capture_ms,
            unchanged_ms,
            encode_ms,
            background_ms,
            bytes,
        }
    }
}
