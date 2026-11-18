use super::clipboard::copy_to_clipboard;
use super::*;

impl App {
    pub(super) fn copy(&mut self) {
        let message =
            match export::export_typst(&self.graph).and_then(|code| copy_to_clipboard(&code)) {
                Ok(()) => {
                    self.delivered = Some(self.graph.clone());
                    format!("Copied {} as Typst", self.graph_summary())
                }
                Err(e) => format!("Copy failed: {e:#}"),
            };
        self.say(message);
    }
    /// "5 vertices, 7 edges", "1 vertex".
    fn graph_summary(&self) -> String {
        let plural =
            |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let vertices = plural(self.graph.vertices.len(), "vertex", "vertices");
        match self.graph.edges.len() {
            0 => vertices,
            n => format!("{vertices}, {}", plural(n, "edge", "edges")),
        }
    }
}
