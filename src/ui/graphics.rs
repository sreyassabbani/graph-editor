//! Cached PNG layers placed in explicit terminal-cell rectangles. Raster
//! resolution and placement size are independent; transparent pixels retain the
//! terminal background. Text/UI remains owned by Ratatui.
use anyhow::Result;
use base64::{engine::general_purpose::STANDARD, Engine};
use ratatui::layout::Rect;
use resvg::tiny_skia::Pixmap;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::{self, Write};

fn image_id() -> u32 {
    0x4700_0000 | (std::process::id() & 0x00ff_ffff)
}

/// tmux sets $TMUX in every pane; TERM is often screen-256color or even
/// xterm-256color there, so checking TERM alone misses real sessions.
pub(super) fn is_tmux() -> bool {
    std::env::var_os("TMUX").is_some()
        || std::env::var("TERM").is_ok_and(|s| s.starts_with("tmux") || s.starts_with("screen"))
}
fn passthrough(sequence: &str, tmux: bool) -> String {
    if tmux {
        format!("\x1bPtmux;{}\x1b\\", sequence.replace('\x1b', "\x1b\x1b"))
    } else {
        sequence.into()
    }
}

pub(super) fn cleanup() -> String {
    passthrough(
        &format!(
            "\x1b_Ga=d,d=I,i={},q=2\x1b\\\x1b_Ga=d,d=I,i={},q=2\x1b\\{}",
            image_id(),
            hover_id(),
            marquee_ids()
                .iter()
                .map(|id| format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\"))
                .collect::<String>()
                + &[0, 1, 2]
                    .map(|set| {
                        let (first, last) = tile_ids(set);
                        format!("\x1b_Ga=d,d=R,x={first},y={last},q=2\x1b\\")
                    })
                    .concat()
        ),
        is_tmux(),
    )
}

/// Side of a dragged-layer tile in device pixels. Small tiles hug edges
/// tightly, so a moved diagonal edge resends a thin strip of pixels.
pub(super) const TILE: u32 = 64;

/// The first and last tile id of this process for tile set `set`: dragged
/// layer (0), decorations (1) or the pen (2). One image per tile position.
/// Each holds 13 bits of the process id, so two editors in one terminal
/// share a range only if their ids agree in those bits, and 15 bits of tile
/// position (see [`TileSet::id`]).
fn tile_ids(set: u32) -> (u32, u32) {
    let base = (0x5000_0000 + set * 0x1000_0000) | ((std::process::id() & 0x1fff) << 15);
    (base, base + 0x7fff)
}

/// Tiles of one layer now shown, by (column, row), with a hash of their
/// pixels, and the area and cell size they were placed for.
#[derive(Default)]
struct TileSet {
    set: u32,
    /// Stacking order: decorations over the frame, the pen behind it.
    z: i32,
    shown: HashMap<(u32, u32), u64>,
    placed_for: Option<(Rect, (u16, u16))>,
}

impl TileSet {
    fn id(&self, (col, row): (u32, u32)) -> u32 {
        tile_ids(self.set).0 + (row << 8) + col
    }
    /// Show `layer` (pixels and their canvas origin, both aligned to
    /// [`TILE`]). Only tiles whose pixels changed are sent; empty ones are
    /// removed. `None` removes every tile.
    fn stage(
        &mut self,
        pending: &mut String,
        layer: Option<(Pixmap, (u32, u32))>,
        area: Rect,
        cell: (u16, u16),
    ) {
        if self.placed_for != Some((area, cell)) {
            self.clear(pending);
            self.placed_for = Some((area, cell));
        }
        let mut shown = HashMap::new();
        if let Some((pix, origin)) = layer {
            let (width, height) = (pix.width(), pix.height());
            let data = pix.data();
            for top in (0..height).step_by(TILE as usize) {
                for left in (0..width).step_by(TILE as usize) {
                    let (w, h) = (TILE.min(width - left), TILE.min(height - top));
                    let mut tile = Vec::with_capacity((w * h * 4) as usize);
                    for y in top..top + h {
                        let start = ((y * width + left) * 4) as usize;
                        tile.extend_from_slice(&data[start..start + (w * 4) as usize]);
                    }
                    if tile.iter().skip(3).step_by(4).all(|alpha| *alpha == 0) {
                        continue;
                    }
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    tile.hash(&mut hasher);
                    let hash = hasher.finish();
                    let (x, y) = (origin.0 + left, origin.1 + top);
                    let key = (x / TILE, y / TILE);
                    // Ids hold 8 bits of column and 7 of row (a canvas of
                    // 16384 x 8192 px); never spill into another range.
                    if key.0 >= 1 << 8 || key.1 >= 1 << 7 {
                        continue;
                    }
                    shown.insert(key, hash);
                    if self.shown.get(&key) == Some(&hash) {
                        continue;
                    }
                    let id = self.id(key);
                    let size = resvg::tiny_skia::IntSize::from_wh(w, h).expect("non-empty tile");
                    let pix = Pixmap::from_vec(tile, size).expect("tile pixels");
                    pending.push_str(&upload_raw(pix, id));
                    pending.push_str(&place(id, 1, (x, y), (0, 0), (w, h), self.z, area, cell));
                }
            }
        }
        for key in self.shown.keys().filter(|k| !shown.contains_key(k)) {
            pending.push_str(&delete(self.id(*key)));
        }
        self.shown = shown;
    }
    fn clear(&mut self, pending: &mut String) {
        for key in self.shown.keys() {
            pending.push_str(&delete(self.id(*key)));
        }
        self.shown.clear();
    }
    fn forget(&mut self) {
        self.shown.clear();
        self.placed_for = None;
    }
}

fn hover_id() -> u32 {
    image_id() + 0x0100_0000
}

/// Marquee fill, horizontal dashes and vertical dashes.
fn marquee_ids() -> [u32; 3] {
    [1, 2, 3].map(|n| image_id() + n * 0x0200_0000)
}

/// Rubber band in canvas device pixels, already clipped to the canvas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct MarqueeRect {
    pub left: u32,
    pub top: u32,
    pub width: u32,
    pub height: u32,
}

