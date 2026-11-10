# Graph editor design

A mouse-first terminal graph-theory editor whose output is readable Fletcher
Typst code. Start a fresh in-memory canvas with `just graph`; copy the code or
save a `.typ` file. No project format, JSON persistence, or file reopening.

## Interaction

One toolbar row; use the rest of the terminal pane as the canvas. No persistent
instruction subtitles. Hover highlights a grid cell; clicking empty space adds a
vertex. Click two vertices to connect. Drag vertices to move continuously, snap
on release, and reject occupied destinations. Right/middle-drag pans. Selected
vertices expose label/style/delete actions in the same toolbar row. Double-click
cycles dot/circle/plain styles. Keyboard shortcuts supplement mouse actions.

Stable vertex IDs are separate from labels. Reject loops and duplicate edges;
deleting a vertex removes incident edges. A completed drag is one undo step;
Escape cancels it. Camera changes never change graph coordinates. Use the same
camera transform for drawing and hit testing. Labels remain literal text.

## Rendering

Transparent background; Frappé colors match the user's Ghostty/Rex configuration.
Ratatui owns UI text and the event loop. Describe graph geometry as SVG;
resvg/usvg own rasterization, font discovery, shaping, measurement, masking and
compositing. Render at the terminal's native pixel dimensions. Never enlarge a
reduced-resolution preview or implement a font renderer. Kitty terminals receive
compressed PNGs placed in explicit cell rectangles. Preserve alpha, including
node interiors that mask incident edges. Keep the small transport adapter isolated
in `ui/graphics.rs`; other protocols retain the ratatui-image fallback.

Keep expensive rendering and PNG/Kitty encoding on a single background worker.
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

Export a direct `#context diagram(...)` fragment, assuming `diagram`, `node` and
`edge` are in scope. No imports, helper functions or named drawing wrapper.
Put shared styles at the diagram level, keep one call per line, and use short
compass offsets for labels. Copy and Save produce identical fragments. Preserve
isolated vertices, positions, styles, Unicode and literal labels. Every component
inherits surrounding Typst text color. Save atomically; confirm existing-file overwrite.

Follow the module boundaries in README.md. Run `just graph-check` in the direnv
shell, including full-pane sizing/resize, transparency, protocol framing, graph
invariants, gesture history/cancellation, save dialogs and compilation in normal
and blue text. The user tests interactive behavior; do not manually drive Rex
or access the clipboard for verification. Preserve unrelated homework changes.
