mod canvas;
mod clipboard;
mod events;
mod graphics;
mod input;
mod mouse;
mod output;
mod prefs;
mod renderer;
mod theme;
mod view;

use crate::{
    core::{Camera, Edge, Graph, History, VertexStyle},
    export,
};
use anyhow::{Context, Result};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
        KeyboardEnhancementFlags, MouseButton, MouseEvent, MouseEventKind,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
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
    time::{Duration, Instant},
};

/// What a press that may become a move started on.
#[derive(Clone, Copy)]
enum Grab {
    Vertex,
    /// A selected edge: a click selects just it.
    Edge(Edge),
    /// Empty canvas inside a multi-selection's box: a click deselects.
    Empty,
}

enum Drag {
    /// Dragging `anchor` moves every vertex in `ids` (the selection) together.
    Vertices {
        anchor: u64,
        ids: Vec<u64>,
        original: Graph,
        offset: (f64, f64),
        moved: bool,
        /// A plain click on a member of a multi-selection narrows it to that vertex.
        narrow: bool,
        /// What was pressed, for a click that never moves.
        grab: Grab,
    },
    /// Rubber-band selection in screen coordinates.
    Marquee {
        start: (f64, f64),
        end: (f64, f64),
        additive: bool,
    },
    Pan(f64, f64),
}

/// Selected vertices and edges. Bulk actions (style, delete, connect, move)
/// apply to all of them; a single selected vertex is also the label target.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Selection {
    pub vertices: Vec<u64>,
    pub edges: Vec<Edge>,
}

fn same_edge(a: &Edge, b: &Edge) -> bool {
    (a.a == b.a && a.b == b.b) || (a.a == b.b && a.b == b.a)
}

impl Selection {
    pub fn vertex(id: u64) -> Self {
        Self {
            vertices: vec![id],
            edges: Vec::new(),
        }
    }
    pub fn edge(edge: Edge) -> Self {
        Self {
            vertices: Vec::new(),
            edges: vec![edge],
        }
    }
    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty() && self.edges.is_empty()
    }
    pub fn len(&self) -> usize {
        self.vertices.len() + self.edges.len()
    }
    pub fn has_vertex(&self, id: u64) -> bool {
        self.vertices.contains(&id)
    }
    pub fn has_edge(&self, edge: &Edge) -> bool {
        self.edges.iter().any(|e| same_edge(e, edge))
    }
    pub fn toggle_vertex(&mut self, id: u64) {
        if self.has_vertex(id) {
            self.vertices.retain(|&v| v != id);
        } else {
            self.vertices.push(id);
        }
    }
    pub fn toggle_edge(&mut self, edge: Edge) {
        if self.has_edge(&edge) {
            self.edges.retain(|e| !same_edge(e, &edge));
        } else {
            self.edges.push(edge);
        }
    }
    pub fn extend(&mut self, other: Selection) {
        for id in other.vertices {
            if !self.has_vertex(id) {
                self.vertices.push(id);
            }
        }
        for edge in other.edges {
            if !self.has_edge(&edge) {
                self.edges.push(edge);
            }
        }
    }
    /// Drop anything that no longer exists (after undo, delete, ...).
    pub fn retain_existing(&mut self, graph: &Graph) {
        self.vertices.retain(|&id| graph.vertex(id).is_some());
        self.edges
            .retain(|edge| graph.edges.iter().any(|e| same_edge(e, edge)));
    }
    pub fn describe(&self) -> String {
        let count =
            |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let vertices = count(self.vertices.len(), "vertex", "vertices");
        let edges = count(self.edges.len(), "edge", "edges");
        match (self.vertices.len(), self.edges.len()) {
            (_, 0) => vertices,
            (0, _) => edges,
            _ => format!("{vertices}, {edges}"),
        }
    }
}

