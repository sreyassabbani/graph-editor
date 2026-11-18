//! Cached PNG layers placed in explicit terminal-cell rectangles. Raster
//! resolution and placement size are independent; transparent pixels retain the
//! terminal background. Text/UI remains owned by Ratatui.
use anyhow::Result;
use base64::{engine::general_purpose::STANDARD, Engine};
use ratatui::layout::Rect;
use resvg::tiny_skia::Pixmap;
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
        ),
        is_tmux(),
    )
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
    /// Canvas size and strip thicknesses of the uploaded marquee sprites.
    marquee_sprites: Option<((u32, u32), (u32, u32))>,
    marquee_placement: Option<(MarqueeRect, Rect, (u16, u16))>,
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
        }
    }
    /// Upload a new canvas frame; the next [`Self::stage_view`] places it.
    pub(super) fn stage_frame(&mut self, frame: KittyFrame) {
        self.pending.push_str(&frame.upload);
        self.frame = Some((frame.camera, frame.margin));
        self.view = None;
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
                self.pending.push_str(&upload_raw(sprites.horizontal, ids[1]));
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
            self.pending.push_str(&place(id, p, at, (0, 0), crop, -1, area, cell));
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
        self.hover_size = None;
        self.hover_placement = None;
        self.marquee_sprites = None;
        self.marquee_placement = None;
        self.frame = None;
        self.view = None;
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
        for (s, d) in s.chunks_exact(4).zip(d.chunks_exact_mut(4)) {
            let a = u16::from(s[3]);
            if partial(s) {
                for k in 0..3 {
                    d[k] = ((u16::from(s[k]) * 255 + a / 2) / a).min(255) as u8;
                }
                d[3] = s[3];
            } else {
                d.copy_from_slice(s);
            }
        }
    };
    let mut blocks = src.chunks_exact(BLOCK).zip(dst.chunks_exact_mut(BLOCK));
    for (s, d) in &mut blocks {
        // (a + 1) & 0xfe is zero exactly for a = 0 and a = 255.
        let any = s
            .chunks_exact(4)
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
        let camera = Camera { x: 0., y: 0., zoom: 10. };
        let mut canvas = KittyCanvas::new();
        canvas.stage_frame(KittyFrame { upload: "UPLOAD".into(), camera, margin: (20, 20) });
        canvas.stage_view(camera, scale, area, cell);
        let mut output = Vec::new();
        canvas.flush(&mut output).unwrap();
        let first = String::from_utf8(output).unwrap();
        assert!(first.starts_with("UPLOAD"));
        assert!(first.contains("a=p,i=") && first.contains("x=20,y=20,w=64,h=128,C=1,z=-2"));
        // Camera half a unit right: content shifts 10 device px left.
        let panned = Camera { x: 0.5, y: -0.25, ..camera };
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
        assert_eq!(crop_origin(camera, Camera { x: -100., ..camera }, (20, 20), scale), (0, 20));
        assert_eq!(crop_origin(camera, Camera { zoom: 12., ..camera }, (20, 20), scale), (20, 20));
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
            let map = libc::mmap(std::ptr::null_mut(), 24, libc::PROT_READ, libc::MAP_SHARED, fd, 0);
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
        for (p, mine) in pix.pixels().iter().zip(ours.chunks_exact(4)) {
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
        let rect = MarqueeRect { left: 12, top: 40, width: 100, height: 50 };
        canvas.stage_marquee(Some(rect), area, cell, sprites).unwrap();
        let mut output = Vec::new();
        canvas.flush(&mut output).unwrap();
        let first = String::from_utf8(output).unwrap();
        assert_eq!(first.matches("a=t,f=32").count(), 3);
        assert!(first.contains("s=640,v=384"));
        for rect in [
            MarqueeRect { width: 300, ..rect },
            MarqueeRect { left: 0, top: 0, width: 1, height: 1 },
        ] {
            canvas
                .stage_marquee(Some(rect), area, cell, |_, _| panic!("drag must not upload"))
                .unwrap();
            let mut output = Vec::new();
            canvas.flush(&mut output).unwrap();
            let sequence = String::from_utf8(output).unwrap();
            assert_eq!(sequence.matches("a=p,").count(), 5);
            assert!(sequence.len() < 800);
        }
        let mut output = Vec::new();
        canvas.stage_marquee(Some(MarqueeRect { left: 0, top: 0, width: 1, height: 1 }), area, cell, |_, _| panic!()).unwrap();
        canvas.flush(&mut output).unwrap();
        assert!(output.is_empty());
        canvas.stage_marquee(None, area, cell, |_, _| panic!()).unwrap();
        canvas.flush(&mut output).unwrap();
        assert_eq!(String::from_utf8(output).unwrap().matches("a=d,d=i").count(), 3);
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
