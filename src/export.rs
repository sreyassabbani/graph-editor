use std::fmt::Write as _;

use anyhow::Result;

use crate::core::{Graph, VertexStyle};

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

// Pasteable graph IR. The shared `graph` helper owns all Fletcher details.
// Stable IDs are independent of labels and survive edits/deleted vertices.
pub fn export_typst(graph: &Graph) -> Result<String> {
    graph.validate().map_err(anyhow::Error::msg)?;
    if graph.vertices.is_empty() {
        return Ok(String::new());
    }
    let mut out = String::from("#graph(\n  vertices: (\n");
    // Put a uniform style at graph level; mixed graphs only annotate exceptions.
    let style = if graph
        .vertices
        .iter()
        .all(|v| v.style == graph.vertices[0].style)
    {
        graph.vertices[0].style
    } else {
        VertexStyle::Dots
    };
    for v in &graph.vertices {
        let override_style = if v.style == style {
            String::new()
        } else {
            format!(", \"{}\"", style_name(v.style))
        };
        writeln!(
            out,
            "    v{}: ({}, {}, {}{override_style}),",
            v.id,
            // Adding 0.0 turns -0.0 (from snapping just above an axis) into 0.
            v.x + 0.0,
            v.y + 0.0,
            content(&v.label)
        )
        .unwrap();
    }
    out.push_str("  ),\n");
    if !graph.edges.is_empty() {
        out.push_str("  edges: (\n");
        for edge in &graph.edges {
            writeln!(out, "    (\"v{}\", \"v{}\"),", edge.a, edge.b).unwrap();
        }
        out.push_str("  ),\n");
    }
    if style != VertexStyle::Dots {
        writeln!(out, "  style: \"{}\",", style_name(style)).unwrap();
    }
    out.push_str(")\n");
    Ok(out)
}

fn style_name(style: VertexStyle) -> &'static str {
    style.name()
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
        assert!(output.starts_with("#graph("));
        assert!(output.contains(r#"text("quote \" slash \\ $x$ #evil\n雪")"#));
        assert!(output.contains("v2: (1, 2, [2], \"circles\")"));
        assert!(output.contains("v3: (2, 0, [3], \"plain\")"));
        assert!(output.contains("(\"v1\", \"v2\")"));
        for boilerplate in [
            "#import",
            "#let",
            "fletcher.",
            "context",
            "node(",
            "edge(",
            "pt",
            "text.fill",
        ] {
            assert!(!output.contains(boilerplate));
        }
    }
    #[test]
    fn compact_ir_preserves_sparse_ids_and_uniform_style() {
        let mut graph = Graph::default();
        graph.add_vertex(-2., 2.);
        graph.add_vertex(-4., 5.);
        graph.add_vertex(1., 4.);
        graph.remove_vertex(2);
        graph.connect(3, 1);
        for v in &mut graph.vertices {
            v.style = VertexStyle::Circles;
        }
        assert_eq!(export_typst(&graph).unwrap(), "#graph(\n  vertices: (\n    v1: (-2, 2, [1]),\n    v3: (1, 4, [3]),\n  ),\n  edges: (\n    (\"v3\", \"v1\"),\n  ),\n  style: \"circles\",\n)\n");
    }
    #[test]
    fn negative_zero_coordinates_export_as_zero() {
        let mut graph = Graph::default();
        graph.add_vertex(-3., -0.);
        let output = export_typst(&graph).unwrap();
        assert!(output.contains("v1: (-3, 0, [1])"), "{output}");
    }
    #[test]
    fn empty_graph_is_empty_content() {
        assert_eq!(export_typst(&Graph::default()).unwrap(), "");
    }
}
