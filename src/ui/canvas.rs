//! Describe the graph as SVG. usvg owns font discovery/shaping/measurement;
//! resvg owns antialiasing, compositing, masks and rasterization.
use super::*;
use resvg::{tiny_skia::Transform, usvg};
use std::collections::HashMap;
use std::f64::consts::{PI, TAU};
use std::{cell::RefCell, fmt::Write as _, rc::Rc, sync::OnceLock};

/// The pen ring's turn is stored in thousandths of a dash.
pub(super) const SPIN_STEPS: u16 = 1000;

/// Width of the pen ring's dashes.
const PEN_WIDTH: f64 = 1.5;

/// Distance along the pen ring from one dash to the next: about 7 px, an
/// exact fraction of the circumference so the dashes meet seamlessly.
fn pen_period(r: f64) -> f64 {
    let c = TAU * r;
    c / (c / 7.).round().max(4.)
}

/// Curves for `t` in 0..=1 (clamped). The unroll leaves the spool slowly and
/// accelerates hard (quartic in), then coasts onto the far vertex (cubic out),
/// so the two halves meet at full speed and read as one motion.
fn ease_in_quart(t: f64) -> f64 {
    t.clamp(0., 1.).powi(4)
}
fn ease_out_cubic(t: f64) -> f64 {
    1. - (1. - t.clamp(0., 1.)).powi(3)
}
/// A soft symmetric curve for color.
fn ease_in_out_sine(t: f64) -> f64 {
    (1. - (PI * t.clamp(0., 1.)).cos()) / 2.
}

/// A piece of the pen's dashed ring around `c`, from angle `start` (screen
/// radians, clockwise from 3 o'clock) clockwise through `sweep`. The dashes
/// sit where the whole ring has them at `spin`, turned a further `turn`
/// radians clockwise: the ring spinning as it feeds out or winds on.
#[allow(clippy::too_many_arguments)]
fn pen_arc(
    out: &mut String,
    c: (f64, f64),
    r: f64,
    start: f64,
    sweep: f64,
    spin: u16,
    turn: f64,
    color: &str,
) {
    if sweep <= 0.02 {
        return;
    }
    let period = pen_period(r);
    let offset =
        (r * (start - turn) - period * f64::from(spin) / f64::from(SPIN_STEPS)).rem_euclid(period);
    let at = |a: f64| (c.0 + r * a.cos(), c.1 + r * a.sin());
    let p = at(start);
    write!(out, r#"<path d="M{} {}"#, p.0, p.1).unwrap();
    // At most a half turn per arc command, so a full ring needs no special case.
    let mut done = 0.;
    while done < sweep.min(TAU) - 1e-9 {
        done = (done + PI).min(sweep.min(TAU));
        let q = at(start + done);
        write!(out, " A{r} {r} 0 0 1 {} {}", q.0, q.1).unwrap();
    }
    write!(
        out,
        r#"" fill="none" stroke="{color}" stroke-width="{PEN_WIDTH}" stroke-linecap="round" stroke-dasharray="{} {}" stroke-dashoffset="{offset}"/>"#,
        period * 0.45,
        period * 0.55
    )
    .unwrap();
}

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

/// Mark sizes in logical pixels at `zoom`. They follow `typst/graph.typ`
/// (12pt text, 1.8pt dots, 10pt circles, 0.7pt strokes, labels centered 8.8pt
/// out) scaled by zoom, so the preview matches the PDF. Floors keep a
/// zoomed-out view legible.
pub(super) fn sizes(zoom: f64) -> Sizes {
    let pt = zoom / crate::core::UNIT_PT;
    // Quarter-pixel sizes so a zoom gesture reuses cached label sprites.
    let font = ((12. * pt).max(9.) * 4.).round() / 4.;
    let dot = (1.8 * pt).max(2.);
    Sizes {
        font,
        dot,
        ring: (10. * pt).max(5.),
        stroke: (0.7 * pt).max(1.),
        reach: (crate::core::LABEL_REACH * zoom).max(dot + font * 0.6),
    }
}

/// Label direction (an index into `COMPASS`) per vertex id.
type Directions = Rc<HashMap<u64, usize>>;

thread_local! {
    /// The last graph's directions, keyed by a hash of positions and edges.
    static DIRECTIONS: RefCell<Option<(u64, Directions)>> = const { RefCell::new(None) };
}

/// Each vertex's label direction. They depend only on positions and edges,
/// and choosing them is quadratic in the vertex count, so the last answer is
/// kept: selection, hover, labels and styles never change it.
pub(super) fn label_directions(graph: &Graph) -> Directions {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for v in &graph.vertices {
        (v.id, v.x.to_bits(), v.y.to_bits()).hash(&mut hasher);
    }
    for e in &graph.edges {
        (e.a, e.b).hash(&mut hasher);
    }
    let key = hasher.finish();
    DIRECTIONS.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some((k, directions)) = cache.as_ref() {
            if *k == key {
                return directions.clone();
            }
        }
        let directions =
            Rc::new(graph.label_directions(crate::core::LABEL_REACH, crate::core::LABEL_CAP));
        *cache = Some((key, directions.clone()));
        directions
    })
}

