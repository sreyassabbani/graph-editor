use std::collections::HashSet;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VertexStyle {
    #[default]
    Dots,
    Circles,
    Plain,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Vertex {
    pub id: u64,
    pub x: f64,
    pub y: f64,
    pub label: String,
    pub style: VertexStyle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edge {
    pub a: u64,
    pub b: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Graph {
    pub vertices: Vec<Vertex>,
    pub edges: Vec<Edge>,
    pub next_id: u64,
}

impl Default for Graph {
    fn default() -> Self {
        Self {
            vertices: Vec::new(),
            edges: Vec::new(),
            next_id: 1,
        }
    }
}

impl Graph {
    /// Add a vertex at finite, unoccupied coordinates. Callers must check these
    /// preconditions before insertion. Panics if the vertex ID space is exhausted.
    pub fn add_vertex(&mut self, x: f64, y: f64) -> u64 {
        let id = self.next_id;
        self.next_id = id.checked_add(1).expect("vertex ID space exhausted");
        self.vertices.push(Vertex {
            id,
            x,
            y,
            label: id.to_string(),
            style: VertexStyle::default(),
        });
        id
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
            // Normalize signed zero, which compares equal but has a different bit representation.
            let bits = |value: f64| if value == 0.0 { 0 } else { value.to_bits() };
            if !positions.insert((bits(vertex.x), bits(vertex.y))) {
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
        // Leave one world unit of padding on each side, including single vertices.
        self.zoom = (width / (max_x - min_x + 2.0))
            .min(height / (max_y - min_y + 2.0))
            .clamp(8.0, 256.0);
    }
}

#[derive(Clone, Debug, Default)]
pub struct History {
    undo: Vec<Graph>,
    redo: Vec<Graph>,
}

impl History {
    pub fn checkpoint(&mut self, graph: &Graph) {
        Self::push_bounded(&mut self.undo, graph.clone());
        self.redo.clear();
    }
    pub fn undo(&mut self, graph: &mut Graph) -> bool {
        if let Some(previous) = self.undo.pop() {
            Self::push_bounded(&mut self.redo, std::mem::replace(graph, previous));
            true
        } else {
            false
        }
    }
    pub fn redo(&mut self, graph: &mut Graph) -> bool {
        if let Some(next) = self.redo.pop() {
            Self::push_bounded(&mut self.undo, std::mem::replace(graph, next));
            true
        } else {
            false
        }
    }
    fn push_bounded(stack: &mut Vec<Graph>, graph: Graph) {
        if stack.len() == 100 {
            stack.remove(0);
        }
        stack.push(graph);
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
}
