# Terminal graph editor

```nu
just graph
```

Outside the direnv shell, run `direnv exec . just graph`.

The canvas fills the terminal pane below one toolbar row. The background is
transparent, with Catppuccin Frappé blue and foreground colors matching
`~/config/home/ghostty.conf` and `~/config/home/rex/catppuccin-frappe.conf`.
There are no standing instruction or status subtitles.

- Hover highlights a grid cell; click empty space to add a vertex.
- Click two vertices to connect them. Escape cancels a pending connection.
- Drag a vertex to move it; release snaps to the grid. An occupied destination
  restores the original position. Right/middle-drag or scrolling pans.
- Double-click cycles dot, circle and plain styles. Selecting a vertex adds
  Label, Style, Delete and Style all to the same toolbar row.
- Copy puts a Fletcher fragment on the macOS clipboard. Save .typ writes the same
  fragment to a file. Enter saves, Escape cancels,
  Ctrl-U clears the path; existing files require a second Enter to overwrite.

Keyboard alternatives: Ctrl-C copies; Ctrl-S or Ctrl-E saves .typ; Ctrl-Z undoes;
Ctrl-Y or Ctrl-Shift-Z redoes; Enter edits the selected label; Delete removes it;
F fits; +/- zooms. Ctrl/Alt-wheel zooms at the pointer when supported. Q quits;
uncopied/unsaved work requires confirmation.

Every launch starts a fresh canvas. There is no JSON storage or file reopening.
Copy and Save assume Fletcher's `diagram`, `node`, and `edge` are already in
scope. Import them once in your document:

```typst
#import "@preview/fletcher:0.5.8": diagram, node, edge
```

The generated fragment starts with `#context diagram(...)`: shared styling appears
once, ordinary labels use `[1]`, and label offsets use short compass directions.
There are no imports, helper functions, `drawing` variables, or repository-local
dependencies in the output. Saved files contain the same code for pasting into
your document. All ink inherits surrounding text color. Complex labels use
literal `text("...")` strings. Simple undirected graphs are supported, without
loops or duplicate edges.

## Maintenance

- `core.rs`: graph invariants, camera math, bounded undo history.
- `export.rs`, `fileio.rs`: Fletcher generation and atomic file replacement.
- `cli.rs`, `main.rs`: scratch launch and help.
- `ui.rs`: editor state and terminal lifecycle.
- `ui/events.rs`: motion coalescing and frame scheduling.
- `ui/input.rs`: gestures, edits and shortcuts.
- `ui/view.rs`: toolbar, canvas sizing and image cache.
- `ui/renderer.rs`: bounded background render worker and latest-scene mailbox.
- `ui/canvas.rs`, `ui/theme.rs`: SVG scene description and colors; resvg/usvg handle rendering and fonts.
- `ui/graphics.rs`: Kitty PNG transport and independent hover placement.
- `ui/output.rs`, `ui/clipboard.rs`: save dialog and clipboard.

The preview uses [resvg/usvg](https://github.com/linebender/resvg) to render SVG
geometry and shaped system-font labels directly at the terminal's reported pixel
resolution. No bitmap-font rasterizer or reduced-resolution canvas is used.
Fonts prefer GeistMono Nerd Font Mono, then Menlo/monospace with usvg fallback.
Grid dots use a brighter Frappé color at 90% opacity. The PNG retains transparency
and is explicitly placed across the canvas rectangle. Ratatui handles the UI;
other graphics protocols retain the ratatui-image transport fallback.

SVG parsing, font shaping, rasterization, PNG compression and Kitty encoding run
on one background worker. It accepts one replaceable pending scene, keeping work
bounded during rapid dragging or zooming. Completed frames can still display
during continuous gestures. Edge endpoints use a vertex index rather than a
vertex scan per edge.

On Kitty terminals, hover is a separate native-resolution image behind the graph.
Moving it sends only a placement command; ordinary hover does not clone the
graph, shape fonts, rasterize the canvas or compress/upload another full PNG.
The highlight is clipped at viewport edges. Other protocols render hover into
their background-worker frames through the existing fallback.

Redraw starts are spaced by at least 8 ms, coalesce adjacent motion events, and use
synchronized terminal updates. Unchanged canvases are cached. This interval is
not a measured frame-rate guarantee. Native pinch/font changes remain
terminal-dependent. The Kitty transport follows the
[official protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/).

Run `direnv exec . just graph-check`. Automated coverage includes pane sizing
and resizing, transparent PNG backgrounds and node interiors, image placement
and chunking, hover movement without pixel uploads, bounded worker scheduling,
gesture boundaries, graph edits/history, overwrite confirmation,
native-resolution rendering, Unicode font shaping, and Fletcher fragment compilation in normal and blue text. Interactive
appearance and feel are left for the user to test; do not drive Rex or the
clipboard for manual verification.
