# Internals

## Rendering

The canvas is an SVG scene rendered with [resvg/usvg](https://github.com/linebender/resvg)
at the terminal's reported pixel resolution, then sent as a Kitty image. Sizes
for dots, circles, strokes, text and label distance follow `typst/graph.typ`
scaled by zoom, so the editor matches the PDF. If you change the helper, keep
`src/core.rs` and `src/ui/canvas.rs` in sync.

Labels are drawn in GeistMono Nerd Font Mono, falling back to Menlo and then
monospace. Startup loads only those font files; the full system font database
(for Unicode fallback) loads on a background thread. Each label is shaped once
per text, size and pixel scale, then cached.

One background worker does SVG parsing, font shaping, rasterising,
compression and Kitty encoding. It holds at most one pending scene and
replaces it when a newer one arrives, so rapid dragging or zooming never
queues up work. Redraws start at least 8 ms apart, adjacent mouse-motion
events are coalesced, and frames use synchronised terminal updates.

**Kitty transport.** Frames are straight-alpha RGBA, so the background stays
transparent. They go through POSIX shared memory when the terminal supports it
(Ghostty, Kitty; not through tmux); otherwise they're sent zlib-compressed
inline. Each frame is rendered 64 px larger than the canvas on every side, so
panning only moves the crop window (a ~150-byte command). A new frame is
rendered once a pan passes half that margin.

**Hover.** On Kitty terminals, the hover marker is a separate image behind the
graph, so moving it sends only a placement command. Other protocols draw
hover into each frame through the ratatui-image fallback.

**Dragging and typing labels.** On Kitty terminals, once vertices start
moving (or their label is being typed), the frame is redrawn once without
them and their edges. That part is drawn into 64 px tiles over it, and each
pointer move or keystroke resends only the tiles whose pixels changed, so a
step costs tens of kilobytes instead of a full frame. Releasing (or saving
the label) renders one full frame, which replaces the tiles. A tile can't
erase the frame's pixels under it, so an unrelated edge crossing a moving
circle or plain label is left out of that frame too and drawn, clipped, with
the tiles; the frame is redrawn only when the set of crossed edges changes.
The tiles draw only what the frame on screen leaves out, so a group that
starts moving while an older partial frame is up waits a frame rather than
showing twice.

**Decorations.** On Kitty the frame never shows selection, the pen ring or
the hovered edge. Selection and the hovered edge are a second tile set (`Layer::Decor`) drawn over it
on the input thread, so clicks that only select or move the pen send no
frame at all: one mark is under 1 KB and a millisecond. The tiles cover the
selection's extent, so select-all on a large graph costs more (tens of
milliseconds at a few hundred vertices). While a new zoom renders, or a pan
runs past the frame's margin, the old frame stays up; decor, the hover marker
and click hit-testing all use the camera of that frame (`shown_camera`), so
they line up with what is visible.

**The pen.** Its dashed ring is a third tile set (`Layer::Pen`), placed
behind the frame so labels stay on top. While the pen is down it turns
steadily (a few small tiles per step, about 40 a second), easing up to speed
when the pen goes down and keeping it as the pen moves on; after 60 s without
input it eases to rest and sends nothing more.
When a click draws an edge, the frame leaves that edge out for 480 ms while
the pen layer unrolls the ring into it, spinning up as it pays out, and winds
a new ring onto the far end, coasting to rest. Then the next frame includes
the edge; until it arrives, the pen layer keeps drawing the edge
(`KittyFrame::hides`). Other protocols show a still ring and no animation.

**Input.** With SGR-Pixels mouse reporting (mode 1016: Ghostty, Kitty,
WezTerm, Rex; not through tmux), panning and dragging follow the pointer pixel
by pixel; elsewhere they move a cell at a time. Scrolling pans 12 px per tick,
and Ctrl/Alt-scroll zooms 8% per tick. Enhanced keyboard reporting is on while
the editor runs, so Cmd shortcuts reach it; it's restored on exit.

## Modules

| File | Role |
| --- | --- |
| `core.rs` | Graph invariants, camera maths, bounded undo history |
| `export.rs` | The `#graph(...)` code that Copy produces |
| `cli.rs`, `main.rs` | Launch and `--help` |
| `ui.rs` | Editor state and terminal lifecycle |
| `ui/events.rs` | Coalescing motion events and scheduling frames |
| `ui/input.rs` | Gestures, edits and shortcuts |
| `ui/view.rs` | Toolbar, canvas sizing, image cache |
| `ui/renderer.rs` | Background render worker and its one-scene mailbox |
| `ui/canvas.rs`, `ui/theme.rs` | SVG scene and Catppuccin Frappé colours |
| `ui/graphics.rs` | Kitty RGBA transport and hover placement |
| `ui/output.rs`, `ui/clipboard.rs` | Copying to the clipboard |
| `ui/prefs.rs` | Remembered style and label mode for new vertices (`~/.local/state/petersen/defaults`) |

## Tests

`cargo test` covers pane sizing, transparency, image placement, cropping and
chunking, shared-memory frames, hover and box-selection movement without
re-uploading pixels, worker scheduling, gestures, edits and undo history,
toolbar state, font shaping, and compiling the exported code with Typst in
normal and coloured text (skipped without `typst`). How the editor looks and
feels in use is still checked by hand.
