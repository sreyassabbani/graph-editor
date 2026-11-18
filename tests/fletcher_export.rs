use std::{fs, path::PathBuf, process::Command};

use graph_editor::{
    core::{Graph, VertexStyle},
    export,
};

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

// Compile the actual Copy IR against the bundled `typst/graph.typ` helper,
// in a scratch directory, in normal and coloured surrounding text. Needs
// `typst` on PATH (and network access the first time, to fetch Fletcher);
// skipped with a message when Typst is not installed.
#[test]
fn graph_ir_compiles_with_bundled_helper_in_surrounding_color() {
    if Command::new("typst").arg("--version").output().is_err() {
        eprintln!("skipping: `typst` is not on PATH");
        return;
    }
    let scratch =
        Scratch(std::env::temp_dir().join(format!("fletcher-export-{}", std::process::id())));
    fs::create_dir(&scratch.0).unwrap();
    let helper = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("typst/graph.typ");
    fs::copy(helper, scratch.0.join("graph.typ")).unwrap();
    let mut graph = Graph::default();
    graph.add_vertex(-1., 1.);
    graph.add_vertex(1., 1.);
    graph.add_vertex(0., -1.);
    graph.add_vertex(3., 0.); // isolated vertex
    graph.vertices[0].label = "quotes \" slash \\ $x$ #panic(\"no\")\nλ".into();
    graph.vertices[1].style = VertexStyle::Circles;
    graph.vertices[2].style = VertexStyle::Plain;
    graph.connect(1, 2);
    graph.connect(2, 3);
    let mut dots = graph.clone();
    for vertex in &mut dots.vertices {
        vertex.style = VertexStyle::Dots;
    }
    let mut circles = dots.clone();
    let mut plain = dots.clone();
    for vertex in &mut circles.vertices {
        vertex.style = VertexStyle::Circles;
    }
    for vertex in &mut plain.vertices {
        vertex.style = VertexStyle::Plain;
    }
    for (case, graph) in [
        ("mixed", graph),
        ("dots", dots),
        ("circles", circles),
        ("plain", plain),
    ] {
        let module = export::export_typst(&graph).unwrap();
        assert!(module.contains("#graph("));
        assert!(module.contains("v4: (3, 0, [4])"));
        for (import_name, import) in [("helper", "#import \"graph.typ\": graph")] {
            for (name, color) in [("normal", "#000000"), ("blue", "#007bff")] {
                let input = scratch.0.join(format!("{case}-{import_name}-{name}.typ"));
                let svg = scratch.0.join(format!("{case}-{import_name}-{name}.svg"));
                fs::write(&input, format!("{import}\n#set page(width: auto, height: auto, margin: 5mm)\n#text(fill: rgb(\"{color}\"))[\n{module}\n]\n")).unwrap();
                let compiled = Command::new("typst")
                    .arg("compile")
                    .arg(&input)
                    .arg(&svg)
                    .output()
                    .expect("running typst");
                assert!(
                    compiled.status.success(),
                    "{}",
                    String::from_utf8_lossy(&compiled.stderr)
                );
                let svg = fs::read_to_string(svg).unwrap();
                assert!(
                    svg.contains(color),
                    "export did not use surrounding text color {color}"
                );
                if name == "blue" {
                    assert!(!svg.contains("#000000"), "export contains fixed black ink");
                }
            }
        }
    }
}