/// Pre-rendered pieces the marquee is cropped from: a canvas-sized fill, a
/// canvas-wide dash strip and a canvas-tall dash strip.
pub(super) struct MarqueeSprites {
    pub fill: Pixmap,
    pub horizontal: Pixmap,
    pub vertical: Pixmap,
}

/// Native-pixel bounds, relative to the canvas; may cross its edges.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct HoverRect {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(PartialEq)]
struct HoverPlacement {
    bounds: HoverRect,
    area: Rect,
    cell: (u16, u16),
}

pub(super) struct KittyCanvas {
    pending: String,
    hover_size: Option<(u32, u32)>,
    hover_placement: Option<HoverPlacement>,
    /// Camera and margin of the uploaded canvas frame.
    frame: Option<(crate::core::Camera, (u32, u32))>,
    /// Crop origin and area of the canvas placement now shown.
    view: Option<((u32, u32), Rect)>,
    /// The pane area and cell size the uploaded frame was first placed in.
    frame_fits: Option<(Rect, (u16, u16))>,
    /// Canvas size and strip thicknesses of the uploaded marquee sprites.
    marquee_sprites: Option<((u32, u32), (u32, u32))>,
    marquee_placement: Option<(MarqueeRect, Rect, (u16, u16))>,
    /// Vertices the shown frame leaves out (dragged or relabeled), which the
    /// moving tiles then draw.
    frame_omits: Vec<u64>,
    frame_cuts: Vec<crate::core::Edge>,
    /// An edge the shown frame leaves out while it is animated in.
    frame_hides: Option<crate::core::Edge>,
    tiles: TileSet,
    decor: TileSet,
    pen: TileSet,
}

/// Put a cropped placement of `id` at canvas pixel (x, y): the cursor goes to
/// the containing cell and X/Y carry the remainder, as for the hover.
#[allow(clippy::too_many_arguments)]
fn place(
    id: u32,
    p: u32,
    at: (u32, u32),
    source: (u32, u32),
    crop: (u32, u32),
    z: i32,
    area: Rect,
    cell: (u16, u16),
) -> String {
    let cell = (u32::from(cell.0), u32::from(cell.1));
    format!(
        "\x1b7\x1b[{};{}H{}\x1b8",
        u32::from(area.y) + at.1 / cell.1 + 1,
        u32::from(area.x) + at.0 / cell.0 + 1,
        passthrough(
            &format!(
                "\x1b_Ga=p,i={id},p={p},X={},Y={},x={},y={},w={},h={},C=1,z={z},q=2\x1b\\",
                at.0 % cell.0,
                at.1 % cell.1,
                source.0,
                source.1,
                crop.0,
                crop.1
            ),
            is_tmux(),
        )
    )
}

