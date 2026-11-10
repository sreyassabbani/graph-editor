//! Describe the graph as SVG. usvg owns font discovery/shaping/measurement;
//! resvg owns antialiasing, compositing, masks and rasterization.
use super::*;
use resvg::{tiny_skia::Transform, usvg};
use std::collections::HashMap;
use std::{fmt::Write as _, sync::OnceLock};

fn options() -> &'static usvg::Options<'static> {
    static OPTIONS: OnceLock<usvg::Options<'static>> = OnceLock::new();
    OPTIONS.get_or_init(|| {
        let mut options = usvg::Options::default();
        options.fontdb_mut().load_system_fonts();
        options
    })
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn document(w: f64, h: f64, body: &str) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">{body}</svg>"#
    )
}

/// A tiny native-resolution SVG sprite. Kitty moves its placement independently
/// of the graph; resvg still owns all rasterization.
pub(super) fn render_hover(width: u32, height: u32, scale: (f64, f64)) -> Result<Pixmap> {
    let accent = theme::color(theme::ACCENT);
    let svg = document(
        width as f64,
        height as f64,
        &format!(
            r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{accent}" fill-opacity="0.12" stroke="{accent}" stroke-opacity="0.7" stroke-width="{}"/>"#,
            scale.0,
            scale.1,
            width as f64 - 2. * scale.0,
            height as f64 - 2. * scale.1,
            scale.0
        ),
    );
    let tree = usvg::Tree::from_str(&svg, &usvg::Options::default())?;
    let mut pix = Pixmap::new(width, height).context("allocating hover sprite")?;
    resvg::render(&tree, Transform::identity(), &mut pix.as_mut());
    Ok(pix)
}

pub(super) struct Scene {
    pub state: CanvasState,
    pub logical_size: (f64, f64),
}

impl App {
    pub(super) fn pixel_size(&self) -> (u32, u32) {
        let cell = self.picker.font_size();
        (
            self.area.width as u32 * cell.0 as u32,
            self.area.height as u32 * cell.1 as u32,
        )
    }
    pub(super) fn hover_bounds(&self) -> Option<graphics::HoverRect> {
        self.hover.map(|(x, y)| {
            let (w, h) = self.size();
            let p = self.camera.world_to_screen(x, y, w, h);
            let cell = self.picker.font_size();
            let scale = (cell.0 as f64 / self.cell.0, cell.1 as f64 / self.cell.1);
            let side = self.camera.zoom + 2.;
            graphics::HoverRect {
                left: ((p.0 - side / 2.) * scale.0).floor() as i32,
                top: ((p.1 - side / 2.) * scale.1).floor() as i32,
                width: (side * scale.0).ceil() as u32,
                height: (side * scale.1).ceil() as u32,
            }
        })
    }
}

