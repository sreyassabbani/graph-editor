// Small undirected graph drawings. Coordinates increase rightward/upward.
#import "@preview/fletcher:0.5.8" as fletcher

#let _names(n) = {
  assert(type(n) == int and n > 0, message: "vertex count must be a positive integer")
  range(n).map(i => str(i + 1))
}

// Constructors return data, not rendered content. All names are strings.
#let complete(n) = {
  let vertices = _names(n)
  let edges = ()
  for i in range(n) {
    for j in range(i + 1, n) { edges.push((vertices.at(i), vertices.at(j))) }
  }
  (vertices: vertices, edges: edges)
}

#let path(n) = {
  let vertices = _names(n)
  (vertices: vertices, edges: range(n - 1).map(i => (vertices.at(i), vertices.at(i + 1))))
}

#let cycle(n) = {
  assert(type(n) == int and n >= 3, message: "a simple cycle needs at least three vertices")
  let result = path(n)
  result.edges.push((result.vertices.last(), result.vertices.first()))
  result
}

#let bipartite(m, n) = {
  let left = _names(m).map(v => "u" + v)
  let right = _names(n).map(v => "v" + v)
  let edges = ()
  let positions = (:)
  for (i, v) in left.enumerate() { positions.insert(v, (-1, (m - 1) / 2 - i)) }
  for (i, v) in right.enumerate() { positions.insert(v, (1, (n - 1) / 2 - i)) }
  for u in left { for v in right { edges.push((u, v)) } }
  (vertices: left + right, edges: edges, positions: positions)
}

#let _segment-distance(p, a, b) = {
  let (dx, dy) = (b.at(0) - a.at(0), b.at(1) - a.at(1))
  let length = dx * dx + dy * dy
  let t = if length == 0 { 0 } else {
    calc.clamp(((p.at(0) - a.at(0)) * dx + (p.at(1) - a.at(1)) * dy) / length, 0, 1)
  }
  calc.sqrt(calc.pow(p.at(0) - a.at(0) - t * dx, 2) + calc.pow(p.at(1) - a.at(1) - t * dy, 2))
}

// Automatic label placement; petersen's src/core.rs mirrors this rule.
// 1. Each label tries eight compass spots at distance `reach`. Score: clearance
//    from every edge and other vertex (saturating at `cap`), plus a small bonus
//    for pointing away from the drawing's center.
// 2. Consistency: a label whose spot is fully clear (clearance >= reach) adopts
//    the n/s direction most used by its row, or e/w most used by its column.
#let _auto-directions(positions, pairs, reach, cap, overrides, directions) = {
  let names = positions.keys()
  let center = (
    positions.values().map(p => p.at(0)).sum() / names.len(),
    positions.values().map(p => p.at(1)).sum() / names.len(),
  )
  let first = (:)
  let clear = (:)
  for name in names {
    let (x, y) = positions.at(name)
    let outward = (x - center.at(0), y - center.at(1))
    let outward-norm = calc.max(calc.sqrt(calc.pow(outward.at(0), 2) + calc.pow(outward.at(1), 2)), 1e-9)
    let best = none
    let best-score = -1e9
    let clearances = (:)
    for (key, d) in directions {
      let n = calc.sqrt(d.at(0) * d.at(0) + d.at(1) * d.at(1))
      let spot = (x + d.at(0) / n * reach, y + d.at(1) / n * reach)
      let c = cap
      for (a, b) in pairs { c = calc.min(c, _segment-distance(spot, positions.at(a), positions.at(b))) }
      for (other, q) in positions {
        if other != name { c = calc.min(c, calc.sqrt(calc.pow(spot.at(0) - q.at(0), 2) + calc.pow(spot.at(1) - q.at(1), 2))) }
      }
      clearances.insert(key, c)
      let score = c / cap + 0.1 * (d.at(0) * outward.at(0) + d.at(1) * outward.at(1)) / n / outward-norm
      if score > best-score + 1e-9 {
        best = key
        best-score = score
      }
    }
    first.insert(name, overrides.at(name, default: best))
    clear.insert(name, clearances)
  }
  let result = (:)
  for name in names {
    let (x, y) = positions.at(name)
    let votes = (:)
    for (other, q) in positions {
      if other == name { continue }
      let d = first.at(other)
      if (calc.abs(q.at(1) - y) < 1e-9 and d in ("n", "s")) or (calc.abs(q.at(0) - x) < 1e-9 and d in ("e", "w")) {
        votes.insert(d, votes.at(d, default: 0) + 1)
      }
    }
    let current = first.at(name)
    let pick = current
    for (key, _) in directions {
      if clear.at(name).at(key) >= reach - 1e-9 and votes.at(key, default: 0) > votes.at(pick, default: 0) { pick = key }
    }
    result.insert(name, pick)
  }
  result
}