/// Delete image `id` with its placements and pixel data.
fn delete(id: u32) -> String {
    passthrough(&format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\"), is_tmux())
}

fn upload_raw(pix: Pixmap, id: u32) -> String {
    let (width, height) = (pix.width(), pix.height());
    upload(
        // fdeflate's fast mode stores solid colors almost uncompressed (a
        // Retina fill is 32 MB); miniz's level 1 shrinks it to kilobytes.
        &miniz_oxide::deflate::compress_to_vec_zlib(&unpremultiply(pix), 1),
        &format!("a=t,f=32,o=z,s={width},v={height},t=d,i={id},q=2"),
        is_tmux(),
    )
}

/// Where the visible canvas starts inside a frame rendered for `rendered`
/// with `margin`. A different zoom cannot be shown by cropping; keep the
/// frame centred until its replacement arrives.
pub(super) fn crop_origin(
    rendered: crate::core::Camera,
    now: crate::core::Camera,
    margin: (u32, u32),
    scale: (f64, f64),
) -> (u32, u32) {
    if rendered.zoom != now.zoom {
        return margin;
    }
    // Content moves right when the camera moves left; world y points up.
    let dx = (rendered.x - now.x) * now.zoom * scale.0;
    let dy = (now.y - rendered.y) * now.zoom * scale.1;
    let axis = |m: u32, d: f64| (f64::from(m) - d).round().clamp(0., f64::from(2 * m)) as u32;
    (axis(margin.0, dx), axis(margin.1, dy))
}

impl KittyCanvas {
    pub(super) fn new() -> Self {
        Self {
            pending: String::new(),
            hover_size: None,
            hover_placement: None,
            marquee_sprites: None,
            marquee_placement: None,
            frame: None,
            view: None,
            frame_fits: None,
            frame_omits: Vec::new(),
            frame_cuts: Vec::new(),
            frame_hides: None,
            tiles: TileSet {
                z: -1,
                ..TileSet::default()
            },
            decor: TileSet {
                set: 1,
                z: -1,
                ..TileSet::default()
            },
            pen: TileSet {
                set: 2,
                z: -3,
                ..TileSet::default()
            },
        }
    }
    pub(super) fn frame_omits(&self) -> &[u64] {
        &self.frame_omits
    }
    pub(super) fn frame_cuts(&self) -> &[crate::core::Edge] {
        &self.frame_cuts
    }
    pub(super) fn frame_hides(&self) -> Option<crate::core::Edge> {
        self.frame_hides
    }
    /// The camera the canvas on screen shows: the uploaded frame's, shifted
    /// by the crop now placed. It differs from the current camera while a
    /// new zoom is rendering, or a pan has run past the frame's margin.
    /// `scale` is device pixels per logical pixel.
    pub(super) fn shown_camera(&self, scale: (f64, f64)) -> Option<crate::core::Camera> {
        let (rendered, margin) = self.frame?;
        let (crop, _) = self.view?;
        let z = rendered.zoom;
        Some(crate::core::Camera {
            x: rendered.x + (f64::from(crop.0) - f64::from(margin.0)) / (z * scale.0),
            y: rendered.y - (f64::from(crop.1) - f64::from(margin.1)) / (z * scale.1),
            zoom: z,
        })
    }
    /// Show the dragged or relabeled layer (see [`TileSet::stage`]).
    pub(super) fn stage_tiles(
        &mut self,
        layer: Option<(Pixmap, (u32, u32))>,
        area: Rect,
        cell: (u16, u16),
    ) {
        self.tiles.stage(&mut self.pending, layer, area, cell);
    }
    /// Show selection highlights and the pen ring (see [`TileSet::stage`]).
    pub(super) fn stage_decor(
        &mut self,
        layer: Option<(Pixmap, (u32, u32))>,
        area: Rect,
        cell: (u16, u16),
    ) {
        self.decor.stage(&mut self.pending, layer, area, cell);
    }
    /// Show the pen ring and an edge it unrolls into (see [`TileSet::stage`]).
    pub(super) fn stage_pen(
        &mut self,
        layer: Option<(Pixmap, (u32, u32))>,
        area: Rect,
        cell: (u16, u16),
    ) {
        self.pen.stage(&mut self.pending, layer, area, cell);
    }
    /// Upload a new canvas frame; the next [`Self::stage_view`] places it.
    pub(super) fn stage_frame(&mut self, frame: KittyFrame) {
        self.pending.push_str(&frame.upload);
        self.frame_omits = frame.omits;
        self.frame_cuts = frame.cuts;
        self.frame_hides = frame.hides;
        self.frame = Some((frame.camera, frame.margin));
        self.view = None;
        self.frame_fits = None;
    }
    /// Show the canvas for `camera`. A pan within the frame's margin only moves
    /// the crop window: a ~150 byte command instead of a new frame. `scale` is
    /// device pixels per logical pixel.
    pub(super) fn stage_view(
        &mut self,
        camera: crate::core::Camera,
        scale: (f64, f64),
        area: Rect,
        cell: (u16, u16),
    ) {
        let Some((rendered, margin)) = self.frame else {
            return;
        };
        // After a resize the old frame no longer fits the pane: take it down
        // until the frame rendered for the new size arrives, rather than
        // showing it cropped or offset.
        if *self.frame_fits.get_or_insert((area, cell)) != (area, cell) {
            if self.view.take().is_some() {
                self.pending.push_str(&passthrough(
                    &format!("\x1b_Ga=d,d=i,i={},p=1,q=2\x1b\\", image_id()),
                    is_tmux(),
                ));
            }
            return;
        }
        let source = crop_origin(rendered, camera, margin, scale);
        if self.view == Some((source, area)) {
            return;
        }
        let size = (
            u32::from(area.width) * u32::from(cell.0),
            u32::from(area.height) * u32::from(cell.1),
        );
        self.pending
            .push_str(&place(image_id(), 1, (0, 0), source, size, -2, area, cell));
        self.view = Some((source, area));
    }
    /// Show the rubber band by cropping placements of sprites uploaded once per
    /// canvas size, so dragging it never re-renders or re-sends the canvas.
    pub(super) fn stage_marquee(
        &mut self,
        rect: Option<MarqueeRect>,
        area: Rect,
        cell: (u16, u16),
        render: impl FnOnce(u32, u32) -> Result<MarqueeSprites>,
    ) -> Result<()> {
        let rect = rect.filter(|r| r.width > 0 && r.height > 0);
        let placement = rect.map(|r| (r, area, cell));
        if self.marquee_placement == placement {
            return Ok(());
        }
        let ids = marquee_ids();
        let Some(r) = rect else {
            for id in ids {
                self.pending.push_str(&passthrough(
                    &format!("\x1b_Ga=d,d=i,i={id},q=2\x1b\\"),
                    is_tmux(),
                ));
            }
            self.marquee_placement = None;
            return Ok(());
        };
        let size = (
            u32::from(area.width) * u32::from(cell.0),
            u32::from(area.height) * u32::from(cell.1),
        );
        let thickness = match self.marquee_sprites {
            Some((uploaded, thickness)) if uploaded == size => thickness,
            _ => {
                let sprites = render(size.0, size.1)?;
                let thickness = (sprites.horizontal.height(), sprites.vertical.width());
                self.pending.push_str(&upload_raw(sprites.fill, ids[0]));
                self.pending
                    .push_str(&upload_raw(sprites.horizontal, ids[1]));
                self.pending.push_str(&upload_raw(sprites.vertical, ids[2]));
                self.marquee_sprites = Some((size, thickness));
                thickness
            }
        };
        let (th, tv) = (thickness.0.min(r.height), thickness.1.min(r.width));
        for (id, p, at, crop) in [
            (ids[0], 1, (r.left, r.top), (r.width, r.height)),
            (ids[1], 1, (r.left, r.top), (r.width, th)),
            (ids[1], 2, (r.left, r.top + r.height - th), (r.width, th)),
            (ids[2], 1, (r.left, r.top), (tv, r.height)),
            (ids[2], 2, (r.left + r.width - tv, r.top), (tv, r.height)),
        ] {
            self.pending
                .push_str(&place(id, p, at, (0, 0), crop, -1, area, cell));
        }
        self.marquee_placement = placement;
        Ok(())
    }
    /// Upload the small highlight once per zoom size. Motion only replaces its
    /// placement: no full-canvas render, PNG encoding, or pixel upload.
    pub(super) fn stage_hover(
        &mut self,
        bounds: Option<HoverRect>,
        area: Rect,
        cell: (u16, u16),
        render: impl FnOnce(u32, u32) -> Result<Pixmap>,
    ) -> Result<()> {
        let placement = bounds
            .filter(|b| {
                b.left < i32::from(area.width) * i32::from(cell.0)
                    && b.top < i32::from(area.height) * i32::from(cell.1)
                    && b.left + b.width as i32 > 0
                    && b.top + b.height as i32 > 0
            })
            .map(|bounds| HoverPlacement { bounds, area, cell });
        if self.hover_placement == placement {
            return Ok(());
        }
        let Some(p) = &placement else {
            self.pending.push_str(&passthrough(
                &format!("\x1b_Ga=d,d=i,i={},p=1,q=2\x1b\\", hover_id()),
                is_tmux(),
            ));
            self.hover_placement = None;
            return Ok(());
        };
        let b = p.bounds;
        let size = (b.width, b.height);
        if self.hover_size != Some(size) {
            let png = render(b.width, b.height)?.encode_png()?;
            self.pending.push_str(&upload(
                &png,
                &format!("a=t,f=100,t=d,i={},q=2", hover_id()),
                is_tmux(),
            ));
            self.hover_size = Some(size);
        }
        // Crop at all four viewport boundaries so the highlight never covers
        // the toolbar or spills into a neighboring pane. No c/r: native size.
        let x = b.left.max(0) as u32;
        let y = b.top.max(0) as u32;
        let crop_x = (-b.left).max(0) as u32;
        let crop_y = (-b.top).max(0) as u32;
        let w = (b.width - crop_x).min(u32::from(area.width) * u32::from(cell.0) - x);
        let h = (b.height - crop_y).min(u32::from(area.height) * u32::from(cell.1) - y);
        self.pending.push_str(&format!(
            "\x1b7\x1b[{};{}H",
            u32::from(area.y) + y / u32::from(cell.1) + 1,
            u32::from(area.x) + x / u32::from(cell.0) + 1
        ));
        self.pending.push_str(&passthrough(
            &format!(
                "\x1b_Ga=p,i={},p=1,X={},Y={},x={crop_x},y={crop_y},w={w},h={h},C=1,z=-3,q=2\x1b\\",
                hover_id(),
                x % u32::from(cell.0),
                y % u32::from(cell.1)
            ),
            is_tmux(),
        ));
        self.pending.push_str("\x1b8");
        self.hover_placement = placement;
        Ok(())
    }
    pub(super) fn hide(&mut self) {
        self.pending = cleanup();
        self.tiles.forget();
        self.decor.forget();
        self.pen.forget();
        self.frame_omits.clear();
        self.frame_cuts.clear();
        self.frame_hides = None;
        self.hover_size = None;
        self.hover_placement = None;
        self.marquee_sprites = None;
        self.marquee_placement = None;
        self.frame = None;
        self.view = None;
        self.frame_fits = None;
    }
    pub(super) fn flush(&mut self, output: &mut impl Write) -> io::Result<()> {
        if !self.pending.is_empty() {
            output.write_all(self.pending.as_bytes())?;
            output.flush()?;
            self.pending.clear();
        }
        Ok(())
    }
}

/// Straight-alpha RGBA from tiny-skia's premultiplied pixels. Nearly every
/// canvas pixel is fully transparent or opaque and is copied unchanged; only
/// antialiased edge pixels pay for a (rounded) division.
pub(super) fn unpremultiply(pix: Pixmap) -> Vec<u8> {
    let mut rgba = vec![0; pix.data().len()];
    unpremultiply_into(pix.data(), &mut rgba);
    rgba
}

/// One pass from premultiplied `src` to straight-alpha `dst`. Most of the
/// canvas is fully transparent or opaque: 16-pixel blocks with no partial
/// alpha (a vectorizable test) are copied unchanged.
fn unpremultiply_into(src: &[u8], dst: &mut [u8]) {
    const BLOCK: usize = 64;
    let partial = |p: &[u8]| ((u16::from(p[3]) + 1) & 0xfe) != 0;
    let convert = |s: &[u8], d: &mut [u8]| {
        for (s, d) in s.as_chunks::<4>().0.iter().zip(d.as_chunks_mut::<4>().0) {
            let a = u16::from(s[3]);
            if partial(s) {
                for k in 0..3 {
                    d[k] = ((u16::from(s[k]) * 255 + a / 2) / a).min(255) as u8;
                }
                d[3] = s[3];
            } else {
                *d = *s;
            }
        }
    };
    let blocks = src
        .as_chunks::<BLOCK>()
        .0
        .iter()
        .zip(dst.as_chunks_mut::<BLOCK>().0);
    for (s, d) in blocks {
        // (a + 1) & 0xfe is zero exactly for a = 0 and a = 255.
        let any = s
            .as_chunks::<4>()
            .0
            .iter()
            .fold(0u16, |acc, p| acc | ((u16::from(p[3]) + 1) & 0xfe));
        if any == 0 {
            d.copy_from_slice(s);
        } else {
            convert(s, d);
        }
    }
    let done = src.len() / BLOCK * BLOCK;
    convert(&src[done..], &mut dst[done..]);
}

/// How full frames reach the terminal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Transport {
    /// zlib-compressed RGBA inside the escape sequence (f=32, o=z).
    Inline,
    /// Raw RGBA in a POSIX shared-memory object the terminal maps directly:
    /// no compression, base64 or decompression on either side.
    SharedMemory,
}

