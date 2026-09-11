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
