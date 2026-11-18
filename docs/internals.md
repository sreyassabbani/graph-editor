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
