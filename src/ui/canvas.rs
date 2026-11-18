//! Describe the graph as SVG. usvg owns font discovery/shaping/measurement;
//! resvg owns antialiasing, compositing, masks and rasterization.
use super::*;
use resvg::{tiny_skia::Transform, usvg};
use std::collections::HashMap;
use std::{cell::RefCell, fmt::Write as _, rc::Rc, sync::OnceLock};

static FULL_FONTS: OnceLock<usvg::Options<'static>> = OnceLock::new();

/// Scanning every system font (~900 faces on macOS) takes ~100 ms. The canvas
/// only names two families, so early frames use just those files while
/// [`preload_fonts`] builds the full database (for Unicode fallback) off-thread.
pub(super) fn options() -> &'static usvg::Options<'static> {
    if let Some(full) = FULL_FONTS.get() {
        return full;
    }
    static QUICK: OnceLock<usvg::Options<'static>> = OnceLock::new();
    QUICK.get_or_init(|| {
        let mut options = usvg::Options::default();
        let db = options.fontdb_mut();
        for path in label_font_files() {
            let _ = db.load_font_file(path);
        }
        if db.is_empty() {
            return full_options();
        }
        options
    })
}

fn full_options() -> usvg::Options<'static> {
    let mut options = usvg::Options::default();
    options.fontdb_mut().load_system_fonts();
    options
}

pub(super) fn preload_fonts() {
    std::thread::spawn(|| FULL_FONTS.get_or_init(full_options));
}

/// Regular GeistMono Nerd Font Mono and Menlo, found by file name: walking
/// the font directories is cheap, parsing every face is not.
fn label_font_files() -> Vec<std::path::PathBuf> {
    // Menlo ships at a fixed path; walking all of /System/Library/Fonts costs ~25 ms.
    let menlo = std::path::PathBuf::from("/System/Library/Fonts/Menlo.ttc");
    let mut found: Vec<_> = menlo.exists().then_some(menlo).into_iter().collect();
    let mut dirs = vec![std::path::PathBuf::from("/Library/Fonts")];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(std::path::Path::new(&home).join("Library/Fonts"));
        dirs.push(std::path::Path::new(&home).join(".local/share/fonts"));
    }
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|t| t.is_dir()) || path.is_dir() {
                dirs.push(path);
            } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name == "Menlo.ttc" || name.starts_with("GeistMonoNerdFontMono-Regular.") {
                    if found.contains(&path) {
                        continue;
                    }
                    found.push(path);
                }
            }
        }
    }
    found
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

/// One label shaped and rasterized at native resolution. Text shaping is most
/// of a frame's cost, and labels rarely change between frames, so sprites are
/// cached per (text, size, pixel scale, font database) and blitted.
pub(super) struct LabelSprite {
    pix: Option<Pixmap>,
    /// Device-pixel offset of `pix` from the label's center point.
    offset: (i32, i32),
    /// Logical ink bounds relative to the center point: (left, top, right, bottom).
    bounds: Option<(f64, f64, f64, f64)>,
}

type LabelKey = (String, u64, u64, u64, usize);
thread_local! {
    static LABELS: RefCell<HashMap<LabelKey, Rc<LabelSprite>>> = RefCell::new(HashMap::new());
}

pub(super) fn label_sprite(
    text: &str,
    font: f64,
    scale: (f64, f64),
    fonts: &usvg::Options,
) -> Rc<LabelSprite> {
    let key = (
        text.to_owned(),
        font.to_bits(),
        scale.0.to_bits(),
        scale.1.to_bits(),
        fonts as *const _ as usize,
    );
    if let Some(hit) = LABELS.with(|c| c.borrow().get(&key).cloned()) {
        return hit;
    }
    let sprite = Rc::new(shape_label(text, font, scale, fonts));
    LABELS.with(|c| {
        let mut cache = c.borrow_mut();
        // Zoom gestures mint new sizes; keep the cache bounded.
        if cache.len() >= 2048 {
            cache.clear();
        }
        cache.insert(key, sprite.clone());
    });
    sprite
}

