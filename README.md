# Terminal graph editor

A mouse-first graph-theory editor for the terminal. Draw an undirected graph on
a grid, then copy it as compact [Typst](https://typst.app) code that renders
with [Fletcher](https://typst.app/universe/package/fletcher) through the bundled
`typst/graph.typ` helper.

```sh
cargo run --release
```

Requirements:

- A terminal with Kitty graphics (Ghostty, Kitty, WezTerm); iTerm2 and Sixel
  terminals work through a slower fallback.
- macOS for Copy, which uses `pbcopy`.
- Optional: `typst` on `PATH` for the export compilation test.

The canvas fills the terminal pane below one toolbar row. The background is
transparent and the colours are Catppuccin Frappé.
There is no standing instruction row: the bottom row shows operation results
(cleared after 3 s) and, only while the editor waits on you (a pending
connection, a label prompt), what to do next.

- Hover marks the grid point where a click would place a vertex; it is hidden
  over vertices, edges, occupied points and during drags.
- Click two vertices to connect them. Escape cancels a pending connection.
- Click an edge to select and highlight it. Vertices take priority near endpoints.
- Drag a box on empty canvas to select every vertex inside it and every edge with
  both ends inside. Ctrl/Alt/Cmd-click (or Shift, where the terminal passes it
  through) toggles one vertex or edge; a modifier while boxing adds to the
  selection. `a` selects everything; Escape clears the selection.
- Drag a vertex to move it; dragging a selected vertex moves the whole selection.
  The preview snaps to grid points as you drag, so it shows exactly where the
  vertices land; overlapping an unselected vertex keeps the last valid spot.
  Arrow keys move the selection one grid unit (or pan with nothing selected).
- Right/middle-drag or scrolling pans; Shift-scroll pans sideways.
- Double-click cycles dot, circle and plain styles; new vertices take the last
  chosen style.
- Toolbar: Copy, Undo, Redo and Fit are always in place and dim when they would
  do nothing. Relabel appears only when labels are not already 1…n. With a
  selection: Label, Style (all selected vertices), Connect (only while some
  selected pair is unjoined) and Delete. The selection size and zoom − / + sit
  at the right. Hovered buttons highlight.
- Label edits one vertex's label, or gives every selected vertex the same one;
  the canvas previews it as you type. Enter or a click elsewhere saves; Escape
  cancels. Entering an empty label unlabels them, and new vertices stay
  unlabeled until a vertex is labeled again. An unlabeled plain vertex is drawn
  as a dot, in the editor and the PDF.
- Relabel (or `R`) numbers every vertex 1…n in creation order; new vertices
  continue from n + 1. Vertex IDs in the copied code are unchanged.
- The new-vertex style and label mode persist across launches in
  `~/.local/state/graph-editor/defaults`.
  Drawings themselves are never stored.
- Copy puts a Fletcher fragment on the macOS clipboard.

Keyboard alternatives: Ctrl-C copies;
Cmd-Z, Ctrl-Z or `u` undoes; Cmd-Shift-Z, Ctrl-Shift-Z, Ctrl-Y, Ctrl-R or `U`
redoes. Enter edits the selected vertex label; Backspace/Delete removes the
selection; `s` restyles it; `c` connects it; `a` or Ctrl-A selects all;
F fits (never past 2× zoom); +/- zooms. Ctrl/Alt-wheel zooms at the pointer.
Escape only cancels (drag, connection, selection); `q` quits, asking again if
the drawing has not been copied.

Where the terminal supports pixel mouse reporting (SGR-Pixels, mode 1016:
Ghostty, Kitty, WezTerm; not through tmux), panning and dragging follow the
pointer pixel by pixel; elsewhere they move a text cell at a time. Scrolling
pans 12 px per tick and Ctrl/Alt-scroll zooms 8% per tick.

Panning does not re-render: each Kitty frame is rendered 64 px larger on every
side and pans move the crop window (a ~150 byte command). A new frame is
rendered only once a pan passes half that margin. Full frames go through
POSIX shared memory when the terminal reports support (Ghostty, Kitty; not
through tmux), skipping compression and base64 entirely; otherwise they are
zlib-compressed inline.

Enhanced terminal keyboard reporting is enabled while the editor runs so Cmd
modifiers can be received, then restored on exit. Labels keep
ordinary `u`/`U` and Backspace as text editing keys.

Every launch starts a fresh canvas. There is no JSON storage or file reopening.
Copy emits a compact `#graph(...)` IR. Copy `typst/graph.typ` into your Typst
project and import `graph` from it (`#import "graph.typ": graph`), or re-export
it from your own template. That one helper imports the pinned Fletcher package
internally. The editor previews exactly what the helper draws (sizes, label
placement, styles), so keep any copy you edit in sync with `src/core.rs` and
`src/ui/canvas.rs`.

```typst
#graph(
  vertices: (
    v1: (3, 4, [1]),
    v2: (3, 2, [2]),
    v3: (3, 1, [3]),
  ),
  edges: (
    ("v1", "v2"),
    ("v2", "v3"),
  ),
)
```

Vertex rows are `(x, y, label)`, optionally `(x, y, label, "circles")` or
`(x, y, label, "plain")` for mixed styles. Uniform non-dot graphs use a single
`style: "circles"`/`"plain"` option. IDs are stable references, independent of
visible labels. Labels accept Typst content, including `$v_1$`; editor labels
remain literal and complex strings use `text("...")` safely.

The helper owns contextual color, sizes, strokes and automatic label placement.
Options such as `unit: 10mm`, `labels: false`, and
`label-directions: (v1: "n")` remain editable. No imports or definitions are repeated in each drawing. Existing direct Fletcher
fragments continue working with their original imports. Simple undirected graphs
are supported, without loops or duplicate edges.

## Maintenance

- `core.rs`: graph invariants, camera math, bounded undo history.
- `export.rs`: graph IR generation.
- `cli.rs`, `main.rs`: scratch launch and help.
- `ui.rs`: editor state and terminal lifecycle.
- `ui/events.rs`: motion coalescing and frame scheduling.
- `ui/input.rs`: gestures, edits and shortcuts.
- `ui/view.rs`: toolbar, canvas sizing and image cache.
- `ui/renderer.rs`: bounded background render worker and latest-scene mailbox.
- `ui/canvas.rs`, `ui/theme.rs`: SVG scene description and colors; resvg/usvg handle rendering and fonts.
- `ui/graphics.rs`: Kitty raw-RGBA transport and independent hover placement.
- `ui/output.rs`, `ui/clipboard.rs`: copying to the clipboard.
- `ui/prefs.rs`: remembered new-vertex defaults.

The preview uses [resvg/usvg](https://github.com/linebender/resvg) to render SVG
geometry and shaped system-font labels directly at the terminal's reported pixel
resolution. No bitmap-font rasterizer or reduced-resolution canvas is used.
Fonts prefer GeistMono Nerd Font Mono, then Menlo/monospace. Startup loads only
those two files; the full system font database (for Unicode fallback) loads on a
background thread. Each label is shaped once per text, size and pixel scale,
then cached and blitted; preview sizes (dots, circles, strokes, text, label
distance) follow `typst/graph.typ` scaled by zoom. Grid dots use a brighter Frappé color at 90% opacity. Frames
are sent as zlib-compressed straight-alpha RGBA, keep transparency, and are
explicitly placed across the canvas rectangle. Ratatui handles the UI;
other graphics protocols retain the ratatui-image transport fallback.

SVG parsing, font shaping, rasterization, compression and Kitty encoding run
on one background worker. It accepts one replaceable pending scene, keeping work
bounded during rapid dragging or zooming. Completed frames can still display
during continuous gestures. Edge endpoints use a vertex index rather than a
vertex scan per edge.

On Kitty terminals, hover is a separate native-resolution image behind the graph.
Moving it sends only a placement command; ordinary hover does not clone the
graph, shape fonts, rasterize the canvas or compress/upload another full frame.
The highlight is clipped at viewport edges. Other protocols render hover into
their background-worker frames through the existing fallback.

Redraw starts are spaced by at least 8 ms, coalesce adjacent motion events, and use
synchronized terminal updates. Unchanged canvases are cached. This interval is
not a measured frame-rate guarantee. Native pinch/font changes remain
terminal-dependent. The Kitty transport follows the
[official protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/).

Run `cargo test`. Automated coverage includes pane sizing and resizing,
transparent backgrounds and node interiors, image placement, cropping and
chunking, shared-memory frames, hover and marquee movement without pixel
uploads, bounded worker scheduling, gesture boundaries, graph edits/history,
toolbar state, native-resolution rendering, Unicode font shaping, and Fletcher
fragment compilation in normal and blue text (skipped without `typst`).
Interactive appearance and feel are tested by hand.