// Scene snapshots cross the worker boundary. Hover is a separate Kitty layer.
#[derive(Clone, PartialEq)]
struct CanvasState {
    graph: Graph,
    camera: Camera,
    selection: Selection,
    edge_start: Option<u64>,
    hover: Option<(f64, f64)>,
    /// Rubber-band rectangle (two screen-space corners) while dragging one.
    marquee: Option<((f64, f64), (f64, f64))>,
    /// The edge under the pointer, highlighted faintly.
    hover_edge: Option<Edge>,
    size: (u32, u32),
    layer: Layer,
    /// Turn of the pen's dashed ring ([`canvas::SPIN_STEPS`] per dash), and the edge it is unrolling into: from, to and progress in
    /// thousandths. Only the Kitty pen layer animates.
    spin: u16,
    unroll: Option<(u64, u64, u16)>,
    /// An edge a frame leaves out while it is animated in; the pen layer
    /// draws it until a frame that has it arrives.
    hide_edge: Option<Edge>,
}

/// Which parts of the scene a frame draws. While vertices are dragged or
/// relabeled on Kitty, one frame holds everything that stays put and the
/// changing part is redrawn as small tiles, so each pointer move or keystroke
/// sends kilobytes instead of a whole canvas.
#[derive(Clone, Debug, Default, PartialEq)]
enum Layer {
    #[default]
    All,
    /// Everything except these vertices and their edges, and except these
    /// other edges, which cross a moving circle or plain label (an overlay
    /// cannot erase the frame's pixels under it).
    Still(Vec<u64>, Vec<Edge>),
    /// Only these vertices, their labels and their edges, and these other
    /// edges.
    Moving(Vec<u64>, Vec<Edge>),
    /// Only the marks drawn over a Kitty frame: selection highlights, the
    /// edge-start ring and the hovered edge. Clicks that only change these
    /// redraw a few tiles instead of the canvas.
    Decor,
    /// Only the pen: its rotating ring and an edge it unrolls into, drawn
    /// behind the frame so labels and marks stay on top.
    Pen,
}

impl Layer {
    fn shows_vertex(&self, id: u64) -> bool {
        match self {
            Layer::All => true,
            Layer::Still(ids, _) => !ids.contains(&id),
            Layer::Moving(ids, _) => ids.contains(&id),
            Layer::Decor | Layer::Pen => false,
        }
    }
    fn cut(cuts: &[Edge], a: u64, b: u64) -> bool {
        cuts.iter()
            .any(|e| (e.a, e.b) == (a, b) || (e.a, e.b) == (b, a))
    }
    fn shows_edge(&self, a: u64, b: u64) -> bool {
        match self {
            Layer::All => true,
            Layer::Still(ids, cuts) => {
                !ids.contains(&a) && !ids.contains(&b) && !Self::cut(cuts, a, b)
            }
            Layer::Moving(ids, cuts) => {
                ids.contains(&a) || ids.contains(&b) || Self::cut(cuts, a, b)
            }
            Layer::Decor | Layer::Pen => false,
        }
    }
}