/// The screen box around a selection of two or more vertices, padded so
/// its marks fit inside. Dragging inside it moves the selection.
pub(super) fn selection_box(
    graph: &Graph,
    selection: &Selection,
    camera: crate::core::Camera,
    w: f64,
    h: f64,
) -> Option<((f64, f64), (f64, f64))> {
    if selection.vertices.len() < 2 {
        return None;
    }
    let pad = (camera.zoom * 0.5).max(12.);
    let mut corners: Option<((f64, f64), (f64, f64))> = None;
    for v in graph.vertices.iter().filter(|v| selection.has_vertex(v.id)) {
        let p = camera.world_to_screen(v.x, v.y, w, h);
        let (lo, hi) = corners.unwrap_or((p, p));
        corners = Some((
            (lo.0.min(p.0), lo.1.min(p.1)),
            (hi.0.max(p.0), hi.1.max(p.1)),
        ));
    }
    corners.map(|(lo, hi)| ((lo.0 - pad, lo.1 - pad), (hi.0 + pad, hi.1 + pad)))
}

pub(super) struct Sizes {
    pub font: f64,
    pub dot: f64,
    pub ring: f64,
    pub stroke: f64,
    pub reach: f64,
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
            let shown = self.displayed_camera();
            let p = shown.world_to_screen(x, y, w, h);
            let cell = self.picker.font_size();
            let scale = (cell.0 as f64 / self.cell.0, cell.1 as f64 / self.cell.1);
            let side = shown.zoom + 2.;
            graphics::HoverRect {
                left: ((p.0 - side / 2.) * scale.0).floor() as i32,
                top: ((p.1 - side / 2.) * scale.1).floor() as i32,
                width: (side * scale.0).ceil() as u32,
                height: (side * scale.1).ceil() as u32,
            }
        })
    }
}

impl App {
    /// The vertex whose drawn label covers `p`, so clicking a word selects
    /// its vertex instead of placing a new one under it.
    pub(super) fn hit_label(&self, p: (f64, f64)) -> Option<u64> {
        let (w, h) = self.size();
        let s = sizes(self.camera.zoom);
        let scale = self.device_scale();
        let directions = label_directions(&self.graph);
        self.graph
            .vertices
            .iter()
            .rev()
            .filter(|v| !v.label.is_empty())
            .find(|v| {
                let (x, y) = self.camera.world_to_screen(v.x, v.y, w, h);
                // Shape only labels that could reach `p` (a generous bound),
                // so hovering after a zoom does not lay out every label.
                let bound = s.reach + s.font * (v.label.chars().count() as f64 + 2.);
                if (p.0 - x).abs() > bound || (p.1 - y).abs() > bound {
                    return false;
                }
                let (lx, ly) = match (v.style, directions.get(&v.id)) {
                    (VertexStyle::Dots, Some(&d)) => {
                        let (dx, dy) = crate::core::COMPASS[d].1;
                        let len = dx.hypot(dy);
                        (x + dx / len * s.reach, y - dy / len * s.reach)
                    }
                    _ => (x, y),
                };
                label_sprite(&v.label, s.font, scale, options())
                    .bounds
                    .is_some_and(|b| {
                        (lx + b.0 - 2.0..=lx + b.2 + 2.).contains(&p.0)
                            && (ly + b.1 - 2.0..=ly + b.3 + 2.).contains(&p.1)
                    })
            })
            .map(|v| v.id)
    }
}