impl Scene {
    pub(super) fn render(&self) -> Result<Pixmap> {
        let (width, height) = self.state.size;
        anyhow::ensure!(
            width as u64 * height as u64 <= 25_000_000,
            "canvas too large"
        );
        let (w, h) = self.logical_size;
        anyhow::ensure!(w > 0. && h > 0., "empty canvas");
        let svg = self.svg_scene(w, h)?;
        let tree = usvg::Tree::from_str(&svg, options()).context("parsing graph SVG")?;
        let mut pix = Pixmap::new(width, height).context("allocating canvas")?;
        // Geometry stays in logical units; render vectors and fonts directly at
        // the terminal's native pixel dimensions. Never enlarge a small bitmap.
        resvg::render(
            &tree,
            Transform::from_scale(width as f32 / w as f32, height as f32 / h as f32),
            &mut pix.as_mut(),
        );
        Ok(pix)
    }
    fn svg_scene(&self, w: f64, h: f64) -> Result<String> {
        let foreground = theme::color(theme::FOREGROUND);
        let accent = theme::color(theme::ACCENT);
        let grid = theme::color(theme::GRID);
        let spacing = if self.state.camera.zoom < 12. {
            (12. / self.state.camera.zoom).ceil()
        } else {
            1.
        } * self.state.camera.zoom;
        let origin = self.state.camera.world_to_screen(0., 0., w, h);
        let (ox, oy) = (
            origin.0.rem_euclid(spacing) - spacing / 2.,
            origin.1.rem_euclid(spacing) - spacing / 2.,
        );
        let middle = spacing / 2.;
        let mut labels = String::new();
        let mut nodes = String::new();
        let mut holes = String::new();
        for v in &self.state.graph.vertices {
            let (x, y) = self.state.camera.world_to_screen(v.x, v.y, w, h);
            let color = if self.state.selected == Some(v.id) {
                &accent
            } else {
                &foreground
            };
            match v.style {
                VertexStyle::Dots => {
                    write!(nodes, r#"<circle cx="{x}" cy="{y}" r="5" fill="{color}"/>"#).unwrap();
                }
                VertexStyle::Circles => {
                    write!(holes, r#"<circle cx="{x}" cy="{y}" r="9" fill="black"/>"#).unwrap();
                    write!(nodes, r#"<circle cx="{x}" cy="{y}" r="9" fill="none" stroke="{color}" stroke-width="1.5"/>"#).unwrap();
                }
                VertexStyle::Plain => {}
            }
            let (lx, ly, anchor, baseline) = if v.style == VertexStyle::Dots {
                (x + 10., y - 7., "start", "alphabetic")
            } else {
                (x, y, "middle", "central")
            };
            write!(labels, r#"<text id="label-{}" x="{lx}" y="{ly}" text-anchor="{anchor}" dominant-baseline="{baseline}" font-family="'GeistMono Nerd Font Mono', Menlo, monospace" font-size="12" fill="{foreground}" xml:space="preserve">{}</text>"#, v.id, xml(&v.label)).unwrap();
            if self.state.edge_start == Some(v.id) {
                write!(nodes, r#"<circle cx="{x}" cy="{y}" r="15" fill="none" stroke="{accent}" stroke-width="1.5"/>"#).unwrap();
            }
        }
        // Ask usvg for shaped text bounds instead of estimating character widths.
        // Only plain nodes need this measurement pass to mask incident edges.
        if self
            .state
            .graph
            .vertices
            .iter()
            .any(|v| v.style == VertexStyle::Plain)
        {
            let measured = usvg::Tree::from_str(&document(w, h, &labels), options())?;
            for v in self
                .state
                .graph
                .vertices
                .iter()
                .filter(|v| v.style == VertexStyle::Plain)
            {
                if let Some(label) = measured.node_by_id(&format!("label-{}", v.id)) {
                    let b = label.abs_bounding_box();
                    write!(
                        holes,
                        r#"<rect x="{}" y="{}" width="{}" height="{}" fill="black"/>"#,
                        b.x() - 3.,
                        b.y() - 3.,
                        b.width() + 6.,
                        b.height() + 6.
                    )
                    .unwrap();
                }
            }
        }
        let mut scene = format!(
            r#"<defs><pattern id="grid" patternUnits="userSpaceOnUse" x="{ox}" y="{oy}" width="{spacing}" height="{spacing}"><circle cx="{middle}" cy="{middle}" r="1.25" fill="{grid}" opacity="0.9"/></pattern><mask id="node-holes" maskUnits="userSpaceOnUse" x="0" y="0" width="{w}" height="{h}"><rect width="{w}" height="{h}" fill="white"/>{holes}</mask></defs><rect width="{w}" height="{h}" fill="url(#grid)"/>"#
        );
        if let Some((x, y)) = self.state.hover {
            let p = self.state.camera.world_to_screen(x, y, w, h);
            let size = self.state.camera.zoom;
            write!(scene, r#"<rect x="{}" y="{}" width="{size}" height="{size}" fill="{accent}" fill-opacity="0.12" stroke="{accent}" stroke-opacity="0.7" stroke-width="1"/>"#, p.0 - size / 2., p.1 - size / 2.).unwrap();
        }
        write!(scene, r#"<g stroke="{foreground}" stroke-width="1.5" stroke-linecap="round" mask="url(#node-holes)">"#).unwrap();
        // Resolve endpoints once instead of scanning all vertices per edge.
        let vertices: HashMap<_, _> = self
            .state
            .graph
            .vertices
            .iter()
            .map(|v| (v.id, v))
            .collect();
        for edge in &self.state.graph.edges {
            if let (Some(a), Some(b)) = (vertices.get(&edge.a), vertices.get(&edge.b)) {
                let p = self.state.camera.world_to_screen(a.x, a.y, w, h);
                let q = self.state.camera.world_to_screen(b.x, b.y, w, h);
                write!(
                    scene,
                    r#"<line x1="{}" y1="{}" x2="{}" y2="{}"/>"#,
                    p.0, p.1, q.0, q.1
                )
                .unwrap();
            }
        }
        write!(scene, "</g>{nodes}{labels}").unwrap();
        Ok(document(w, h, &scene))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_pixels_and_transparent_background_survive_png_encoding() {
        let mut app = super::super::test_support::app();
        app.graph.vertices[0].style = VertexStyle::Circles;
        let other = app.graph.add_vertex(2., 0.);
        app.graph.connect(1, other);
        let (w, h) = app.pixel_size();
        let pix = app.scene().render().unwrap();
        assert_eq!(pix.pixel(1, 1).unwrap().alpha(), 0);
        assert_eq!(pix.pixel(w / 2 - 3, h / 2 + 6).unwrap().alpha(), 0);
        let decoded = image::load_from_memory(&pix.encode_png().unwrap()).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (w, h));
    }
    #[test]
    fn svg_labels_are_literal_unicode_and_use_native_font_shaping() {
        let mut app = super::super::test_support::app();
        app.graph.vertices[0].label = "λ<&".into();
        let (w, h) = app.size();
        let scene = app.scene().svg_scene(w, h).unwrap();
        assert!(scene.contains("λ&lt;&amp;"));
        let tree = usvg::Tree::from_str(&scene, options()).unwrap();
        let bounds = tree.node_by_id("label-1").unwrap().abs_bounding_box();
        assert!(bounds.width() > 0. && bounds.height() > 0.);
    }
}
