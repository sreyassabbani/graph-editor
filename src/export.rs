use std::fmt::Write as _;
use std::path::Path;

use anyhow::Result;

use crate::core::{Graph, Vertex, VertexStyle};
use crate::fileio::atomic_write;

// Typst strings are literal text; never place user labels in markup or math mode.
fn typst_string(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                write!(out, "\\u{{{:x}}}", c as u32).unwrap();
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// This is a pasteable fragment. Fletcher's diagram/node/edge are already in
// scope; keep imports, helpers and document-specific wrappers out of the output.
pub fn export_typst(graph: &Graph) -> Result<String> {
    graph.validate().map_err(anyhow::Error::msg)?;
    if graph.vertices.is_empty() {
        return Ok(String::new());
    }
    let dots_only = graph.vertices.iter().all(|v| v.style == VertexStyle::Dots);
    let mut out = String::from("#context diagram(\n  axes: (ltr, btt), cell-size: 12mm, spacing: 0pt,\n  node-inset: 0pt, edge-stroke: 0.7pt + text.fill,\n");
    if dots_only {
        out.push_str("  node-fill: text.fill,\n");
    }
    out.push('\n');
    for v in &graph.vertices {
        write_vertex(&mut out, v, dots_only);
    }
    let labelled_dots: Vec<_> = graph
        .vertices
        .iter()
        .filter(|v| v.style == VertexStyle::Dots && !v.label.is_empty())
        .collect();
    if !labelled_dots.is_empty() {
        // Normalize before finding the centroid to avoid overflow for finite,
        // extreme coordinates. Only label direction depends on the centroid.
        let scale = graph
            .vertices
            .iter()
            .fold(1_f64, |s, v| s.max(v.x.abs()).max(v.y.abs()));
        let count = graph.vertices.len() as f64;
        let cx = graph
            .vertices
            .iter()
            .map(|v| v.x / scale / count)
            .sum::<f64>();
        let cy = graph
            .vertices
            .iter()
            .map(|v| v.y / scale / count)
            .sum::<f64>();
        let horizontal = graph
            .vertices
            .iter()
            .all(|v| (v.y / scale - cy).abs() < 0.001 / scale);
        out.push('\n');
        for v in labelled_dots {
            let (dx, dy) = label_offset(v.x / scale - cx, v.y / scale - cy, horizontal);
            let fill = if dots_only { ", fill: none" } else { "" };
            writeln!(
                out,
                "  node((rel: ({dx}pt, {dy}pt), to: <v{}>), {}, snap: false{fill}),",
                v.id,
                content(&v.label)
            )
            .unwrap();
        }
    }
    if !graph.edges.is_empty() {
        out.push('\n');
    }
    for edge in &graph.edges {
        writeln!(out, "  edge(<v{}>, <v{}>),", edge.a, edge.b).unwrap();
    }
    out.push_str(")\n");
    Ok(out)
}

fn write_vertex(out: &mut String, v: &Vertex, dots_only: bool) {
    let position = format!("({}, {})", v.x, v.y);
    match v.style {
        VertexStyle::Dots => {
            let fill = if dots_only { "" } else { ", fill: text.fill" };
            writeln!(
                out,
                "  node({position}, name: <v{}>, radius: 1.8pt{fill}),",
                v.id
            )
            .unwrap();
        }
        VertexStyle::Circles => {
            writeln!(out, "  node({position}, {}, name: <v{}>, radius: 10pt, inset: 3pt, stroke: 0.7pt + text.fill),", content(&v.label), v.id).unwrap();
        }
        VertexStyle::Plain => {
            writeln!(
                out,
                "  node({position}, {}, name: <v{}>, inset: 3pt),",
                content(&v.label),
                v.id
            )
            .unwrap();
        }
    }
}

// Keep ordinary labels readable. Complex strings use text() so literal markup,
// newlines and Unicode retain their exact meaning when pasted into Typst.
fn content(label: &str) -> String {
    if label.trim() == label
        && !label.contains("  ")
        && label.chars().all(|c| c.is_alphanumeric() || c == ' ')
    {
        format!("[{label}]")
    } else {
        format!("text({})", typst_string(label))
    }
}

fn label_offset(dx: f64, dy: f64, horizontal: bool) -> (i8, i8) {
    if horizontal {
        return (0, 9);
    }
    if dx == 0. && dy == 0. {
        return (9, 0);
    }
    let sx = if dx < 0. { -1 } else { 1 };
    let sy = if dy < 0. { -1 } else { 1 };
    let threshold = (std::f64::consts::PI / 8.).tan();
    if dy.abs() <= dx.abs() * threshold {
        (sx * 9, 0)
    } else if dx.abs() <= dy.abs() * threshold {
        (0, sy * 9)
    } else {
        (sx * 6, sy * 6)
    }
}

pub fn write_typst(path: &Path, graph: &Graph) -> Result<()> {
    atomic_write(path, export_typst(graph)?.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exports_readable_fragments_with_literal_labels_and_mixed_styles() {
        let mut graph = Graph::default();
        graph.add_vertex(0., 0.);
        graph.add_vertex(1., 2.);
        graph.vertices[0].label = "quote \" slash \\ $x$ #evil\n雪".into();
        graph.vertices[1].style = VertexStyle::Circles;
        graph.add_vertex(2., 0.);
        graph.vertices[2].style = VertexStyle::Plain;
        graph.connect(1, 2);
        let output = export_typst(&graph).unwrap();
        assert!(output.starts_with("#context diagram("));
        assert!(output.contains("text(\"quote \\\" slash \\\\ $x$ #evil\\n雪\")"));
        assert!(output.contains("node((1, 2), [2], name: <v2>"));
        assert!(output.contains("node((2, 0), [3], name: <v3>"));
        assert!(output.contains("edge(<v1>, <v2>)"));
        for boilerplate in ["#import", "#let", "fletcher.", "#drawing"] {
            assert!(!output.contains(boilerplate));
        }
    }
    #[test]
    fn example_uses_short_outward_label_offsets() {
        let mut graph = Graph::default();
        graph.add_vertex(-2., 2.);
        graph.add_vertex(-4., 5.);
        graph.add_vertex(1., 4.);
        graph.connect(1, 2);
        graph.connect(3, 1);
        graph.connect(2, 3);
        let output = export_typst(&graph).unwrap();
        assert!(output.contains("rel: (0pt, -9pt)"));
        assert!(output.contains("rel: (-6pt, 6pt)"));
        assert!(output.contains("rel: (9pt, 0pt)"));
        assert_eq!(output.matches("node-fill: text.fill").count(), 1);
        assert!(output.lines().count() < 22);
    }
    #[test]
    fn empty_graph_is_empty_content() {
        assert_eq!(export_typst(&Graph::default()).unwrap(), "");
    }
}