/// A rendered canvas uploaded without a placement. It is `margin` device
/// pixels larger than the canvas on every side, so panning can be shown by
/// cropping it differently instead of rendering and sending a new frame.
pub(super) struct KittyFrame {
    pub upload: String,
    pub camera: crate::core::Camera,
    pub margin: (u32, u32),
    /// Vertices it leaves out (see `Layer::Still`); empty for a full frame.
    pub omits: Vec<u64>,
    /// Other edges it leaves out, which cross a moving mark.
    pub cuts: Vec<crate::core::Edge>,
    /// An edge it leaves out while the pen animates it in.
    pub hides: Option<crate::core::Edge>,
}

pub(super) fn encode_frame(pix: Pixmap, transport: Transport, tmux: bool) -> Result<String> {
    let (width, height) = (pix.width(), pix.height());
    let header = format!("a=t,f=32,s={width},v={height},i={},q=2", image_id());
    if transport == Transport::SharedMemory {
        let name = shm::write(pix)?;
        return Ok(passthrough(
            &format!("\x1b_G{header},t=s;{}\x1b\\", STANDARD.encode(name)),
            tmux,
        ));
    }
    let compressed = fdeflate::compress_to_vec(&unpremultiply(pix));
    Ok(upload(&compressed, &format!("{header},o=z,t=d"), tmux))
}