fn shape_label(text: &str, font: f64, scale: (f64, f64), fonts: &usvg::Options) -> LabelSprite {
    let empty = LabelSprite {
        pix: None,
        offset: (0, 0),
        bounds: None,
    };
    let svg = document(
        1.,
        1.,
        &format!(
            r#"<text x="0" y="0" text-anchor="middle" dominant-baseline="central" font-family="'GeistMono Nerd Font Mono', Menlo, monospace" font-size="{font}" fill="{}" xml:space="preserve">{}</text>"#,
            theme::color(theme::FOREGROUND),
            xml(text)
        ),
    );
    let Ok(tree) = usvg::Tree::from_str(&svg, fonts) else {
        return empty;
    };
    let b = tree.root().abs_stroke_bounding_box();
    if !(b.width() > 0. && b.height() > 0.) {
        return empty;
    }
    let bounds = (
        f64::from(b.x()),
        f64::from(b.y()),
        f64::from(b.right()),
        f64::from(b.bottom()),
    );
    let left = (bounds.0 * scale.0).floor() as i32 - 1;
    let top = (bounds.1 * scale.1).floor() as i32 - 1;
    let right = (bounds.2 * scale.0).ceil() as i32 + 1;
    let bottom = (bounds.3 * scale.1).ceil() as i32 + 1;
    let pix = Pixmap::new((right - left) as u32, (bottom - top) as u32).map(|mut pix| {
        resvg::render(
            &tree,
            Transform::from_row(
                scale.0 as f32,
                0.,
                0.,
                scale.1 as f32,
                -left as f32,
                -top as f32,
            ),
            &mut pix.as_mut(),
        );
        pix
    });
    LabelSprite {
        pix,
        offset: (left, top),
        bounds: Some(bounds),
    }
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

/// Marquee pieces at native resolution: a faint fill and 4-on/3-off dashes,
/// matching the SVG marquee drawn for non-Kitty terminals.
pub(super) fn render_marquee(
    width: u32,
    height: u32,
    scale: (f64, f64),
) -> Result<graphics::MarqueeSprites> {
    use resvg::tiny_skia::{Color, Paint, Rect};
    let [r, g, b] = theme::ACCENT;
    let mut fill = Pixmap::new(width, height).context("allocating marquee")?;
    fill.fill(Color::from_rgba8(r, g, b, (0.08 * 255.) as u8));
    let mut paint = Paint::default();
    paint.set_color(Color::from_rgba8(r, g, b, (0.8 * 255.) as u8));
    let thickness = (
        scale.0.round().max(1.) as u32,
        scale.1.round().max(1.) as u32,
    );
    let dashes = |length: u32, step: f64, horizontal: bool| -> Result<Pixmap> {
        let (w, h) = if horizontal {
            (length, thickness.1)
        } else {
            (thickness.0, length)
        };
        let mut pix = Pixmap::new(w, h).context("allocating marquee")?;
        let mut at = 0.;
        while at < f64::from(length) {
            let dash = (4. * step).min(f64::from(length) - at) as f32;
            let rect = if horizontal {
                Rect::from_xywh(at as f32, 0., dash, h as f32)
            } else {
                Rect::from_xywh(0., at as f32, w as f32, dash)
            };
            if let Some(rect) = rect {
                pix.fill_rect(rect, &paint, Transform::identity(), None);
            }
            at += 7. * step;
        }
        Ok(pix)
    };
    Ok(graphics::MarqueeSprites {
        horizontal: dashes(width, scale.0, true)?,
        vertical: dashes(height, scale.1, false)?,
        fill,
    })
}

/// Regions edges must not cross: circle nodes and plain-node labels. Edges are
/// clipped geometrically; an SVG mask would cost full-canvas offscreen layers.
enum Hole {
    Circle((f64, f64), f64),
    Rect((f64, f64), (f64, f64)),
}

impl Hole {
    /// Parameter interval of segment p + t(q - p) lying inside this hole.
    fn interval(&self, p: (f64, f64), q: (f64, f64)) -> Option<(f64, f64)> {
        let d = (q.0 - p.0, q.1 - p.1);
        let (lo, hi) = match *self {
            Hole::Circle(c, r) => {
                let f = (p.0 - c.0, p.1 - c.1);
                let a = d.0 * d.0 + d.1 * d.1;
                let b = f.0 * d.0 + f.1 * d.1;
                let disc = b * b - a * (f.0 * f.0 + f.1 * f.1 - r * r);
                if a == 0. || disc <= 0. {
                    return None;
                }
                let root = disc.sqrt();
                ((-b - root) / a, (-b + root) / a)
            }
            Hole::Rect(min, max) => {
                let (mut lo, mut hi) = (f64::NEG_INFINITY, f64::INFINITY);
                for (start, delta, a, b) in [(p.0, d.0, min.0, max.0), (p.1, d.1, min.1, max.1)] {
                    if delta == 0. {
                        if start <= a || start >= b {
                            return None;
                        }
                    } else {
                        let (t0, t1) = ((a - start) / delta, (b - start) / delta);
                        lo = lo.max(t0.min(t1));
                        hi = hi.min(t0.max(t1));
                    }
                }
                (lo, hi)
            }
        };
        let (lo, hi) = (lo.max(0.), hi.min(1.));
        (lo < hi).then_some((lo, hi))
    }
}

/// Visible parameter ranges of a segment after removing every hole.
fn visible_pieces(p: (f64, f64), q: (f64, f64), holes: &[Hole]) -> Vec<(f64, f64)> {
    let mut cuts: Vec<_> = holes.iter().filter_map(|h| h.interval(p, q)).collect();
    cuts.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut pieces = Vec::new();
    let mut start = 0.;
    for (lo, hi) in cuts {
        if lo > start {
            pieces.push((start, lo));
        }
        start = f64::max(start, hi);
    }
    if start < 1. {
        pieces.push((start, 1.));
    }
    pieces
}

pub(super) struct Scene {
    pub state: CanvasState,
    pub logical_size: (f64, f64),
    /// Extra device pixels rendered on each side (included in `state.size`
    /// and `logical_size`) so pans can be shown by cropping.
    pub margin: (u32, u32),
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
        // One font database per frame: the background full-font load must not
        // land between measuring plain labels and drawing them.
        let fonts = options();
        let scale = (width as f64 / w, height as f64 / h);
        let (svg, labels) = self.svg_scene(w, h, scale, fonts)?;
        let tree = usvg::Tree::from_str(&svg, fonts).context("parsing graph SVG")?;
        let mut pix = Pixmap::new(width, height).context("allocating canvas")?;
        self.draw_grid(&mut pix, w, h);
        // Geometry stays in logical units; render vectors and fonts directly at
        // the terminal's native pixel dimensions. Never enlarge a small bitmap.
        resvg::render(
            &tree,
            Transform::from_scale(scale.0 as f32, scale.1 as f32),
            &mut pix.as_mut(),
        );
        // Labels sit above edges and nodes, as they did inside the SVG.
        for (sprite, (x, y)) in labels {
            if let Some(label) = &sprite.pix {
                pix.draw_pixmap(
                    (x * scale.0).round() as i32 + sprite.offset.0,
                    (y * scale.1).round() as i32 + sprite.offset.1,
                    label.as_ref(),
                    &resvg::tiny_skia::PixmapPaint::default(),
                    Transform::identity(),
                    None,
                );
            }
        }
        Ok(pix)
    }
    /// Grid dots blended from small antialiased sprites, one per quarter-pixel
    /// phase. One path holding every dot costs ~20 ms on a Retina pane at
    /// zoom 12 (5000+ subpaths through the AA rasterizer); stamping is ~1 ms.
    fn draw_grid(&self, pix: &mut Pixmap, w: f64, h: f64) {
        use resvg::tiny_skia::{Color, FillRule, Paint, PathBuilder};
        const PHASES: usize = 4;
        let zoom = self.state.camera.zoom;
        let spacing = if zoom < 12. { (12. / zoom).ceil() } else { 1. } * zoom;
        let origin = self.state.camera.world_to_screen(0., 0., w, h);
        let start = (origin.0.rem_euclid(spacing), origin.1.rem_euclid(spacing));
        let scale = (pix.width() as f64 / w, pix.height() as f64 / h);
        let radius = 1.25 * scale.0.min(scale.1);
        let half = radius.ceil() as i32 + 1;
        let side = (2 * half + 1) as u32;
        let [r, g, b] = theme::GRID;
        let mut paint = Paint::default();
        paint.set_color(Color::from_rgba8(r, g, b, (0.9 * 255.) as u8));
        paint.anti_alias = true;
        let mut sprites = Vec::with_capacity(PHASES * PHASES);
        for py in 0..PHASES {
            for px in 0..PHASES {
                let mut sprite = Pixmap::new(side, side).expect("small sprite");
                let (cx, cy) = (
                    half as f64 + 0.5 + px as f64 / PHASES as f64,
                    half as f64 + 0.5 + py as f64 / PHASES as f64,
                );
                if let Some(path) = PathBuilder::from_circle(cx as f32, cy as f32, radius as f32) {
                    sprite.fill_path(
                        &path,
                        &paint,
                        FillRule::Winding,
                        Transform::identity(),
                        None,
                    );
                }
                sprites.push(sprite);
            }
        }
        let (width, height) = (pix.width() as i32, pix.height() as i32);
        let data = pix.data_mut();
        // Split a pixel-space center into an integer pixel and a phase index.
        let split = |c: f64| {
            let c = c - 0.5;
            let quarters = (c * PHASES as f64).round() as i64;
            (
                quarters.div_euclid(PHASES as i64) as i32,
                quarters.rem_euclid(PHASES as i64) as usize,
            )
        };
        // Start one step early so dots straddling the top/left edges are drawn.
        let mut y = start.1 - spacing;
        while y < h + spacing {
            let (row, py) = split(y * scale.1);
            let mut x = start.0 - spacing;
            while x < w + spacing {
                let (col, px) = split(x * scale.0);
                let sprite = sprites[py * PHASES + px].data();
                for sy in 0..side as i32 {
                    let ty = row - half + sy;
                    if !(0..height).contains(&ty) {
                        continue;
                    }
                    for sx in 0..side as i32 {
                        let tx = col - half + sx;
                        if !(0..width).contains(&tx) {
                            continue;
                        }
                        let src = ((sy * side as i32 + sx) * 4) as usize;
                        let alpha = u32::from(sprite[src + 3]);
                        if alpha == 0 {
                            continue;
                        }
                        // Premultiplied source-over.
                        let dst = ((ty * width + tx) * 4) as usize;
                        for k in 0..4 {
                            let d = u32::from(data[dst + k]);
                            data[dst + k] = (u32::from(sprite[src + k])
                                + (d * (255 - alpha) + 127) / 255)
                                as u8;
                        }
                    }
                }
                x += spacing;
            }
            y += spacing;
        }
    }
    /// The scene without labels, plus each label's sprite and center point.
    fn svg_scene(
        &self,
        w: f64,
        h: f64,
        scale: (f64, f64),
        fonts: &usvg::Options,
    ) -> Result<(String, Vec<(Rc<LabelSprite>, (f64, f64))>)> {
        let foreground = theme::color(theme::FOREGROUND);
        let accent = theme::color(theme::ACCENT);
        let mut labels = Vec::new();
        let mut nodes = String::new();
        let mut holes = Vec::new();
        let directions = self
            .state
            .graph
            .label_directions(crate::core::LABEL_REACH, crate::core::LABEL_CAP);
        // Sizes follow `typst/graph.typ` (12pt text, 1.8pt dots, 10pt circles,
        // 0.7pt strokes, labels centered 8.8pt out) scaled by zoom, so the
        // preview matches the PDF. Floors keep a zoomed-out view legible.
        let pt = self.state.camera.zoom / crate::core::UNIT_PT;
        // Quarter-pixel sizes so a zoom gesture reuses cached label sprites.
        let font = ((12. * pt).max(9.) * 4.).round() / 4.;
        let dot = (1.8 * pt).max(2.);
        let ring = (10. * pt).max(5.);
        let stroke = (0.7 * pt).max(1.);
        let reach = (crate::core::LABEL_REACH * self.state.camera.zoom).max(dot + font * 0.6);
        for v in &self.state.graph.vertices {
            let (x, y) = self.state.camera.world_to_screen(v.x, v.y, w, h);
            let color = if self.state.selection.has_vertex(v.id) {
                &accent
            } else {
                &foreground
            };
            match v.style {
                VertexStyle::Dots => {
                    write!(
                        nodes,
                        r#"<circle cx="{x}" cy="{y}" r="{dot}" fill="{color}"/>"#
                    )
                    .unwrap();
                }
                VertexStyle::Circles => {
                    holes.push(Hole::Circle((x, y), ring));
                    write!(nodes, r#"<circle cx="{x}" cy="{y}" r="{ring}" fill="none" stroke="{color}" stroke-width="{stroke}"/>"#).unwrap();
                }
                // A plain vertex with no label is a dot, as in `typst/graph.typ`.
                VertexStyle::Plain if v.label.is_empty() => {
                    write!(
                        nodes,
                        r#"<circle cx="{x}" cy="{y}" r="{dot}" fill="{color}"/>"#
                    )
                    .unwrap();
                }
                VertexStyle::Plain => {}
            }
            // Fletcher centers the label node on the offset point.
            let (lx, ly) = if v.style == VertexStyle::Dots {
                // World y points up; screen y points down.
                let (dx, dy) = crate::core::COMPASS[directions[&v.id]].1;
                let len = dx.hypot(dy);
                (x + dx / len * reach, y - dy / len * reach)
            } else {
                (x, y)
            };
            let sprite = label_sprite(&v.label, font, scale, fonts);
            // Plain labels take the vertex's place, so edges stop at their
            // shaped bounds.
            if let (VertexStyle::Plain, Some(b)) = (v.style, sprite.bounds) {
                holes.push(Hole::Rect(
                    (lx + b.0 - 3., ly + b.1 - 3.),
                    (lx + b.2 + 3., ly + b.3 + 3.),
                ));
            }
            labels.push((sprite, (lx, ly)));
            if self.state.edge_start == Some(v.id) {
                write!(nodes, r#"<circle cx="{x}" cy="{y}" r="{}" fill="none" stroke="{accent}" stroke-width="1.5"/>"#, ring.max(dot) + 5.).unwrap();
            }
        }
        let mut scene = String::new();
        if let Some((x, y)) = self.state.hover {
            let p = self.state.camera.world_to_screen(x, y, w, h);
            let size = self.state.camera.zoom;
            write!(scene, r#"<rect x="{}" y="{}" width="{size}" height="{size}" fill="{accent}" fill-opacity="0.12" stroke="{accent}" stroke-opacity="0.7" stroke-width="1"/>"#, p.0 - size / 2., p.1 - size / 2.).unwrap();
        }
        write!(
            scene,
            r#"<g stroke="{foreground}" stroke-width="{stroke}" stroke-linecap="round">"#
        )
        .unwrap();
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
                let highlight = if self.state.selection.has_edge(edge) {
                    format!(r#" stroke="{accent}" stroke-width="{}""#, stroke + 1.)
                } else {
                    String::new()
                };
                let at = |t: f64| (p.0 + t * (q.0 - p.0), p.1 + t * (q.1 - p.1));
                for (t0, t1) in visible_pieces(p, q, &holes) {
                    let (a, b) = (at(t0), at(t1));
                    write!(
                        scene,
                        r#"<line x1="{}" y1="{}" x2="{}" y2="{}"{highlight}/>"#,
                        a.0, a.1, b.0, b.1
                    )
                    .unwrap();
                }
            }
        }
        write!(scene, "</g>{nodes}").unwrap();
        if let Some((a, b)) = self.state.marquee {
            let (x, y) = (a.0.min(b.0), a.1.min(b.1));
            let (rw, rh) = ((a.0 - b.0).abs(), (a.1 - b.1).abs());
            write!(scene, r#"<rect x="{x}" y="{y}" width="{rw}" height="{rh}" fill="{accent}" fill-opacity="0.08" stroke="{accent}" stroke-opacity="0.8" stroke-width="1" stroke-dasharray="4 3"/>"#).unwrap();
        }
        Ok((document(w, h, &scene), labels))
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
    fn labels_are_literal_unicode_shaped_once_and_cached() {
        let fonts = options();
        let sprite = label_sprite("λ<&", 12., (2., 2.), fonts);
        let (l, t, r, b) = sprite.bounds.unwrap();
        // Centered on the anchor, and the markup characters became glyphs.
        assert!(l < 0. && r > 0. && t < 0. && b > 0.);
        assert!(r - l > label_sprite("λ", 12., (2., 2.), fonts).bounds.unwrap().2 * 2.);
        assert!(sprite
            .pix
            .as_ref()
            .unwrap()
            .pixels()
            .iter()
            .any(|p| p.alpha() > 0));
        assert!(Rc::ptr_eq(
            &sprite,
            &label_sprite("λ<&", 12., (2., 2.), fonts)
        ));
        assert!(label_sprite("", 12., (2., 2.), fonts).pix.is_none());
    }
    #[test]
    fn edges_are_clipped_out_of_circle_and_label_holes() {
        let holes = [
            Hole::Circle((0., 0.), 9.),
            Hole::Rect((40., -5.), (60., 5.)),
        ];
        let pieces = visible_pieces((0., 0.), (100., 0.), &holes);
        assert_eq!(pieces.len(), 2);
        assert!((pieces[0].0 - 0.09).abs() < 1e-9 && (pieces[0].1 - 0.4).abs() < 1e-9);
        assert!((pieces[1].0 - 0.6).abs() < 1e-9 && pieces[1].1 == 1.);
        // Misses and fully hidden segments.
        assert_eq!(
            visible_pieces((0., 20.), (100., 20.), &holes),
            vec![(0., 1.)]
        );
        assert!(visible_pieces((-1., 0.), (1., 0.), &holes).is_empty());
    }
    /// `cargo test --release frame_benchmark -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn frame_benchmark() {
        let mut app = super::super::test_support::app();
        for i in 1..30 {
            let id = app
                .graph
                .add_vertex((i % 6) as f64 * 2., (i / 6) as f64 * 2.);
            if i % 3 == 0 {
                app.graph.vertex_mut(id).unwrap().style = VertexStyle::Circles;
            }
        }
        for i in 1..29u64 {
            app.graph.connect(i, i + 1);
        }
        app.graph.vertex_mut(2).unwrap().style = VertexStyle::Plain;
        // A 1600x1000 canvas, and a real Retina pane (100x59 cells) at the
        // default zoom and at the densest grid zoom.
        for transport in [
            graphics::Transport::Inline,
            graphics::Transport::SharedMemory,
        ] {
            for (size, logical, zoom) in [
                ((1600, 1000), (800., 500.), 48.),
                ((1900, 2478), (760., 944.), 48.),
                ((1900, 2478), (760., 944.), 12.),
            ] {
                let mut scene = app.scene();
                scene.state.size = size;
                scene.logical_size = logical;
                scene.state.camera.zoom = zoom;
                let _ = scene.render().unwrap(); // load fonts once
                let n = 20;
                let (mut grid, mut raster, mut encode) = (0., 0., 0.);
                let mut bytes = 0;
                for _ in 0..n {
                    let mut pix = Pixmap::new(size.0, size.1).unwrap();
                    let t = Instant::now();
                    scene.draw_grid(&mut pix, logical.0, logical.1);
                    grid += t.elapsed().as_secs_f64();
                    let t = Instant::now();
                    let pix = scene.render().unwrap();
                    raster += t.elapsed().as_secs_f64();
                    let t = Instant::now();
                    let frame = graphics::encode_frame(pix, transport, false).unwrap();
                    encode += t.elapsed().as_secs_f64();
                    bytes = frame.len();
                }
                let ms = |s: f64| s / f64::from(n) * 1000.;
                eprintln!(
                "{transport:?} {}x{} zoom {zoom}: render {:.2} ms (grid {:.2}), encode {:.2} ms, {} KB sent",
                size.0,
                size.1,
                ms(raster),
                ms(grid),
                ms(encode),
                bytes / 1024
            );
            }
        }
        graphics::shm::unlink_all();
    }
}