impl Scene {
    pub(super) fn render(&self) -> Result<Pixmap> {
        self.render_region((0, 0), self.state.size)
    }
    /// The `size` device pixels at `origin` of the frame [`Self::render`]
    /// would draw.
    fn render_region(&self, origin: (u32, u32), size: (u32, u32)) -> Result<Pixmap> {
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
        let mut pix = Pixmap::new(size.0, size.1).context("allocating canvas")?;
        // Tiles of the dragged layer float over a frame that has the grid.
        if self.state.layer == Layer::All || matches!(self.state.layer, Layer::Still(_)) {
            debug_assert_eq!(origin, (0, 0));
            self.draw_grid(&mut pix, w, h);
        }
        // Geometry stays in logical units; render vectors and fonts directly at
        // the terminal's native pixel dimensions. Never enlarge a small bitmap.
        let (ox, oy) = (origin.0 as i32, origin.1 as i32);
        resvg::render(
            &tree,
            Transform::from_row(
                scale.0 as f32,
                0.,
                0.,
                scale.1 as f32,
                -(ox as f32),
                -(oy as f32),
            ),
            &mut pix.as_mut(),
        );
        // Labels sit above edges and nodes, as they did inside the SVG.
        for (sprite, (x, y)) in labels {
            if let Some(label) = &sprite.pix {
                pix.draw_pixmap(
                    (x * scale.0).round() as i32 + sprite.offset.0 - ox,
                    (y * scale.1).round() as i32 + sprite.offset.1 - oy,
                    label.as_ref(),
                    &resvg::tiny_skia::PixmapPaint::default(),
                    Transform::identity(),
                    None,
                );
            }
        }
        Ok(pix)
    }
    /// The dragged layer, rendered only over the `tile`-aligned rectangle its
    /// vertices, labels and edges can reach, with that rectangle's origin.
    /// `None` when nothing of it is on screen.
    pub(super) fn render_tiles(&self, tile: u32) -> Result<Option<(Pixmap, (u32, u32))>> {
        let ids: Vec<u64> = match &self.state.layer {
            Layer::Moving(ids) => ids.clone(),
            Layer::Decor => self.state.selection.vertices.clone(),
            Layer::Pen => {
                let mut ids: Vec<u64> = self.state.edge_start.into_iter().collect();
                ids.extend(self.state.unroll.iter().flat_map(|u| [u.0, u.1]));
                ids.extend(self.state.hide_edge.iter().flat_map(|e| [e.a, e.b]));
                ids
            }
            _ => anyhow::bail!("tiles draw only the dragged layer, decorations or the pen"),
        };
        let (w, h) = self.logical_size;
        let (width, height) = self.state.size;
        let scale = (f64::from(width) / w, f64::from(height) / h);
        let sizes = sizes(self.state.camera.zoom);
        let fonts = options();
        let (mut lo, mut hi) = (
            (f64::INFINITY, f64::INFINITY),
            (f64::NEG_INFINITY, f64::NEG_INFINITY),
        );
        let mut include = |p: (f64, f64), pad: f64| {
            lo = (lo.0.min(p.0 - pad), lo.1.min(p.1 - pad));
            hi = (hi.0.max(p.0 + pad), hi.1.max(p.1 + pad));
        };
        // Room for the circle and the pen ring around it.
        let node = sizes.ring.max(sizes.dot) + 8.;
        // Overlays that draw no labels (only a plain vertex's box or ring).
        let decor = matches!(self.state.layer, Layer::Decor | Layer::Pen);
        let vertices: HashMap<_, _> = self
            .state
            .graph
            .vertices
            .iter()
            .map(|v| (v.id, v))
            .collect();
        let screen = |v: &crate::core::Vertex| self.state.camera.world_to_screen(v.x, v.y, w, h);
        for v in &self.state.graph.vertices {
            if ids.contains(&v.id) {
                // The label sits up to `reach` away and spans its own bounds.
                // Decor draws no labels, only a plain vertex's box: shaping
                // every selected label here put text layout on the input
                // thread for a select-all.
                let label = if decor && v.style != VertexStyle::Plain {
                    0.
                } else {
                    label_sprite(&v.label, sizes.font, scale, fonts)
                        .bounds
                        .map_or(0., |b| {
                            b.0.abs().max(b.1.abs()).max(b.2.abs()).max(b.3.abs())
                        })
                };
                let reach = if decor { 0. } else { sizes.reach };
                include(screen(v), node + reach + label + 4.);
            }
        }
        for edge in &self.state.graph.edges {
            let shown = if decor {
                self.state.selection.has_edge(edge)
                    || self.state.hover_edge.is_some_and(|e| same_edge(&e, edge))
            } else {
                self.state.layer.shows_edge(edge.a, edge.b)
            };
            if shown {
                for id in [edge.a, edge.b] {
                    if let Some(v) = vertices.get(&id) {
                        include(screen(v), sizes.stroke + 4.);
                    }
                }
            }
        }
        if lo.0 > hi.0 {
            return Ok(None);
        }
        let snap = |v: f64, s: f64, max: u32, up: bool| {
            let px = if up { (v * s).ceil() } else { (v * s).floor() }.clamp(0., f64::from(max));
            let t = f64::from(tile);
            let snapped = if up {
                (px / t).ceil() * t
            } else {
                (px / t).floor() * t
            };
            (snapped as u32).min(max)
        };
        let (x0, y0) = (
            snap(lo.0, scale.0, width, false),
            snap(lo.1, scale.1, height, false),
        );
        let (x1, y1) = (
            snap(hi.0, scale.0, width, true),
            snap(hi.1, scale.1, height, true),
        );
        if x1 <= x0 || y1 <= y0 {
            return Ok(None);
        }
        Ok(Some((
            self.render_region((x0, y0), (x1 - x0, y1 - y0))?,
            (x0, y0),
        )))
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
    /// The pen layer: the ring around the pen's vertex, turning; after a
    /// click draws an edge, that ring unrolling into it and winding up again
    /// at the far end; and an edge the shown frame still leaves out.
    fn pen_marks(
        &self,
        scene: &mut String,
        rings: &HashMap<u64, ((f64, f64), f64)>,
        holes: &[Hole],
        stroke: f64,
    ) {
        let state = &self.state;
        let foreground = theme::color(theme::FOREGROUND);
        let accent = theme::color(theme::ACCENT);
        let line = |scene: &mut String, p: (f64, f64), q: (f64, f64), color: &str, opacity: f64| {
            if opacity <= 0.01 {
                return;
            }
            let at = |t: f64| (p.0 + t * (q.0 - p.0), p.1 + t * (q.1 - p.1));
            for (t0, t1) in visible_pieces(p, q, holes) {
                let (a, b) = (at(t0), at(t1));
                write!(scene, r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{color}" stroke-opacity="{opacity}" stroke-width="{stroke}" stroke-linecap="round"/>"#, a.0, a.1, b.0, b.1).unwrap();
            }
        };
        // An edge the frame on screen lacks, drawn exactly as the next frame will.
        let unrolling = state.unroll.map(|(a, b, _)| (a.min(b), a.max(b)));
        if let Some(e) = state
            .hide_edge
            .filter(|e| unrolling != Some((e.a.min(e.b), e.a.max(e.b))))
        {
            if let (Some((p, _)), Some((q, _))) = (rings.get(&e.a), rings.get(&e.b)) {
                line(scene, *p, *q, &foreground, 1.);
            }
        }
        let mut busy = Vec::new();
        if let Some((from, to, k)) = state.unroll {
            if let (Some(&(pa, ra)), Some(&(pb, rb))) = (rings.get(&from), rings.get(&to)) {
                busy = vec![from, to];
                let t = f64::from(k) / 1000.;
                // The ring spins up as it pays out, accelerating; the thread
                // reaches the far vertex at speed and winds on, slowing to a
                // stop; the color settles last.
                let out = ease_in_quart(t / 0.62);
                let up = ease_out_cubic((t - 0.58) / 0.42);
                let tint = ease_in_out_sine((t - 0.5) / 0.5);
                let (dx, dy) = (pb.0 - pa.0, pb.1 - pa.1);
                let len = dx.hypot(dy).max(1e-6);
                let toward = dy.atan2(dx);
                // Its tail runs clockwise into the point facing the new
                // neighbor, as if the dashes were thread leaving the spool.
                let left = (1. - out) * TAU;
                // Its dashes travel with the tail, so the ring visibly turns.
                pen_arc(
                    scene,
                    pa,
                    ra,
                    toward - left,
                    left,
                    state.spin,
                    out * TAU,
                    &accent,
                );
                let reach = ra + out * (len - ra);
                let head = (pa.0 + dx / len * reach, pa.1 + dy / len * reach);
                // The thread settles into the edge's own color as it lands.
                line(scene, pa, head, &foreground, tint);
                line(scene, pa, head, &accent, 1. - tint);
                if state.edge_start == Some(to) {
                    // Its dashes ride the winding end in and coast to rest (a
                    // whole turn later they match the still ring exactly).
                    pen_arc(
                        scene,
                        pb,
                        rb,
                        toward + PI,
                        up * TAU,
                        state.spin,
                        (up - 1.) * TAU,
                        &accent,
                    );
                }
            }
        }
        if let Some(id) = state.edge_start.filter(|id| !busy.contains(id)) {
            if let Some(&(p, r)) = rings.get(&id) {
                pen_arc(scene, p, r, 0., TAU, state.spin, 0., &accent);
            }
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
        let state = &self.state;
        let mut labels = Vec::new();
        let mut nodes = String::new();
        let mut holes = Vec::new();
        let directions = label_directions(&state.graph);
        let Sizes {
            font,
            dot,
            ring,
            stroke,
            reach,
        } = sizes(state.camera.zoom);
        let layer = &state.layer;
        let decor = *layer == Layer::Decor;
        let pen = *layer == Layer::Pen;
        // Decorations are drawn over the frame's marks; a hair wider so the
        // frame's antialiased edge never shows around them.
        let cover = if decor { 0.5 } else { 0. };
        // Center and ring radius of each vertex the pen layer draws at.
        let mut rings = HashMap::new();
        for v in &state.graph.vertices {
            let (x, y) = state.camera.world_to_screen(v.x, v.y, w, h);
            let selected = state.selection.has_vertex(v.id);
            // Fletcher centers the label node on the offset point.
            let (lx, ly) = if v.style == VertexStyle::Dots {
                // World y points up; screen y points down.
                let (dx, dy) = crate::core::COMPASS[directions[&v.id]].1;
                let len = dx.hypot(dy);
                (x + dx / len * reach, y - dy / len * reach)
            } else {
                (x, y)
            };
            // Decorations draw no labels; only plain ones matter there (their
            // box clips edges and marks the selection). Shaping every label
            // would put text layout on the input thread.
            let sprite = (!(decor || pen) || v.style == VertexStyle::Plain)
                .then(|| label_sprite(&v.label, font, scale, fonts));
            // Every vertex clips edges, even one the moving layer leaves out:
            // the circle of one that stays put cuts the dragged edges reaching
            // it. The still frame skips the moving ones' holes: a label being
            // retyped or dragged would leave a stale gap in an edge crossing it.
            let clips = !matches!(layer, Layer::Still(_)) || layer.shows_vertex(v.id);
            if clips && v.style == VertexStyle::Circles {
                holes.push(Hole::Circle((x, y), ring));
            }
            // Plain labels take the vertex's place, so edges stop at their
            // shaped bounds.
            let plain_box = match (v.style, sprite.as_ref().and_then(|s| s.bounds)) {
                (VertexStyle::Plain, Some(b)) => Some((
                    (lx + b.0 - 3., ly + b.1 - 3.),
                    (lx + b.2 + 3., ly + b.3 + 3.),
                )),
                _ => None,
            };
            if let (true, Some((min, max))) = (clips, plain_box) {
                holes.push(Hole::Rect(min, max));
            }
            // The pen ring hugs the mark: just outside a circle, close around
            // a dot, and around a plain label's box.
            let ring_r = match (v.style, plain_box) {
                (VertexStyle::Circles, _) => ring + 3.5,
                (VertexStyle::Plain, Some((min, max))) => {
                    (max.0 - min.0).hypot(max.1 - min.1) / 2. + 1.
                }
                _ => dot + 4.,
            };
            if pen {
                rings.insert(v.id, ((x, y), ring_r));
            }
            let shown = if decor {
                selected
            } else {
                layer.shows_vertex(v.id)
            };
            if !shown {
                continue;
            }
            let color = if selected { &accent } else { &foreground };
            if !decor || selected {
                let (dot, stroke) = (dot + cover, stroke + 2. * cover);
                match v.style {
                    VertexStyle::Circles => {
                        write!(nodes, r#"<circle cx="{x}" cy="{y}" r="{ring}" fill="none" stroke="{color}" stroke-width="{stroke}"/>"#).unwrap();
                    }
                    // A plain vertex with a label is drawn as its label; a
                    // selected one gets a box so the selection is visible.
                    VertexStyle::Plain if plain_box.is_some() => {
                        if let (true, Some((min, max))) = (selected, plain_box) {
                            write!(nodes, r#"<rect x="{}" y="{}" width="{}" height="{}" rx="3" fill="none" stroke="{accent}" stroke-width="1.5"/>"#, min.0, min.1, max.0 - min.0, max.1 - min.1).unwrap();
                        }
                    }
                    // Dots, and a plain vertex with no label, as in `typst/graph.typ`.
                    _ => {
                        write!(
                            nodes,
                            r#"<circle cx="{x}" cy="{y}" r="{dot}" fill="{color}"/>"#
                        )
                        .unwrap();
                    }
                }
            }
            // Kitty draws the pen on its own layer; other protocols bake a
            // still ring into the frame.
            if *layer == Layer::All && state.edge_start == Some(v.id) {
                pen_arc(&mut nodes, (x, y), ring_r, 0., TAU, 0, 0., &accent);
            }
            if let (false, Some(sprite)) = (decor, sprite) {
                labels.push((sprite, (lx, ly)));
            }
        }
        let mut scene = String::new();
        if let Some((x, y)) = state.hover {
            let p = state.camera.world_to_screen(x, y, w, h);
            let size = state.camera.zoom;
            write!(scene, r#"<rect x="{}" y="{}" width="{size}" height="{size}" fill="{accent}" fill-opacity="0.12" stroke="{accent}" stroke-opacity="0.7" stroke-width="1"/>"#, p.0 - size / 2., p.1 - size / 2.).unwrap();
        }
        write!(
            scene,
            r#"<g stroke="{foreground}" stroke-width="{stroke}" stroke-linecap="round">"#
        )
        .unwrap();
        // Resolve endpoints once instead of scanning all vertices per edge.
        let vertices: HashMap<_, _> = state.graph.vertices.iter().map(|v| (v.id, v)).collect();
        for edge in &state.graph.edges {
            let selected = state.selection.has_edge(edge);
            let hovered = state.hover_edge.is_some_and(|e| same_edge(&e, edge));
            let shown = if decor {
                selected || hovered
            } else {
                layer.shows_edge(edge.a, edge.b)
            };
            if !shown {
                continue;
            }
            if let (Some(a), Some(b)) = (vertices.get(&edge.a), vertices.get(&edge.b)) {
                let p = state.camera.world_to_screen(a.x, a.y, w, h);
                let q = state.camera.world_to_screen(b.x, b.y, w, h);
                let highlight = if selected {
                    format!(r#" stroke="{accent}" stroke-width="{}""#, stroke + 2.)
                } else if hovered {
                    // Under the pointer: a click selects it.
                    format!(
                        r#" stroke="{accent}" stroke-opacity="0.6" stroke-width="{}""#,
                        stroke + 1.
                    )
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
        if pen {
            self.pen_marks(&mut scene, &rings, &holes, stroke);
        }
        // The draggable area of a multi-selection (not while it moves).
        if !matches!(layer, Layer::Still(_) | Layer::Moving(_)) {
            if let Some((lo, hi)) =
                selection_box(&state.graph, &state.selection, state.camera, w, h)
            {
                write!(scene, r#"<rect x="{}" y="{}" width="{}" height="{}" rx="4" fill="none" stroke="{accent}" stroke-opacity="0.45" stroke-width="1" stroke-dasharray="3 3"/>"#, lo.0, lo.1, hi.0 - lo.0, hi.1 - lo.1).unwrap();
            }
        }
        if let Some((a, b)) = state.marquee {
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
    #[test]
    fn dragged_layer_tiles_hold_every_pixel_it_draws_and_nothing_else() {
        let mut app = super::super::test_support::app();
        app.graph.vertices[0].style = VertexStyle::Circles;
        let near = app.graph.add_vertex(3., 1.);
        let far = app.graph.add_vertex(-4., -2.);
        app.graph.vertex_mut(far).unwrap().label = "far".into();
        app.graph.connect(1, near);
        app.graph.connect(near, far);
        let mut scene = app.scene();
        let (width, height) = scene.state.size;
        scene.state.layer = Layer::Moving(vec![near]);
        let full = scene.render().unwrap();
        let (tiles, (x0, y0)) = scene.render_tiles(16).unwrap().unwrap();
        assert!(tiles.width() < width, "tiles cover only the dragged part");
        assert_eq!((x0 % 16, y0 % 16, tiles.width() % 16), (0, 0, 0));
        let mut drawn = 0;
        for y in 0..height {
            for x in 0..width {
                let p = full.pixel(x, y).unwrap();
                let inside =
                    (x0..x0 + tiles.width()).contains(&x) && (y0..y0 + tiles.height()).contains(&y);
                if p.alpha() > 0 {
                    drawn += 1;
                    assert!(
                        inside,
                        "pixel ({x}, {y}) of the dragged layer is outside its tiles"
                    );
                }
                if inside {
                    assert_eq!(tiles.pixel(x - x0, y - y0).unwrap(), p);
                }
            }
        }
        assert!(drawn > 0);
        // The still layer leaves out the dragged vertex and both its edges,
        // keeping only the far vertex (and the grid).
        scene.state.layer = Layer::Still(vec![near]);
        let still = scene
            .svg_scene(
                scene.logical_size.0,
                scene.logical_size.1,
                (1., 1.),
                options(),
            )
            .unwrap();
        assert_eq!(still.0.matches("<line").count(), 0);
        assert_eq!(still.0.matches("<circle").count(), 2);
        assert_eq!(still.1.len(), 2);
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