pub(super) mod shm {
    //! Shared-memory frames. The terminal unlinks each object after reading
    //! it; the last few names are also unlinked here (a no-op when already
    //! gone) so frames the terminal never read cannot accumulate.
    use super::*;
    use std::collections::VecDeque;
    use std::ffi::CString;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    const KEEP: usize = 8;
    static NAMES: Mutex<VecDeque<CString>> = Mutex::new(VecDeque::new());

    fn create(len: usize) -> Result<(CString, *mut u8)> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        // macOS limits names to 31 bytes.
        let name = CString::new(format!(
            "/ge{:x}-{:x}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))?;
        // SAFETY: plain POSIX calls on a fresh, exclusively created object.
        unsafe {
            let fd = libc::shm_open(
                name.as_ptr(),
                libc::O_CREAT | libc::O_EXCL | libc::O_RDWR,
                0o600 as libc::c_uint,
            );
            anyhow::ensure!(fd >= 0, "shm_open: {}", io::Error::last_os_error());
            let map = if libc::ftruncate(fd, len as libc::off_t) == 0 {
                libc::mmap(
                    std::ptr::null_mut(),
                    len,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    fd,
                    0,
                )
            } else {
                libc::MAP_FAILED
            };
            libc::close(fd);
            if map == libc::MAP_FAILED {
                libc::shm_unlink(name.as_ptr());
                anyhow::bail!("mapping shared memory: {}", io::Error::last_os_error());
            }
            Ok((name, map.cast()))
        }
    }

    fn remember(name: CString) {
        let mut names = NAMES.lock().unwrap();
        names.push_back(name);
        while names.len() > KEEP {
            let old = names.pop_front().unwrap();
            unsafe { libc::shm_unlink(old.as_ptr()) };
        }
    }

    /// Write straight-alpha RGBA into a new object; returns its name.
    pub(in super::super) fn write(pix: Pixmap) -> Result<String> {
        let len = pix.data().len();
        let (name, map) = create(len)?;
        // SAFETY: `map` is a fresh writable mapping of exactly `len` bytes.
        let out = unsafe { std::slice::from_raw_parts_mut(map, len) };
        unpremultiply_into(pix.data(), out);
        unsafe { libc::munmap(map.cast(), len) };
        let text = name.to_str().unwrap().to_owned();
        remember(name);
        Ok(text)
    }

    /// A 1x1 object for the startup capability query.
    pub(in super::super) fn probe_object() -> Option<CString> {
        let (name, map) = create(4).ok()?;
        unsafe { libc::munmap(map.cast(), 4) };
        Some(name)
    }

    pub(in super::super) fn unlink(name: &CString) {
        unsafe { libc::shm_unlink(name.as_ptr()) };
    }

    pub(in super::super) fn unlink_all() {
        for name in NAMES.lock().unwrap().drain(..) {
            unsafe { libc::shm_unlink(name.as_ptr()) };
        }
    }
}

fn upload(png: &[u8], header: &str, tmux: bool) -> String {
    let payload = STANDARD.encode(png);
    let mut output = String::new();
    let count = payload.len().div_ceil(4096);
    for (index, chunk) in payload.as_bytes().chunks(4096).enumerate() {
        let more = u8::from(index + 1 < count);
        let header = if index == 0 {
            format!("{header},m={more}")
        } else {
            format!("m={more},q=2")
        };
        output.push_str(&passthrough(
            &format!(
                "\x1b_G{header};{}\x1b\\",
                std::str::from_utf8(chunk).unwrap()
            ),
            tmux,
        ));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_resized_pane_takes_the_old_frame_down_until_a_new_one_fits() {
        let camera = crate::core::Camera {
            x: 0.,
            y: 0.,
            zoom: 10.,
        };
        let (scale, cell) = ((2., 2.), (16, 32));
        let mut canvas = KittyCanvas::new();
        canvas.stage_frame(KittyFrame {
            upload: String::new(),
            camera,
            margin: (0, 0),
            omits: Vec::new(),
            cuts: Vec::new(),
            hides: None,
        });
        canvas.stage_view(camera, scale, Rect::new(0, 1, 80, 20), cell);
        assert!(canvas.shown_camera(scale).is_some());
        canvas.pending.clear();
        canvas.stage_view(camera, scale, Rect::new(0, 1, 100, 30), cell);
        assert!(
            canvas.pending.contains("a=d,d=i"),
            "the old placement is removed"
        );
        assert!(canvas.shown_camera(scale).is_none());
        // Back to the size it was drawn for, it shows again.
        canvas.stage_view(camera, scale, Rect::new(0, 1, 80, 20), cell);
        assert!(canvas.shown_camera(scale).is_some());
    }
    #[test]
    fn tile_id_ranges_stay_apart_and_hold_every_tile() {
        let ranges: Vec<_> = (0..3).map(tile_ids).collect();
        for (i, a) in ranges.iter().enumerate() {
            for b in &ranges[i + 1..] {
                assert!(a.1 < b.0 || b.1 < a.0);
            }
            assert!(a.0 > hover_id() && a.0 > marquee_ids()[2]);
            let set = TileSet {
                set: i as u32,
                ..TileSet::default()
            };
            assert_eq!(set.id(((1 << 8) - 1, (1 << 7) - 1)), a.1);
        }
    }
    #[test]
    fn frames_upload_without_placing_and_pans_only_move_the_crop() {
        use crate::core::Camera;
        let mut pix = Pixmap::new(80, 160).unwrap();
        pix.data_mut()[..4].copy_from_slice(&[50, 25, 0, 128]);
        let upload = encode_frame(pix, Transport::Inline, false).unwrap();
        assert!(upload.contains("a=t,f=32,s=80,v=160"));
        assert!(!upload.contains("a=T") && !upload.contains("p=1"));
        let mut payload = String::new();
        for part in upload.split("\x1b_G").skip(1) {
            payload.push_str(
                part.split_once(';')
                    .unwrap()
                    .1
                    .split("\x1b\\")
                    .next()
                    .unwrap(),
            );
        }
        let raw = fdeflate::decompress_to_vec(&STANDARD.decode(payload).unwrap()).unwrap();
        assert_eq!(raw.len(), 80 * 160 * 4);
        assert_eq!(&raw[..4], &[100, 50, 0, 128]);

        let (area, cell, scale) = (Rect::new(0, 1, 8, 8), (8, 16), (2., 2.));
        let camera = Camera {
            x: 0.,
            y: 0.,
            zoom: 10.,
        };
        let mut canvas = KittyCanvas::new();
        canvas.stage_frame(KittyFrame {
            upload: "UPLOAD".into(),
            camera,
            margin: (20, 20),
            omits: Vec::new(),
            cuts: Vec::new(),
            hides: None,
        });
        canvas.stage_view(camera, scale, area, cell);
        let mut output = Vec::new();
        canvas.flush(&mut output).unwrap();
        let first = String::from_utf8(output).unwrap();
        assert!(first.starts_with("UPLOAD"));
        assert!(first.contains("a=p,i=") && first.contains("x=20,y=20,w=64,h=128,C=1,z=-2"));
        // Camera half a unit right: content shifts 10 device px left.
        let panned = Camera {
            x: 0.5,
            y: -0.25,
            ..camera
        };
        canvas.stage_view(panned, scale, area, cell);
        let mut output = Vec::new();
        canvas.flush(&mut output).unwrap();
        let moved = String::from_utf8(output).unwrap();
        assert!(moved.contains("x=30,y=25,") && !moved.contains("UPLOAD"));
        assert!(moved.len() < 200);
        // Same view again: nothing to send.
        canvas.stage_view(panned, scale, area, cell);
        let mut output = Vec::new();
        canvas.flush(&mut output).unwrap();
        assert!(output.is_empty());
        // Beyond the margin the crop clamps; a zoom change keeps it centred.
        assert_eq!(
            crop_origin(camera, Camera { x: -100., ..camera }, (20, 20), scale),
            (0, 20)
        );
        assert_eq!(
            crop_origin(
                camera,
                Camera {
                    zoom: 12.,
                    ..camera
                },
                (20, 20),
                scale
            ),
            (20, 20)
        );
    }
    #[test]
    fn shared_memory_frames_hold_straight_alpha_pixels() {
        let mut pix = Pixmap::new(3, 2).unwrap();
        pix.data_mut()[..4].copy_from_slice(&[50, 25, 0, 128]);
        let upload = encode_frame(pix, Transport::SharedMemory, false).unwrap();
        let name = upload.split(';').nth(1).unwrap().trim_end_matches("\x1b\\");
        assert!(upload.contains("a=t,f=32,s=3,v=2") && upload.contains("t=s"));
        let name = std::ffi::CString::new(STANDARD.decode(name).unwrap()).unwrap();
        // Read it back the way a terminal would.
        unsafe {
            let fd = libc::shm_open(name.as_ptr(), libc::O_RDONLY, 0);
            assert!(fd >= 0);
            let map = libc::mmap(
                std::ptr::null_mut(),
                24,
                libc::PROT_READ,
                libc::MAP_SHARED,
                fd,
                0,
            );
            assert_ne!(map, libc::MAP_FAILED);
            let bytes = std::slice::from_raw_parts(map.cast::<u8>(), 24);
            assert_eq!(&bytes[..4], &[100, 50, 0, 128]);
            libc::munmap(map, 24);
            libc::close(fd);
        }
        shm::unlink_all();
        assert!(unsafe { libc::shm_open(name.as_ptr(), libc::O_RDONLY, 0) } < 0);
    }
    #[test]
    fn unpremultiply_matches_tiny_skia_demultiply() {
        let mut pix = Pixmap::new(256, 1).unwrap();
        for (a, p) in pix.pixels_mut().iter_mut().enumerate() {
            let a = a as u8;
            *p = resvg::tiny_skia::PremultipliedColorU8::from_rgba(a / 3, a / 2, a, a).unwrap();
        }
        let ours = unpremultiply(pix.clone());
        for (p, mine) in pix.pixels().iter().zip(ours.as_chunks::<4>().0) {
            // tiny-skia rounds exact .5 ties inconsistently (float math).
            let c = p.demultiply();
            let theirs = [c.red(), c.green(), c.blue(), c.alpha()];
            assert!(mine.iter().zip(theirs).all(|(m, t)| m.abs_diff(t) <= 1));
            assert_eq!(mine[3], theirs[3]);
        }
    }
    #[test]
    fn png_chunks_obey_protocol_limits_and_finish_the_upload() {
        let data: Vec<_> = (0..10000).map(|n| (n % 251) as u8).collect();
        let sequence = upload(&data, "a=T,f=32", false);
        let mut decoded = Vec::new();
        for part in sequence.split("\x1b_G").skip(1) {
            let (header, rest) = part.split_once(';').unwrap();
            let payload = rest.split("\x1b\\").next().unwrap();
            assert!(payload.len() <= 4096);
            decoded.extend(STANDARD.decode(payload).unwrap());
            if decoded.len() == data.len() {
                assert!(header.contains("m=0"));
            } else {
                assert!(header.contains("m=1"));
            }
        }
        assert_eq!(decoded, data);
    }

    #[test]
    fn marquee_drags_by_cropping_sprites_uploaded_once() {
        let mut canvas = KittyCanvas::new();
        let area = Rect::new(0, 1, 80, 24);
        let cell = (8, 16);
        let sprites = |w, h| {
            Ok(MarqueeSprites {
                fill: Pixmap::new(w, h).unwrap(),
                horizontal: Pixmap::new(w, 2).unwrap(),
                vertical: Pixmap::new(2, h).unwrap(),
            })
        };
        let rect = MarqueeRect {
            left: 12,
            top: 40,
            width: 100,
            height: 50,
        };
        canvas
            .stage_marquee(Some(rect), area, cell, sprites)
            .unwrap();
        let mut output = Vec::new();
        canvas.flush(&mut output).unwrap();
        let first = String::from_utf8(output).unwrap();
        assert_eq!(first.matches("a=t,f=32").count(), 3);
        assert!(first.contains("s=640,v=384"));
        for rect in [
            MarqueeRect { width: 300, ..rect },
            MarqueeRect {
                left: 0,
                top: 0,
                width: 1,
                height: 1,
            },
        ] {
            canvas
                .stage_marquee(Some(rect), area, cell, |_, _| {
                    panic!("drag must not upload")
                })
                .unwrap();
            let mut output = Vec::new();
            canvas.flush(&mut output).unwrap();
            let sequence = String::from_utf8(output).unwrap();
            assert_eq!(sequence.matches("a=p,").count(), 5);
            assert!(sequence.len() < 800);
        }
        let mut output = Vec::new();
        canvas
            .stage_marquee(
                Some(MarqueeRect {
                    left: 0,
                    top: 0,
                    width: 1,
                    height: 1,
                }),
                area,
                cell,
                |_, _| panic!(),
            )
            .unwrap();
        canvas.flush(&mut output).unwrap();
        assert!(output.is_empty());
        canvas
            .stage_marquee(None, area, cell, |_, _| panic!())
            .unwrap();
        canvas.flush(&mut output).unwrap();
        assert_eq!(
            String::from_utf8(output)
                .unwrap()
                .matches("a=d,d=i")
                .count(),
            3
        );
    }
    #[test]
    fn dragged_tiles_resend_only_changed_pixels_and_drop_empty_ones() {
        let mut canvas = KittyCanvas::new();
        let area = Rect::new(0, 1, 80, 24);
        let cell = (8, 16);
        let layer = |dots: &[(u32, u32)]| {
            let mut pix = Pixmap::new(2 * TILE, TILE).unwrap();
            for &(x, y) in dots {
                pix.pixels_mut()[(y * 2 * TILE + x) as usize] =
                    resvg::tiny_skia::ColorU8::from_rgba(255, 255, 255, 255).premultiply();
            }
            Some((pix, (TILE, 0)))
        };
        let staged = |canvas: &mut KittyCanvas, layer| {
            canvas.stage_tiles(layer, area, cell);
            let mut output = Vec::new();
            canvas.flush(&mut output).unwrap();
            let output = String::from_utf8(output).unwrap();
            (
                output.matches("a=t,").count(),
                output.matches("a=d,").count(),
            )
        };
        // Only the tile with pixels is sent; the empty one is skipped.
        assert_eq!(staged(&mut canvas, layer(&[(3, 3)])), (1, 0));
        assert_eq!(staged(&mut canvas, layer(&[(3, 3)])), (0, 0));
        // Pixels moving into the second tile send it; the first is unchanged.
        assert_eq!(staged(&mut canvas, layer(&[(3, 3), (TILE + 1, 1)])), (1, 0));
        // Leaving the first tile deletes it.
        assert_eq!(staged(&mut canvas, layer(&[(TILE + 1, 1)])), (0, 1));
        assert_eq!(staged(&mut canvas, None), (0, 1));
    }
    #[test]
    fn hover_moves_without_rasterizing_or_uploading_pixels_and_clips_to_canvas() {
        let mut canvas = KittyCanvas::new();
        let area = Rect::new(0, 1, 80, 24);
        let cell = (8, 16);
        let bounds = HoverRect {
            left: 20,
            top: 30,
            width: 50,
            height: 50,
        };
        canvas
            .stage_hover(Some(bounds), area, cell, |w, h| {
                Ok(Pixmap::new(w, h).unwrap())
            })
            .unwrap();
        let mut output = Vec::new();
        canvas.flush(&mut output).unwrap();
        assert!(String::from_utf8(output).unwrap().contains("a=t,f=100"));
        for (left, top, expected_crop) in [
            (-10, -20, "x=10,y=20,w=40,h=30"),
            (620, 370, "x=0,y=0,w=20,h=14"),
        ] {
            let moved = HoverRect {
                left,
                top,
                ..bounds
            };
            canvas
                .stage_hover(Some(moved), area, cell, |_, _| {
                    panic!("motion must not rasterize")
                })
                .unwrap();
            let mut output = Vec::new();
            canvas.flush(&mut output).unwrap();
            let sequence = String::from_utf8(output).unwrap();
            assert!(sequence.contains("a=p,"));
            assert!(sequence.contains(expected_crop));
            assert!(!sequence.contains("f=100"));
            assert!(sequence.len() < 200);
        }
        canvas
            .stage_hover(None, area, cell, |_, _| panic!("hiding must not rasterize"))
            .unwrap();
        let mut output = Vec::new();
        canvas.flush(&mut output).unwrap();
        assert!(String::from_utf8(output).unwrap().contains("a=d,d=i"));
        // Returning to the canvas reuses the sprite, even after leaving it.
        canvas
            .stage_hover(Some(bounds), area, cell, |_, _| {
                panic!("sprite should still exist")
            })
            .unwrap();
    }
}
