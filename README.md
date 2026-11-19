# petersen

Draw a graph with the mouse in your terminal, then paste it into
[Typst](https://typst.app) as a [Fletcher](https://typst.app/universe/package/fletcher)
figure that looks the same as on screen.

<img src="docs/demo.gif" width="900" alt="Drawing a five-vertex graph with the pen in the terminal, giving it circled vertices, copying it, pasting it into a Typst file, and the PDF updating">

```sh
nix run github:sreyassabbani/petersen
```

Needs a terminal with Kitty graphics (Ghostty, Kitty, WezTerm, Rex). iTerm2 and
Sixel terminals use a slower fallback. Copy uses `pbcopy`, so it works on macOS only.

## Controls

| Do | How |
| --- | --- |
| Draw | Click empty canvas to add a vertex. The ringed vertex is the pen: each click on empty canvas adds a vertex joined to it, and each click on a vertex joins that one; the pen moves along |
| Branch | Lift the pen, click the vertex to branch from, then keep clicking (a click on a vertex with the pen down joins it) |
| Lift the pen | Esc, right-click, or click the ringed vertex |
| Select | Click a vertex, its label or an edge; drag a box; Ctrl-click toggles; `a` selects all |
| Move | Drag a vertex; a selection moves together when you drag any of its vertices, a selected edge, or inside its dashed box; arrow keys nudge |
| Deselect | Click inside the selection's dashed box, or Esc |
| Label | Double-click a vertex, Enter, or the Label button; an empty label removes it |
| Change style | `s` or the Style button: dot → circle → plain |
| Renumber 1…n | `R` |
| Delete | Backspace |
| Pan / zoom | Scroll or right-drag / Ctrl-scroll, `+` `-`, `F` fits |
| Undo / redo | `u` / `U` (or ⌘Z / ⌘⇧Z) |
| Copy as Typst | Copy button or Ctrl-C |
| Quit | `q` (asks again if you haven't copied) |

Ctrl-click is the toggle that works everywhere: terminals don't report Cmd,
and Ghostty keeps Shift-click for its own text selection unless
`mouse-shift-capture` is set.

Each launch starts with an empty canvas, and nothing is saved.

## In Typst

Copy [`typst/graph.typ`](typst/graph.typ) into your project, import it once,
and paste:

```typst
#import "graph.typ": graph

#graph(
  vertices: (
    v1: (-3, 3, [1]),
    v2: (2, 3, [2]),
    v3: (-3, 0, [3]),
  ),
  edges: (
    ("v1", "v2"),
    ("v1", "v3"),
  ),
)
```

Each vertex is `(x, y, label)`. Add `"circles"` or `"plain"` as a fourth
field to style one vertex, or pass `style: "circles"` for all of them.
Labels accept Typst content such as `[$v_1$]`. You can also pass `unit: 10mm`,
`labels: false` or `label-directions: (v1: "n")`.

## Development

`nix develop`, then `cargo test`. If `typst` is on `PATH`, the tests also
compile the exported code. [docs/internals.md](docs/internals.md) explains how
rendering works and lists what each module does.
