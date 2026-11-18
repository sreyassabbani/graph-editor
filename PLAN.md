# Graph editor design

A mouse-first terminal graph-theory editor whose output is readable Fletcher
Typst code. Start a fresh in-memory canvas with `cargo run --release`; copy the
code to the clipboard. No file output, project format, JSON persistence, or file reopening.

## Interaction

One toolbar row; use the rest of the terminal pane as the canvas. No persistent
instruction subtitles. Hover highlights a grid cell; clicking empty space adds a
vertex. Click two vertices to connect. Drag vertices to move continuously, snap
on release, and reject occupied destinations. Right/middle-drag pans. Selected
vertices expose label/style/delete actions in the same toolbar row. Double-click
cycles dot/circle/plain styles. Keyboard shortcuts supplement mouse actions.
Click near an edge segment to select it; highlight the selected edge and remove
it with Backspace/Delete. Use endpoint IDs for edge selection, never a mutable
vector index. Prefer vertex hits near endpoints and select the nearest edge
within a terminal-cell-sized tolerance. Escape deselects an edge.

Undo uses Cmd-Z/Ctrl-Z/`u`; redo uses Cmd-Shift-Z/Ctrl-Shift-Z/Ctrl-Y/Ctrl-R/`U`.
Keyboard and toolbar history share the same path, clearing unfinished connection
gestures and invalid selections. Text fields keep editing keys local. Enable
enhanced keyboard reporting for Cmd modifiers and restore it on exit.

Stable vertex IDs are separate from labels. Reject loops and duplicate edges;
deleting a vertex removes incident edges. A completed drag is one undo step;
Escape cancels it. Camera changes never change graph coordinates. Use the same
camera transform for drawing and hit testing. Labels remain literal text.

## Rendering

Transparent background; Catppuccin Frappé colors.
Ratatui owns UI text and the event loop. Describe graph geometry as SVG;
resvg/usvg own rasterization, font discovery, shaping, measurement and
compositing. Edges are clipped geometrically around circle nodes and plain
labels (no SVG masks); grid dots are blended from small antialiased sprites. Render at the terminal's native pixel dimensions. Never enlarge a
reduced-resolution preview or implement a font renderer. Kitty terminals receive
zlib-compressed raw RGBA (`f=32,o=z`) placed in explicit cell rectangles.
Preserve alpha, including node interiors that edges stop short of. Keep the small transport adapter isolated
in `ui/graphics.rs`; other protocols retain the ratatui-image fallback.

Keep expensive rendering and Kitty encoding on a single background worker.
Allow one active scene, one replaceable pending scene, and one latest completed
frame. Present completed frames during continuous gestures so a stream of input
cannot starve drawing. Never replay a queue of obsolete render requests. Reject
completed frames whose viewport no longer matches. Index edge endpoints once
per scene, giving expected O(V + E) construction rather than O(V * E) scans.

Separate Kitty hover from graph rendering: describe its small SVG with resvg,
upload it once per size, then move the same placement with pixel offsets and
viewport cropping. Ordinary hover must not traverse/clone the graph or encode
a full canvas. Other protocols retain the worker-based full-frame fallback.

Coalesce only adjacent motion events, preserving presses/releases/scrolls/keys.
Measure the 8 ms redraw interval from frame start, without an extra delay after
rendering. Cache unchanged images and synchronize terminal updates.
Do no work while idle. Restore terminal modes and remove the canvas image on exit.
Do not invoke Typst while drawing. Do not claim a measured frame rate without
measurements; native pinch is terminal-dependent.

## Export and validation

Export a compact `#graph(vertices: (...), edges: (...))` IR. Each vertex row is
`id: (x, y, label[, style])`. Keep IDs independent of visible labels; annotate
only mixed-style exceptions, and put uniform styles at graph level. Assume
`graph` is imported from the bundled `typst/graph.typ` (directly or re-exported
by a template).
The portable helper owns Fletcher imports, contextual ink, label placement and
styling. Preserve the older graph constructor/paths/positions APIs too.
Preserve isolated vertices, positions, styles, Unicode and literal labels.
Compile exports through the bundled helper, including mixed/uniform styles in
normal and blue text.

Follow the module boundaries in README.md. Run `cargo test`, covering
full-pane sizing/resize, transparency, protocol framing, graph invariants,
gesture history/cancellation and compilation in normal and blue text.
Interactive behavior is tested by hand; automated tests do not touch the
clipboard.
