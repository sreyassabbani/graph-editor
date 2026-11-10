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

// Compile actual export output away from the repository: a hidden dependency on a
// local helper must not pass. Run this in the project's Nix/direnv shell.
#[test]
fn exports_fletcher_fragments_that_inherit_text_color() {
    let scratch =
        Scratch(std::env::temp_dir().join(format!("fletcher-export-{}", std::process::id())));
    fs::create_dir(&scratch.0).unwrap();
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
    for (case, graph) in [("mixed", graph), ("dots", dots)] {
        let module = export::export_typst(&graph).unwrap();
        assert!(module.contains("#context diagram("));
        assert!(module.contains("name: <v4>"));
        for (name, color) in [("normal", "#000000"), ("blue", "#007bff")] {
            let input = scratch.0.join(format!("{case}-{name}.typ"));
            let svg = scratch.0.join(format!("{case}-{name}.svg"));
            fs::write(&input, format!("#import \"@preview/fletcher:0.5.8\": diagram, node, edge\n#set page(width: auto, height: auto, margin: 5mm)\n#text(fill: rgb(\"{color}\"))[\n{module}\n]\n")).unwrap();
            let compiled = Command::new("typst")
                .arg("compile")
                .arg(&input)
                .arg(&svg)
                .output()
                .expect("run tests in the Nix/direnv shell, which provides Typst");
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
