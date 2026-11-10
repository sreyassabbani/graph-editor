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

pub(super) fn is_tmux() -> bool {
    std::env::var("TERM").is_ok_and(|s| s.starts_with("tmux"))
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
            "\x1b_Ga=d,d=I,i={},q=2\x1b\\\x1b_Ga=d,d=I,i={},q=2\x1b\\",
            image_id(),
            hover_id()
        ),
        is_tmux(),
    )
}

fn hover_id() -> u32 {
    image_id() + 0x0100_0000
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
}
impl KittyCanvas {
    pub(super) fn new() -> Self {
        Self {
            pending: String::new(),
            hover_size: None,
            hover_placement: None,
        }
    }
    pub(super) fn stage_encoded(&mut self, encoded: String) {
        self.pending.push_str(&encoded);
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
                "\x1b_Ga=p,i={},p=1,X={},Y={},x={crop_x},y={crop_y},w={w},h={h},C=1,z=-2,q=2\x1b\\",
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

pub(super) fn encode(png: &[u8], area: Rect, tmux: bool) -> String {
    let mut output = format!("\x1b7\x1b[{};{}H", area.y + 1, area.x + 1);
    output.push_str(&upload(
        png,
        &format!(
            "a=T,f=100,t=d,i={},p=1,c={},r={},C=1,z=-1,q=2",
            image_id(),
            area.width,
            area.height
        ),
        tmux,
    ));
    output.push_str("\x1b8");
    output
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
    fn small_png_is_explicitly_scaled_to_full_canvas_and_only_sent_once() {
        let pix = Pixmap::new(80, 160).unwrap();
        let mut canvas = KittyCanvas::new();
        canvas.stage_encoded(encode(
            &pix.encode_png().unwrap(),
            Rect::new(0, 1, 100, 60),
            false,
        ));
        let mut output = Vec::new();
        canvas.flush(&mut output).unwrap();
        let sequence = String::from_utf8(output.clone()).unwrap();
        assert!(sequence.contains("c=100,r=60,C=1,z=-1"));
        assert!(sequence.contains("f=100"));
        assert!(sequence.contains("\x1b[2;1H"));
        canvas.flush(&mut output).unwrap();
        assert_eq!(output.len(), sequence.len());
    }
    #[test]
    fn png_chunks_obey_protocol_limits_and_finish_the_upload() {
        let data: Vec<_> = (0..10000).map(|n| (n % 251) as u8).collect();
        let sequence = encode(&data, Rect::new(0, 1, 80, 24), false);
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