// Labels and positions are keyed by vertex name; labels may be Typst content.
// Repeated undirected edges are drawn once. Loops require a separate renderer.
#let graph(
  ..args,
  data: none,
  vertices: (:),
  paths: (),
  edges: (),
  positions: (:),
  layout: "circle",
  labels: auto,
  label-directions: (:),
  style: "dots",
  vertex-styles: (:),
  color: auto,
  unit: 12mm,
  radius: 1.8pt,
  circle-radius: 10pt,
  thickness: 0.7pt,
  label-gap: 7pt,
) = context {
  assert(args.pos().len() <= 1 and args.named().len() == 0, message: "graph accepts one graph data argument")
  assert(args.pos().len() == 0 or data == none, message: "provide graph data only once")
  let data = args.pos().at(0, default: data)
  let edges = edges
  let positions = positions
  // Compact editor IR: id: (x, y, label[, style]). Keep identity separate
  // from visible labels; labels can be literal text or Typst/math content.
  assert(type(vertices) == dictionary, message: "vertices must be a dictionary of id: (x, y, label[, style]) rows")
  let specs = vertices
  let titles = (:)
  let styles = (:)
  for (name, row) in specs {
    assert(type(row) == array and row.len() in (3, 4), message: "each vertex row must be (x, y, label) or (x, y, label, style)")
    assert(name not in positions, message: "provide each vertex position only once")
    positions.insert(name, (row.at(0), row.at(1)))
    titles.insert(name, row.at(2))
    if row.len() == 4 { styles.insert(name, row.at(3)) }
  }
  let labels = if labels == auto { if specs.len() > 0 { titles } else { auto } }
    else if type(labels) == dictionary { titles + labels } else { labels }
  let vertex-styles = styles + vertex-styles
  assert(style in ("dots", "circles", "plain"), message: "style must be dots, circles, or plain")
  assert(layout in ("circle", "square", "line"), message: "layout must be circle, square, or line")
  assert(unit > 0pt and radius > 0pt and circle-radius > 0pt, message: "graph sizes must be positive")
  let ink = if color == auto { text.fill } else { color }
  let vertices = ()
  if data != none {
    vertices = data.vertices
    edges = data.edges + edges
    positions = data.at("positions", default: (:)) + positions
  }
  for route in paths {
    let names = if type(route) == str { route.split().filter(v => v != "") } else { route }
    assert(names.len() >= 2, message: "each path needs at least two vertex names")
    vertices += names
    for i in range(names.len() - 1) { edges.push((names.at(i), names.at(i + 1))) }
  }
  vertices += positions.keys()
  for pair in edges {
    assert(type(pair) == array and pair.len() == 2, message: "edges must be pairs of vertex names")
    vertices += pair
  }
  vertices = vertices.dedup()
  assert(vertices.len() > 0, message: "graph needs at least one vertex")
  assert(vertices.all(v => type(v) == str and v != ""), message: "vertex names must be nonempty strings")
  for mapping in (label-directions, vertex-styles, if type(labels) == dictionary { labels } else { (:) }) {
    assert(mapping.keys().all(v => v in vertices), message: "label options reference an unknown vertex")
  }
  assert(vertex-styles.values().all(s => s in ("dots", "circles", "plain")), message: "vertex styles must be dots, circles, or plain")
  let pairs = ()
  for pair in edges {
    assert(pair.first() != pair.last(), message: "self-loops are not supported")
    let key = pair.sorted()
    if key not in pairs { pairs.push(key) }
  }
  // Partial manual layouts are usually a mistake; never silently invent positions.
  if positions.len() > 0 {
    assert(vertices.all(v => v in positions), message: "provide positions for every vertex, or omit positions")
  } else {
    if layout == "square" {
      assert(vertices.len() == 4, message: "square layout needs exactly four vertices")
    }
    for (i, name) in vertices.enumerate() {
      let p = if layout == "square" {
        ((-1, 1), (1, 1), (1, -1), (-1, -1)).at(i)
      } else if layout == "line" {
        (i - (vertices.len() - 1) / 2, 0)
      } else {
        let angle = 90deg - i * 360deg / vertices.len()
        (1.5 * calc.cos(angle), 1.5 * calc.sin(angle))
      }
      positions.insert(name, p)
    }
  }
  for p in positions.values() {
    assert(type(p) == array and p.len() == 2 and p.all(x => type(x) in (int, float)), message: "positions must be numeric (x, y) pairs")
  }
  assert(positions.values().dedup().len() == positions.len(), message: "vertices cannot occupy the same position")
  let center-x = positions.values().map(p => p.at(0)).sum() / vertices.len()
  let center-y = positions.values().map(p => p.at(1)).sum() / vertices.len()
  let horizontal = positions.values().all(p => calc.abs(p.at(1) - center-y) < 0.001)
  for d in label-directions.values() {
    assert(d in ("n", "ne", "e", "se", "s", "sw", "w", "nw"), message: "label direction must be n, ne, e, se, s, sw, w, or nw")
  }
  let directions = (n: (0, 1), ne: (1, 1), e: (1, 0), se: (1, -1), s: (0, -1), sw: (-1, -1), w: (-1, 0), nw: (-1, 1))
  let automatic = _auto-directions(positions, pairs, (radius + label-gap) / unit, (label-gap + 4pt) / unit, label-directions, directions)
  text(fill: ink, fletcher.diagram(
    axes: (ltr, btt), cell-size: unit, spacing: 0pt,
    node-inset: 0pt, edge-stroke: thickness + ink,
    {
      for name in vertices {
        let p = positions.at(name)
        let style = vertex-styles.at(name, default: style)
        let title = if labels == false { [] } else if type(labels) == dictionary {
          labels.at(name, default: math.equation(name))
        } else { math.equation(name) }
        // A plain vertex with no label would be invisible; draw it as a dot.
        let style = if style == "plain" and title == [] { "dots" } else { style }
        fletcher.node(
          p, if style == "dots" { [] } else { title },
          name: label(name),
          radius: if style == "circles" { circle-radius } else if style == "dots" { radius } else { auto },
          inset: if style == "dots" { 0pt } else { 3pt },
          fill: if style == "dots" { ink } else { none },
          stroke: if style == "circles" { thickness + ink } else { none },
        )
        if style == "dots" and labels != false {
          let d = if name in label-directions { label-directions.at(name) } else if horizontal { "n" } else { automatic.at(name) }
          let direction = directions.at(d)
          let n = calc.sqrt(direction.at(0) * direction.at(0) + direction.at(1) * direction.at(1))
          fletcher.node(
            (rel: direction.map(x => x / n * (radius + label-gap)), to: label(name)),
            title, stroke: none, fill: none, inset: 0pt, snap: false,
          )
        }
      }
      for (a, b) in pairs { fletcher.edge(label(a), label(b)) }
    },
  ))
}