struct App {
    graph: Graph,
    camera: Camera,
    history: History,
    /// Style given to newly placed vertices; follows the last style change.
    new_style: VertexStyle,
    /// New vertices get no label; set by clearing a label.
    new_unlabeled: bool,
    delivered: Option<Graph>,
    selection: Selection,
    /// The pen: the vertex the next click draws an edge from.
    edge_start: Option<u64>,
    hover: Option<(f64, f64)>,
    hover_edge: Option<Edge>,
    drag: Option<Drag>,
    /// The last plain vertex click, for double-clicks, and the history
    /// generation of the edge it drew, if any (so a double-click can take
    /// that edge back).
    last_click: Option<(u64, Instant, Option<u64>)>,
    /// The camera a left-button gesture hit-tests against, fixed at its press.
    gesture_camera: Option<Camera>,
    /// The pen's ring unrolling into the edge it just drew: from, to, start.
    unroll: Option<(u64, u64, Instant)>,
    /// The pen ring's turn in dashes (0..1), how far it has spun up (0..1),
    /// and when that was last advanced.
    spin_pos: f64,
    spin_ramp: f64,
    spin_at: Instant,
    last_input: Instant,
    editing: Option<String>,
    /// Labeling several vertices starts with an empty field that stands for
    /// "keep their labels" until the user types or presses Enter.
    editing_placeholder: bool,
    status: String,
    /// When `status` was set; results clear themselves after [`STATUS_TTL`].
    status_at: Option<Instant>,
    /// Toolbar column under the pointer, for button highlighting.
    toolbar_hover: Option<u16>,
    /// The toolbar button pressed; it acts on release over the same button.
    toolbar_press: Option<view::ToolbarAction>,
    area: Rect,
    cell: (f64, f64),
    picker: Picker,
    kitty: Option<graphics::KittyCanvas>,
    renderer: renderer::Renderer,
    dirty: bool,
    scene_dirty: bool,
    quit_pending: bool,
    canvas_state: Option<CanvasState>,
    /// The dragged layer the shown tiles were drawn from.
    tiles_state: Option<CanvasState>,
    /// The decorations the shown decor tiles were drawn from.
    decor_state: Option<CanvasState>,
    /// The pen layer the shown pen tiles were drawn from.
    pen_state: Option<CanvasState>,
    protocol: Option<StatefulProtocol>,
    fit_on_open: bool,
    /// The terminal reports mouse positions in pixels (SGR-Pixels, mode 1016)
    /// rather than cells, so pans and drags move smoothly.
    pixel_mouse: bool,
    /// How Kitty frames reach the terminal; shared memory when it says so.
    transport: graphics::Transport,
}

/// How long an operation result stays in the status row.
const STATUS_TTL: Duration = Duration::from_secs(3);

struct TerminalGuard {
    keyboard_enhanced: bool,
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        if self.keyboard_enhanced {
            let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
        }
        let _ = execute!(
            stdout(),
            crossterm::terminal::EndSynchronizedUpdate,
            crossterm::style::Print(graphics::cleanup()),
            crossterm::style::Print("\x1b[?1016l"),
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        graphics::shm::unlink_all();
    }
}

pub fn run() -> Result<()> {
    canvas::preload_fonts();
    // Query in the alternate screen, before crossterm owns input.
    let mut guard = TerminalGuard {
        keyboard_enhanced: false,
    };
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
    let capabilities = mouse::probe(picker.protocol_type() == ProtocolType::Kitty);
    let pixel_mouse = capabilities.pixel_mouse;
    execute!(
        stdout(),
        EnableMouseCapture,
        crossterm::style::Print(if pixel_mouse { "\x1b[?1016h" } else { "" }),
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
        )
    )?;
    guard.keyboard_enhanced = true;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    let mut app = App::new(Graph::default(), picker);
    app.pixel_mouse = pixel_mouse;
    if capabilities.shared_memory {
        app.transport = graphics::Transport::SharedMemory;
    }
    let preferences = prefs::path();
    if let Some(path) = &preferences {
        prefs::load(&mut app, path);
    }
    let result = app.event_loop(&mut terminal);
    if let Some(path) = &preferences {
        prefs::save(&app, path);
    }
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
            new_style: graph
                .vertices
                .last()
                .map_or(VertexStyle::default(), |v| v.style),
            new_unlabeled: false,
            graph,
            camera: Camera::default(),
            history: History::default(),
            delivered: None,
            selection: Selection::default(),
            edge_start: None,
            hover: None,
            hover_edge: None,
            drag: None,
            last_click: None,
            gesture_camera: None,
            unroll: None,
            spin_pos: 0.,
            spin_ramp: 0.,
            spin_at: Instant::now(),
            last_input: Instant::now(),
            editing: None,
            editing_placeholder: false,
            status: String::new(),
            status_at: None,
            toolbar_hover: None,
            toolbar_press: None,
            area: Rect::default(),
            cell,
            picker,
            kitty: (picker.protocol_type() == ProtocolType::Kitty).then(graphics::KittyCanvas::new),
            renderer: renderer::Renderer::new(),
            dirty: true,
            scene_dirty: true,
            canvas_state: None,
            tiles_state: None,
            decor_state: None,
            pen_state: None,
            protocol: None,
            fit_on_open: true,
            pixel_mouse: false,
            transport: graphics::Transport::Inline,
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
