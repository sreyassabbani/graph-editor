mod canvas;
mod clipboard;
mod events;
mod graphics;
mod input;
mod output;
mod renderer;
mod theme;
mod view;

use crate::{
    core::{Camera, Graph, History, VertexStyle},
    export,
};
use anyhow::{Context, Result};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
        MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Terminal,
};
use ratatui_image::{
    picker::{Picker, ProtocolType},
    protocol::StatefulProtocol,
    StatefulImage,
};
use resvg::tiny_skia::Pixmap;
use std::{
    io::{self, stdout},
    path::PathBuf,
    time::{Duration, Instant},
};

enum Drag {
    Vertex {
        id: u64,
        original: Graph,
        offset: (f64, f64),
        moved: bool,
    },
    Pan(f64, f64),
}

struct PathPrompt {
    input: String,
    overwrite: bool,
}

// Scene snapshots cross the worker boundary. Hover is a separate Kitty layer.
#[derive(Clone, PartialEq)]
struct CanvasState {
    graph: Graph,
    camera: Camera,
    selected: Option<u64>,
    edge_start: Option<u64>,
    hover: Option<(f64, f64)>,
    size: (u32, u32),
}

struct App {
    graph: Graph,
    camera: Camera,
    history: History,
    export_path: Option<PathBuf>,
    path_prompt: Option<PathPrompt>,
    delivered: Option<Graph>,
    selected: Option<u64>,
    edge_start: Option<u64>,
    hover: Option<(f64, f64)>,
    drag: Option<Drag>,
    last_click: Option<(u64, Instant)>,
    editing: Option<String>,
    status: String,
    area: Rect,
    cell: (f64, f64),
    picker: Picker,
    kitty: Option<graphics::KittyCanvas>,
    renderer: renderer::Renderer,
    dirty: bool,
    scene_dirty: bool,
    quit_pending: bool,
    canvas_state: Option<CanvasState>,
    protocol: Option<StatefulProtocol>,
    fit_on_open: bool,
}

struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            stdout(),
            crossterm::terminal::EndSynchronizedUpdate,
            crossterm::style::Print(graphics::cleanup()),
            DisableMouseCapture,
            LeaveAlternateScreen
        );
    }
}

pub fn run() -> Result<()> {
    // Query in the alternate screen, before crossterm owns input.
    let guard = TerminalGuard;
    execute!(stdout(), EnterAlternateScreen)?;
    let picker =
        Picker::from_query_stdio().context("could not query terminal graphics capabilities")?;
    if picker.protocol_type() == ProtocolType::Halfblocks {
        anyhow::bail!("This terminal did not report a supported graphics protocol. Use a Kitty, iTerm2, or Sixel-compatible terminal.");
    }
    let cell = picker.font_size();
    if cell.0 == 0 || cell.1 == 0 {
        anyhow::bail!("Terminal did not report a usable cell pixel size");
    }
    enable_raw_mode()?;
    execute!(stdout(), EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    let mut app = App::new(Graph::default(), picker);
    let result = app.event_loop(&mut terminal);
    let _ = terminal.show_cursor();
    drop(guard);
    result
}

impl App {
    fn new(graph: Graph, picker: Picker) -> Self {
        // Input and geometry use logical units; SVG rendering uses the terminal
        // pixel dimensions separately, so Retina detail is never discarded.
        let physical = picker.font_size();
        let scale = (8. / physical.0 as f64)
            .min(16. / physical.1 as f64)
            .min(1.);
        let cell = (physical.0 as f64 * scale, physical.1 as f64 * scale);
        Self {
            quit_pending: false,
            graph,
            camera: Camera::default(),
            history: History::default(),
            export_path: None,
            path_prompt: None,
            delivered: None,
            selected: None,
            edge_start: None,
            hover: None,
            drag: None,
            last_click: None,
            editing: None,
            status: String::new(),
            area: Rect::default(),
            cell,
            picker,
            kitty: (picker.protocol_type() == ProtocolType::Kitty).then(graphics::KittyCanvas::new),
            renderer: renderer::Renderer::new(),
            dirty: true,
            scene_dirty: true,
            canvas_state: None,
            protocol: None,
            fit_on_open: true,
        }
    }
}

#[cfg(test)]
mod test_support {
    use super::*;
    pub(super) fn app() -> App {
        let mut graph = Graph::default();
        graph.add_vertex(0., 0.);
        let mut app = App::new(graph, Picker::from_fontsize((8, 16)));
        app.area = Rect::new(0, 2, 80, 21);
        app
    }
}
