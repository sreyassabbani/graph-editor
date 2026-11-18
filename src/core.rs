use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VertexStyle {
    #[default]
    Dots,
    Circles,
    Plain,
}

impl VertexStyle {
    pub fn next(self) -> Self {
        match self {
            Self::Dots => Self::Circles,
            Self::Circles => Self::Plain,
            Self::Plain => Self::Dots,
        }
    }
    /// The `typst/graph.typ` style name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Dots => "dots",
            Self::Circles => "circles",
            Self::Plain => "plain",
        }
    }
    pub fn from_name(name: &str) -> Option<Self> {
        [Self::Dots, Self::Circles, Self::Plain]
            .into_iter()
            .find(|s| s.name() == name)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Vertex {
    pub id: u64,
    pub x: f64,
    pub y: f64,
    pub label: String,
    pub style: VertexStyle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub a: u64,
    pub b: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Graph {
    pub vertices: Vec<Vertex>,
    pub edges: Vec<Edge>,
    pub next_id: u64,
    /// Label for the next numbered vertex; independent of IDs so labeling
    /// can restart at 1 without disturbing stable references.
    pub next_label: u64,
}

impl Default for Graph {
    fn default() -> Self {
        Self {
            vertices: Vec::new(),
            edges: Vec::new(),
            next_id: 1,
            next_label: 1,
        }
    }
}

impl Graph {
    /// Add a vertex at finite, unoccupied coordinates. Callers must check these
    /// preconditions before insertion. Panics if the vertex ID space is exhausted.
    pub fn add_vertex(&mut self, x: f64, y: f64) -> u64 {
        self.add_styled_vertex(x, y, VertexStyle::default(), true)
    }

    /// Like [`Graph::add_vertex`], with an explicit drawing style. Unnumbered
    /// vertices get an empty label and do not consume a number.
    pub fn add_styled_vertex(&mut self, x: f64, y: f64, style: VertexStyle, numbered: bool) -> u64 {
        let id = self.next_id;
        self.next_id = id.checked_add(1).expect("vertex ID space exhausted");
        let label = if numbered {
            let label = self.next_label;
            self.next_label = label.saturating_add(1);
            label.to_string()
        } else {
            String::new()
        };
        self.vertices.push(Vertex {
            id,
            x,
            y,
            label,
            style,
        });
        id
    }

    /// Whether labels already read 1..=n in creation order with n + 1 next,
    /// so [`Graph::restart_labels`] would change nothing.
    pub fn labels_in_order(&self) -> bool {
        self.next_label == self.vertices.len() as u64 + 1
            && self
                .vertices
                .iter()
                .enumerate()
                .all(|(i, v)| v.label == (i + 1).to_string())
    }

    /// Number every vertex 1..=n in creation order; the next vertex gets n + 1.
    pub fn restart_labels(&mut self) {
        for (i, v) in self.vertices.iter_mut().enumerate() {
            v.label = (i + 1).to_string();
        }
        self.next_label = self.vertices.len() as u64 + 1;
    }

    pub fn remove_vertex(&mut self, id: u64) {
        self.vertices.retain(|vertex| vertex.id != id);
        self.edges.retain(|edge| edge.a != id && edge.b != id);
    }

    pub fn connect(&mut self, a: u64, b: u64) -> bool {
        if a == b
            || self.vertex(a).is_none()
            || self.vertex(b).is_none()
            || self
                .edges
                .iter()
                .any(|edge| (edge.a == a && edge.b == b) || (edge.a == b && edge.b == a))
        {
            return false;
        }
        self.edges.push(Edge { a, b });
        true
    }

    pub fn move_vertex(&mut self, id: u64, x: f64, y: f64) -> bool {
        if !x.is_finite()
            || !y.is_finite()
            || self
                .vertices
                .iter()
                .any(|v| v.id != id && v.x == x && v.y == y)
        {
            return false;
        }
        if let Some(vertex) = self.vertex_mut(id) {
            vertex.x = x;
            vertex.y = y;
            true
        } else {
            false
        }
    }

    /// Move every vertex in `ids` by (dx, dy) at once. Refuses (and changes
    /// nothing) if any moved vertex would land on a vertex that stays put, or
    /// if coordinates stop being finite or distinct.
    pub fn translate(&mut self, ids: &[u64], dx: f64, dy: f64) -> bool {
        let moving: HashSet<u64> = ids.iter().copied().collect();
        let mut occupied = HashSet::new();
        for v in self.vertices.iter().filter(|v| !moving.contains(&v.id)) {
            occupied.insert(position_key(v.x, v.y));
        }
        for v in self.vertices.iter().filter(|v| moving.contains(&v.id)) {
            let (x, y) = (v.x + dx, v.y + dy);
            if !x.is_finite() || !y.is_finite() || !occupied.insert(position_key(x, y)) {
                return false;
            }
        }
        for v in self.vertices.iter_mut().filter(|v| moving.contains(&v.id)) {
            v.x += dx;
            v.y += dy;
        }
        true
    }

    pub fn vertex(&self, id: u64) -> Option<&Vertex> {
        self.vertices.iter().find(|vertex| vertex.id == id)
    }

    pub fn vertex_mut(&mut self, id: u64) -> Option<&mut Vertex> {
        self.vertices.iter_mut().find(|vertex| vertex.id == id)
    }

    pub fn validate(&self) -> Result<(), String> {
        let mut ids = HashSet::new();
        let mut positions = HashSet::new();
        for vertex in &self.vertices {
            if !vertex.x.is_finite() || !vertex.y.is_finite() {
                return Err("vertex coordinates must be finite".into());
            }
            if vertex.id == 0 || !ids.insert(vertex.id) {
                return Err("vertex IDs must be unique and nonzero".into());
            }
            if !positions.insert(position_key(vertex.x, vertex.y)) {
                return Err("vertices must occupy distinct positions".into());
            }
            if vertex.id >= self.next_id {
                return Err("next_id must exceed every vertex ID".into());
            }
        }
        if self.next_id == 0 || self.next_id == u64::MAX {
            return Err("next_id must leave room for another vertex".into());
        }
        let mut edges = HashSet::new();
        for edge in &self.edges {
            if edge.a == edge.b || !ids.contains(&edge.a) || !ids.contains(&edge.b) {
                return Err("edges must join two existing distinct vertices".into());
            }
            if !edges.insert((edge.a.min(edge.b), edge.a.max(edge.b))) {
                return Err("duplicate edges are not allowed".into());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub x: f64,
    pub y: f64,
    pub zoom: f64,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            zoom: 48.0,
        }
    }
}

impl Camera {
    pub fn world_to_screen(&self, x: f64, y: f64, width: f64, height: f64) -> (f64, f64) {
        (
            width / 2.0 + (x - self.x) * self.zoom,
            height / 2.0 - (y - self.y) * self.zoom,
        )
    }
    pub fn screen_to_world(&self, sx: f64, sy: f64, width: f64, height: f64) -> (f64, f64) {
        (
            self.x + (sx - width / 2.0) / self.zoom,
            self.y - (sy - height / 2.0) / self.zoom,
        )
    }
    pub fn zoom_at(&mut self, factor: f64, sx: f64, sy: f64, width: f64, height: f64) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let (x, y) = self.screen_to_world(sx, sy, width, height);
        self.zoom = (self.zoom * factor).clamp(8.0, 256.0);
        let (after_x, after_y) = self.screen_to_world(sx, sy, width, height);
        self.x += x - after_x;
        self.y += y - after_y;
    }
    pub fn fit(&mut self, graph: &Graph, width: f64, height: f64) {
        if graph.vertices.is_empty() || width <= 0.0 || height <= 0.0 {
            return;
        }
        let (mut min_x, mut max_x, mut min_y, mut max_y) = (
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
        );
        for vertex in &graph.vertices {
            min_x = min_x.min(vertex.x);
            max_x = max_x.max(vertex.x);
            min_y = min_y.min(vertex.y);
            max_y = max_y.max(vertex.y);
        }
        self.x = min_x / 2.0 + max_x / 2.0;
        self.y = min_y / 2.0 + max_y / 2.0;
        // Leave one world unit of padding on each side; never magnify past
        // twice the default, so fitting one vertex does not fill the screen.
        self.zoom = (width / (max_x - min_x + 2.0))
            .min(height / (max_y - min_y + 2.0))
            .clamp(8.0, 2.0 * Camera::default().zoom);
    }
}

#[derive(Clone, Debug, Default)]
pub struct History {
    undo: Vec<Graph>,
    redo: Vec<Graph>,
    /// Bumped by every change to the stacks, so a caller can tell whether
    /// the last checkpoint is still its own (see [`History::discard`]).
    generation: u64,
    /// What the last checkpoint pushed out: the Redo it cleared and the
    /// oldest step it evicted at the cap, so discarding it loses nothing.
    displaced: (Vec<Graph>, Option<Graph>),
}

impl History {
    pub fn checkpoint(&mut self, graph: &Graph) {
        let evicted = Self::push_bounded(&mut self.undo, graph.clone());
        self.displaced = (std::mem::take(&mut self.redo), evicted);
        self.generation += 1;
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    /// The nearest state with any vertices on each side: in Undo, and in
    /// Redo.
    pub fn nearest_drawings(&self) -> [Option<&Graph>; 2] {
        fn near(stack: &[Graph]) -> Option<&Graph> {
            stack.iter().rev().find(|g| !g.vertices.is_empty())
        }
        [near(&self.undo), near(&self.redo)]
    }
    /// Take back the checkpoint made at `generation` as if it never
    /// happened: `graph` returns to the state before it, and the Redo and
    /// oldest step it displaced come back. False (and nothing changes) if
    /// anything was recorded since.
    pub fn discard(&mut self, graph: &mut Graph, generation: u64) -> bool {
        if generation != self.generation {
            return false;
        }
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        *graph = previous;
        let (redo, evicted) = std::mem::take(&mut self.displaced);
        self.redo = redo;
        if let Some(oldest) = evicted {
            self.undo.insert(0, oldest);
        }
        self.generation += 1;
        true
    }
    #[cfg(test)]
    pub fn depth(&self) -> usize {
        self.undo.len()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn undo(&mut self, graph: &mut Graph) -> bool {
        self.generation += 1;
        if let Some(previous) = self.undo.pop() {
            Self::push_bounded(&mut self.redo, std::mem::replace(graph, previous));
            true
        } else {
            false
        }
    }
    pub fn redo(&mut self, graph: &mut Graph) -> bool {
        self.generation += 1;
        if let Some(next) = self.redo.pop() {
            Self::push_bounded(&mut self.undo, std::mem::replace(graph, next));
            true
        } else {
            false
        }
    }
    /// Push, dropping (and returning) the oldest entry at the cap.
    fn push_bounded(stack: &mut Vec<Graph>, graph: Graph) -> Option<Graph> {
        let evicted = (stack.len() == 100).then(|| stack.remove(0));
        stack.push(graph);
        evicted
    }
}

/// Hashable position. Normalizes signed zero, which compares equal but has a
/// different bit representation.
fn position_key(x: f64, y: f64) -> (u64, u64) {
    let bits = |value: f64| if value == 0.0 { 0 } else { value.to_bits() };
    (bits(x), bits(y))
}

/// Compass directions for dot labels, in the order `typst/graph.typ` tries them.
pub const COMPASS: [(&str, (f64, f64)); 8] = [
    ("n", (0., 1.)),
    ("ne", (1., 1.)),
    ("e", (1., 0.)),
    ("se", (1., -1.)),
    ("s", (0., -1.)),
    ("sw", (-1., -1.)),
    ("w", (-1., 0.)),
    ("nw", (-1., 1.)),
];

/// `typst/graph.typ` defaults in coordinate units: (radius + label-gap) / unit
/// and (label-gap + 4pt) / unit with unit = 12mm.
pub const UNIT_PT: f64 = 12. * 72. / 25.4;
pub const LABEL_REACH: f64 = 8.8 / UNIT_PT;
pub const LABEL_CAP: f64 = 11. / UNIT_PT;

fn segment_distance(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length = dx * dx + dy * dy;
    let t = if length == 0. {
        0.
    } else {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / length).clamp(0., 1.)
    };
    (p.0 - a.0 - t * dx).hypot(p.1 - a.1 - t * dy)
}

impl Graph {
    /// Index into [`COMPASS`] for every vertex's label. Mirrors the automatic
    /// placement in `typst/graph.typ` so the editor previews the compiled PDF.
    pub fn label_directions(&self, reach: f64, cap: f64) -> HashMap<u64, usize> {
        let vs = &self.vertices;
        if vs.is_empty() {
            return HashMap::new();
        }
        let n = vs.len() as f64;
        let center = (
            vs.iter().map(|v| v.x).sum::<f64>() / n,
            vs.iter().map(|v| v.y).sum::<f64>() / n,
        );
        if vs.iter().all(|v| (v.y - center.1).abs() < 0.001) {
            return vs.iter().map(|v| (v.id, 0)).collect(); // a single row: all north
        }
        let at: HashMap<u64, (f64, f64)> = vs.iter().map(|v| (v.id, (v.x, v.y))).collect();
        let segments: Vec<_> = self.edges.iter().map(|e| (at[&e.a], at[&e.b])).collect();
        let mut first = Vec::with_capacity(vs.len());
        let mut clear = Vec::with_capacity(vs.len());
        for v in vs {
            let out = (v.x - center.0, v.y - center.1);
            let out_norm = out.0.hypot(out.1).max(1e-9);
            let (mut best, mut best_score) = (0, f64::NEG_INFINITY);
            let mut clearances = [0.; 8];
            for (i, (_, d)) in COMPASS.iter().enumerate() {
                let len = d.0.hypot(d.1);
                let spot = (v.x + d.0 / len * reach, v.y + d.1 / len * reach);
                let mut c = cap;
                for &(a, b) in &segments {
                    c = c.min(segment_distance(spot, a, b));
                }
                for u in vs.iter().filter(|u| u.id != v.id) {
                    c = c.min((spot.0 - u.x).hypot(spot.1 - u.y));
                }
                clearances[i] = c;
                let score = c / cap + 0.1 * (d.0 * out.0 + d.1 * out.1) / len / out_norm;
                if score > best_score + 1e-9 {
                    (best, best_score) = (i, score);
                }
            }
            first.push(best);
            clear.push(clearances);
        }
        // Consistency: rows share n/s labels and columns share e/w labels when
        // the shared direction is fully clear for this vertex.
        let (north, east, south, west) = (0, 2, 4, 6);
        vs.iter()
            .enumerate()
            .map(|(i, v)| {
                let mut votes = [0usize; 8];
                for (j, u) in vs.iter().enumerate() {
                    let d = first[j];
                    if j != i
                        && (((u.y - v.y).abs() < 1e-9 && (d == north || d == south))
                            || ((u.x - v.x).abs() < 1e-9 && (d == east || d == west)))
                    {
                        votes[d] += 1;
                    }
                }
                let mut pick = first[i];
                for k in 0..8 {
                    if clear[i][k] >= reach - 1e-9 && votes[k] > votes[pick] {
                        pick = k;
                    }
                }
                (v.id, pick)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }
    #[test]
    fn camera_round_trip_and_anchored_zoom() {
        let mut camera = Camera {
            x: 2.0,
            y: -3.0,
            zoom: 48.0,
        };
        let screen = camera.world_to_screen(4.5, -8.0, 800.0, 600.0);
        let world = camera.screen_to_world(screen.0, screen.1, 800.0, 600.0);
        close(world.0, 4.5);
        close(world.1, -8.0);
        camera.zoom_at(100.0, screen.0, screen.1, 800.0, 600.0);
        assert_eq!(camera.zoom, 256.0);
        let world = camera.screen_to_world(screen.0, screen.1, 800.0, 600.0);
        close(world.0, 4.5);
        close(world.1, -8.0);
    }
    #[test]
    fn connections_and_validation() {
        let mut graph = Graph::default();
        let a = graph.add_vertex(0.0, 0.0);
        let b = graph.add_vertex(1.0, 0.0);
        assert!(graph.connect(a, b));
        assert!(!graph.connect(b, a));
        assert!(!graph.connect(a, a));
        assert!(!graph.connect(a, 99));
        assert!(!graph.move_vertex(b, 0.0, 0.0));
        assert!(graph.validate().is_ok());
        graph.edges.push(Edge { a: b, b: a });
        assert!(graph.validate().is_err());
    }
    #[test]
    fn deleting_vertex_removes_incident_edges() {
        let mut graph = Graph::default();
        let a = graph.add_vertex(0.0, 0.0);
        let b = graph.add_vertex(1.0, 0.0);
        let c = graph.add_vertex(2.0, 0.0);
        graph.connect(a, b);
        graph.connect(b, c);
        graph.connect(a, c);
        graph.remove_vertex(b);
        assert_eq!(graph.edges, vec![Edge { a, b: c }]);
        assert!(graph.validate().is_ok());
    }
    #[test]
    fn history_undo_redo_and_branch() {
        let mut graph = Graph::default();
        let mut history = History::default();
        history.checkpoint(&graph);
        graph.add_vertex(0.0, 0.0);
        history.checkpoint(&graph);
        graph.add_vertex(1.0, 0.0);
        assert!(history.undo(&mut graph));
        assert_eq!(graph.vertices.len(), 1);
        assert!(history.undo(&mut graph));
        assert!(graph.vertices.is_empty());
        assert!(history.redo(&mut graph));
        assert_eq!(graph.vertices.len(), 1);
        history.checkpoint(&graph);
        graph.add_vertex(2.0, 0.0);
        assert!(!history.redo(&mut graph));
    }

    #[test]
    fn label_directions_match_typst_rule() {
        // A binary tree: labels share the row below; the apex label goes north.
        let mut graph = Graph::default();
        let pts = [
            (3., 1.),
            (0., 0.),
            (1., 0.),
            (2., 0.),
            (3., 0.),
            (4., 0.),
            (5., 0.),
            (6., 0.),
        ];
        for (x, y) in pts {
            graph.add_vertex(x, y);
        }
        for (a, b) in [(2, 3), (3, 4), (4, 1), (1, 7), (7, 8), (1, 5), (6, 7)] {
            graph.connect(a, b);
        }
        let dirs = graph.label_directions(LABEL_REACH, LABEL_CAP);
        assert_eq!(COMPASS[dirs[&1]].0, "n");
        assert!((2..=8).all(|id| COMPASS[dirs[&id]].0 == "s"));
    }
}
