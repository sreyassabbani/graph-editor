use super::*;
use std::collections::HashMap;

/// Scroll-wheel pan per tick in screen pixels (not grid units), and zoom per
/// Ctrl/Alt-wheel tick. Trackpads send many ticks, so both stay small.
const SCROLL_STEP: f64 = 12.;
const ZOOM_STEP: f64 = 1.08;

impl App {
    pub(super) fn size(&self) -> (f64, f64) {
        (
            self.area.width as f64 * self.cell.0 as f64,
            self.area.height as f64 * self.cell.1 as f64,
        )
    }
    pub(super) fn local(&self, col: u16, row: u16) -> Option<(f64, f64)> {
        if col < self.area.x
            || row < self.area.y
            || col >= self.area.right()
            || row >= self.area.bottom()
        {
            return None;
        }
        Some((
            (col - self.area.x) as f64 * self.cell.0 as f64 + self.cell.0 as f64 / 2.,
            (row - self.area.y) as f64 * self.cell.1 as f64 + self.cell.1 as f64 / 2.,
        ))
    }
    pub(super) fn world(&self, p: (f64, f64)) -> (f64, f64) {
        let (w, h) = self.size();
        self.camera.screen_to_world(p.0, p.1, w, h)
    }
    /// The vertex whose mark covers `p`: at least 16 px around its center
    /// (mouse reports are coarse), and the whole drawn circle when zoomed in,
    /// but never reaching halfway to a neighboring grid point.
    pub(super) fn hit(&self, p: (f64, f64)) -> Option<u64> {
        let (w, h) = self.size();
        let s = super::canvas::sizes(self.camera.zoom);
        self.graph
            .vertices
            .iter()
            .rev()
            .find(|v| {
                let drawn = match v.style {
                    VertexStyle::Circles => s.ring + s.stroke,
                    _ => s.dot,
                };
                let reach = (self.camera.zoom * 0.45).min(16_f64.max(drawn + 3.));
                let q = self.camera.world_to_screen(v.x, v.y, w, h);
                (q.0 - p.0).hypot(q.1 - p.1) <= reach
            })
            .map(|v| v.id)
    }
    /// The vertex a click at `p` is on: its mark, else its label.
    fn hit_vertex_or_label(&self, p: (f64, f64)) -> Option<u64> {
        self.hit(p).or_else(|| self.hit_label(p))
    }
    fn hit_edge(&self, p: (f64, f64)) -> Option<Edge> {
        let (w, h) = self.size();
        // Mouse reports are quantized to terminal cells, so allow half a cell
        // diagonal of slack. Choose the nearest segment, never its extension.
        let tolerance = (self.cell.0.hypot(self.cell.1) / 2. + 1.).max(6.);
        let vertices: HashMap<_, _> = self.graph.vertices.iter().map(|v| (v.id, v)).collect();
        self.graph
            .edges
            .iter()
            .filter_map(|edge| {
                let a = vertices.get(&edge.a)?;
                let b = vertices.get(&edge.b)?;
                let a = self.camera.world_to_screen(a.x, a.y, w, h);
                let b = self.camera.world_to_screen(b.x, b.y, w, h);
                let d = (b.0 - a.0, b.1 - a.1);
                let length_squared = d.0 * d.0 + d.1 * d.1;
                if length_squared == 0. {
                    return None;
                }
                let t = (((p.0 - a.0) * d.0 + (p.1 - a.1) * d.1) / length_squared).clamp(0., 1.);
                let distance = (p.0 - a.0 - t * d.0).hypot(p.1 - a.1 - t * d.1);
                (distance <= tolerance).then_some((distance, *edge))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, edge)| edge)
    }
    /// The grid point a click at `p` would place a vertex on: not over a
    /// vertex or edge (those clicks select), not on an occupied point, and
    /// not mid-gesture.
    fn placement(&self, p: (f64, f64)) -> Option<(f64, f64)> {
        if self.drag.is_some()
            || self.hit_vertex_or_label(p).is_some()
            || self.hit_edge(p).is_some()
        {
            return None;
        }
        let q = self.world(p);
        let g = (q.0.round(), q.1.round());
        (!self.graph.vertices.iter().any(|v| v.x == g.0 && v.y == g.1)).then_some(g)
    }
    fn update_hover(&mut self, p: Option<(f64, f64)>) {
        self.hover = p.and_then(|p| match self.drag {
            Some(Drag::Vertices { moved: true, .. }) => self.landing(p),
            _ => self.placement(p),
        });
        // The edge a click would select, unless a vertex wins there.
        self.hover_edge = p
            .filter(|_| self.drag.is_none())
            .filter(|&p| self.hit_vertex_or_label(p).is_none())
            .and_then(|p| self.hit_edge(p));
    }
    /// While dragging vertices: the grid point the grabbed vertex snaps to on
    /// release, or None when that drop would overlap another vertex.
    fn landing(&self, p: (f64, f64)) -> Option<(f64, f64)> {
        let Some(Drag::Vertices {
            anchor,
            ids,
            original,
            offset,
            ..
        }) = &self.drag
        else {
            return None;
        };
        let q = self.world(p);
        let before = original.vertex(*anchor)?;
        let target = ((q.0 + offset.0).round(), (q.1 + offset.1).round());
        original
            .clone()
            .translate(ids, target.0 - before.x, target.1 - before.y)
            .then_some(target)
    }
    pub(super) fn mouse(&mut self, m: MouseEvent) {
        if self.editing.is_some() {
            // Clicking anywhere saves the label and does nothing else: the
            // header shows no buttons to click, and a stray vertex or edit
            // under a click meant to dismiss the field would be a surprise.
            if matches!(m.kind, MouseEventKind::Down(_)) {
                self.commit_editing(false);
            }
            return;
        }
        if m.kind != MouseEventKind::Moved {
            self.quit_pending = false;
        }
        let (col, row, p) = self.pointer(&m);
        self.toolbar_hover = (row == 0 && p.is_none()).then_some(col);
        // The marker shows where a click acts: on the canvas on screen.
        self.on_displayed_camera(|app| app.update_hover(p));
        // Toolbar buttons act on release over the button they were pressed
        // on, like any button: sliding off cancels.
        if m.kind == MouseEventKind::Down(MouseButton::Left) && row == 0 {
            self.toolbar_press = self.toolbar_action(col, row);
            return;
        }
        if m.kind == MouseEventKind::Up(MouseButton::Left) {
            if let Some(pressed) = self.toolbar_press.take() {
                if self.toolbar_action(col, row) == Some(pressed) {
                    self.toolbar(pressed);
                }
                return;
            }
        }
        match m.kind {
            MouseEventKind::Down(MouseButton::Middle | MouseButton::Right) => {
                if let Some(p) = p {
                    self.drag = Some(Drag::Pan(p.0, p.1));
                    self.edge_start = None;
                    self.last_click = None;
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(p) = p {
                    self.status.clear();
                    self.on_displayed_camera(|app| app.left_down(p, m.modifiers));
                }
            }
            MouseEventKind::Drag(button) => {
                if let Some(p) = p {
                    if button == MouseButton::Left {
                        self.on_displayed_camera(|app| app.left_drag(p));
                    } else if let Some(Drag::Pan(x, y)) = &mut self.drag {
                        self.camera.x -= (p.0 - *x) / self.camera.zoom;
                        self.camera.y += (p.1 - *y) / self.camera.zoom;
                        *x = p.0;
                        *y = p.1;
                    }
                }
            }
            MouseEventKind::Up(button) => {
                // A different button's release must not finish the active gesture.
                match (&self.drag, button) {
                    (Some(Drag::Vertices { .. } | Drag::Marquee { .. }), MouseButton::Left) => {
                        self.on_displayed_camera(|app| app.left_up(p))
                    }
                    (Some(Drag::Pan(..)), MouseButton::Middle | MouseButton::Right) => {
                        self.drag = None
                    }
                    _ => {}
                }
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                if let Some(p) = p {
                    let up = m.kind == MouseEventKind::ScrollUp;
                    if m.modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    {
                        let (w, h) = self.size();
                        self.camera.zoom_at(
                            if up { ZOOM_STEP } else { 1. / ZOOM_STEP },
                            p.0,
                            p.1,
                            w,
                            h,
                        );
                    } else {
                        let step = SCROLL_STEP / self.camera.zoom;
                        let step = if up { step } else { -step };
                        // Shift-scroll pans sideways for wheels without tilt.
                        if m.modifiers.contains(KeyModifiers::SHIFT) {
                            self.camera.x -= step;
                        } else {
                            self.camera.y += step;
                        }
                    }
                }
            }
            MouseEventKind::ScrollLeft => self.camera.x -= SCROLL_STEP / self.camera.zoom,
            MouseEventKind::ScrollRight => self.camera.x += SCROLL_STEP / self.camera.zoom,
            _ => {}
        }
        // Panning, zooming and edits change what lies beneath the pointer.
        self.on_displayed_camera(|app| app.update_hover(p));
    }
    /// Run a canvas click handler against the camera on screen. Right after
    /// a zoom the old frame stays up until the new one is drawn, and clicks
    /// act on what is visible. Only hit-testing sees it: the handlers do not
    /// move the camera, and the real one is restored afterwards.
    fn on_displayed_camera(&mut self, handle: impl FnOnce(&mut Self)) {
        let shown = self.displayed_camera();
        if shown == self.camera {
            return handle(self);
        }
        let camera = std::mem::replace(&mut self.camera, shown);
        handle(self);
        self.camera = camera;
    }
    /// Ctrl/Alt/Super (or Shift, where the terminal reports it) toggle
    /// membership instead of replacing the selection.
    fn additive(mods: KeyModifiers) -> bool {
        mods.intersects(
            KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
        )
    }
    fn left_down(&mut self, p: (f64, f64), mods: KeyModifiers) {
        let additive = Self::additive(mods);
        if let Some(id) = self.hit_vertex_or_label(p) {
            if additive {
                self.selection.toggle_vertex(id);
                self.edge_start = None;
                self.last_click = None;
                return;
            }
            // Grabbing a member of a multi-selection moves the whole group.
            let narrow = self.selection.has_vertex(id) && self.selection.len() > 1;
            if !narrow {
                self.selection = Selection::vertex(id);
            }
            let q = self.world(p);
            let v = self.graph.vertex(id).unwrap();
            self.drag = Some(Drag::Vertices {
                anchor: id,
                // Whichever handle is grabbed, a selection moves as one set.
                ids: if narrow {
                    self.selection_group()
                } else {
                    vec![id]
                },
                original: self.graph.clone(),
                offset: (v.x - q.0, v.y - q.1),
                moved: false,
                narrow,
                grab: Grab::Vertex,
            });
        } else if let Some(edge) = self.hit_edge(p) {
            // Grabbing a selected edge moves the selection with it.
            if !additive && self.selection.has_edge(&edge) {
                self.grab_selection(p, Grab::Edge(edge));
                return;
            }
            if additive {
                self.selection.toggle_edge(edge);
            } else {
                self.selection = Selection::edge(edge);
            }
            self.edge_start = None;
            self.last_click = None;
            self.drag = None;
        } else if !additive && self.in_selection_box(p) {
            // As in other editors, the area of a multi-selection is a handle.
            self.grab_selection(p, Grab::Empty);
        } else {
            self.drag = Some(Drag::Marquee {
                start: p,
                end: p,
                additive,
            });
        }
    }
    /// Every vertex a move of the selection carries: its vertices and the
    /// ends of its edges.
    fn selection_group(&self) -> Vec<u64> {
        let mut ids = self.selection.vertices.clone();
        for e in &self.selection.edges {
            for id in [e.a, e.b] {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        ids.retain(|&id| self.graph.vertex(id).is_some());
        ids
    }
    /// Start moving the whole selection from a press at `p`.
    fn grab_selection(&mut self, p: (f64, f64), grab: Grab) {
        let ids = self.selection_group();
        let Some(anchor) = ids.first().copied() else {
            return;
        };
        let q = self.world(p);
        let v = self.graph.vertex(anchor).unwrap();
        self.drag = Some(Drag::Vertices {
            anchor,
            ids,
            original: self.graph.clone(),
            offset: (v.x - q.0, v.y - q.1),
            moved: false,
            narrow: false,
            grab,
        });
    }
    /// Whether `p` is inside the box drawn around a multi-selection.
    fn in_selection_box(&self, p: (f64, f64)) -> bool {
        let (w, h) = self.size();
        super::canvas::selection_box(&self.graph, &self.selection, self.camera, w, h)
            .is_some_and(|(lo, hi)| (lo.0..=hi.0).contains(&p.0) && (lo.1..=hi.1).contains(&p.1))
    }
    fn left_drag(&mut self, p: (f64, f64)) {
        let q = self.world(p);
        match &mut self.drag {
            Some(Drag::Vertices {
                anchor,
                ids,
                original,
                offset,
                moved,
                ..
            }) => {
                let before = original.vertex(*anchor).unwrap();
                let (dx, dy) = (q.0 + offset.0 - before.x, q.1 + offset.1 - before.y);
                // Ignore tiny mouse noise; retain the grab offset so the node
                // never jumps to the center of a text cell.
                if *moved || dx.hypot(dy) * self.camera.zoom > 4. {
                    *moved = true;
                    // The vertices follow the pointer exactly; the hover marker
                    // shows where the grabbed vertex lands (see `landing`).
                    let mut preview = original.clone();
                    if preview.translate(ids, dx, dy) && preview != self.graph {
                        self.graph = preview;
                    }
                    self.edge_start = None;
                    self.last_click = None;
                }
            }
            Some(Drag::Marquee { end, .. }) => *end = p,
            _ => {}
        }
    }
    fn left_up(&mut self, p: Option<(f64, f64)>) {
        match self.drag.take() {
            Some(Drag::Vertices {
                anchor,
                ids,
                original,
                offset,
                moved,
                narrow,
                grab,
            }) => {
                if moved {
                    let before = original.vertex(anchor).unwrap();
                    // Snap the grabbed vertex to the grid; the group keeps its shape.
                    let target = p
                        .map(|p| {
                            let q = self.world(p);
                            ((q.0 + offset.0).round(), (q.1 + offset.1).round())
                        })
                        .or_else(|| {
                            self.graph
                                .vertex(anchor)
                                .map(|v| (v.x.round(), v.y.round()))
                        });
                    let mut moved_graph = original.clone();
                    let ok = target.is_some_and(|(x, y)| {
                        moved_graph.translate(&ids, x - before.x, y - before.y)
                    });
                    self.graph = if ok { moved_graph } else { original.clone() };
                    if self.graph != original {
                        self.history.checkpoint(&original);
                    }
                } else if let Grab::Edge(edge) = grab {
                    // Released elsewhere: a cancelled click changes nothing.
                    if p.and_then(|p| self.hit_edge(p))
                        .is_some_and(|e| same_edge(&e, &edge))
                    {
                        self.selection = Selection::edge(edge);
                        self.edge_start = None;
                        self.last_click = None;
                    }
                } else if let Grab::Empty = grab {
                    // A click (no move) in a selection's box deselects, as
                    // in other editors; placing a vertex there would leave a
                    // stray one where the user meant to clear the selection.
                    if p.is_some() {
                        self.selection = Selection::default();
                        self.last_click = None;
                    }
                } else if p.and_then(|p| self.hit_vertex_or_label(p)) == Some(anchor) {
                    if narrow {
                        // Narrowing a multi-selection is selection work; it
                        // must not also pick up the pen.
                        self.selection = Selection::vertex(anchor);
                        self.last_click = Some((anchor, Instant::now(), None));
                    } else {
                        self.click_vertex(anchor);
                    }
                }
            }
            Some(Drag::Marquee {
                start,
                end,
                additive,
            }) => {
                if (end.0 - start.0).hypot(end.1 - start.1) < 4. {
                    // A click on empty canvas places a vertex, joined to the
                    // pen's vertex if it is down. A release off the canvas
                    // cancels, leaving the pen where it was.
                    if !additive && p.is_some() {
                        let pen = self.edge_start.take();
                        self.last_click = None;
                        self.click_empty(start, pen);
                    }
                    return;
                }
                // A box selection lifts the pen.
                self.edge_start = None;
                self.last_click = None;
                let picked = self.in_rect(start, end);
                if additive {
                    self.selection.extend(picked);
                } else {
                    self.selection = picked;
                }
            }
            other => self.drag = other,
        }
    }
    /// Click on canvas away from any vertex mark or edge. Its grid point may
    /// still hold a vertex (between the mark and half a grid step): then the
    /// click is on that vertex. Otherwise a vertex is placed there, joined to
    /// `pen`'s vertex, and the pen moves to it.
    fn click_empty(&mut self, p: (f64, f64), pen: Option<u64>) {
        let q = self.world(p);
        let g = (q.0.round(), q.1.round());
        if let Some(id) = self
            .graph
            .vertices
            .iter()
            .find(|v| v.x == g.0 && v.y == g.1)
            .map(|v| v.id)
        {
            self.edge_start = pen;
            self.selection = Selection::vertex(id);
            self.click_vertex(id);
            return;
        }
        // `add_vertex_at` records the undo step; the edge joins that step.
        let Some(id) = self.add_vertex_at(p) else {
            return;
        };
        if let Some(from) = pen {
            self.graph.connect(from, id);
            self.unroll = Some((from, id, Instant::now()));
        }
        self.edge_start = Some(id);
    }
    /// Place a new vertex on the grid point nearest `p` and select it, as one
    /// undo step. `None` if the point is taken.
    fn add_vertex_at(&mut self, p: (f64, f64)) -> Option<u64> {
        let q = self.world(p);
        let (x, y) = (q.0.round(), q.1.round());
        if x.is_finite()
            && y.is_finite()
            && self.graph.next_id < u64::MAX - 1
            && !self.graph.vertices.iter().any(|v| v.x == x && v.y == y)
        {
            self.history.checkpoint(&self.graph);
            let id = self
                .graph
                .add_styled_vertex(x, y, self.new_style, !self.new_unlabeled);
            self.selection = Selection::vertex(id);
            return Some(id);
        }
        None
    }
    /// Vertices inside the screen rectangle, and edges with both ends inside.
    fn in_rect(&self, a: (f64, f64), b: (f64, f64)) -> Selection {
        let (w, h) = self.size();
        let (lo, hi) = ((a.0.min(b.0), a.1.min(b.1)), (a.0.max(b.0), a.1.max(b.1)));
        let vertices: Vec<u64> = self
            .graph
            .vertices
            .iter()
            .filter(|v| {
                let s = self.camera.world_to_screen(v.x, v.y, w, h);
                (lo.0..=hi.0).contains(&s.0) && (lo.1..=hi.1).contains(&s.1)
            })
            .map(|v| v.id)
            .collect();
        let edges = self
            .graph
            .edges
            .iter()
            .filter(|e| vertices.contains(&e.a) && vertices.contains(&e.b))
            .copied()
            .collect();
        Selection { vertices, edges }
    }
    pub(super) fn select_all(&mut self) {
        self.selection = Selection {
            vertices: self.graph.vertices.iter().map(|v| v.id).collect(),
            edges: self.graph.edges.clone(),
        };
        self.edge_start = None;
    }
    /// Cycle the first selected vertex's style and give it to every selected vertex.
    pub(super) fn style_selection(&mut self) {
        let Some(next) = self
            .selection
            .vertices
            .iter()
            .find_map(|&id| self.graph.vertex(id))
            .map(|v| v.style.next())
        else {
            return;
        };
        self.history.checkpoint(&self.graph);
        for v in &mut self.graph.vertices {
            if self.selection.vertices.contains(&v.id) {
                v.style = next;
            }
        }
        self.new_style = next;
    }
    /// Join every pair of selected vertices (a complete graph on the selection).
    pub(super) fn connect_selection(&mut self) {
        let ids: Vec<u64> = self
            .selection
            .vertices
            .iter()
            .copied()
            .filter(|&id| self.graph.vertex(id).is_some())
            .collect();
        let present: std::collections::HashSet<_> = self
            .graph
            .edges
            .iter()
            .map(|e| (e.a.min(e.b), e.a.max(e.b)))
            .collect();
        let mut missing = Vec::new();
        for (i, &a) in ids.iter().enumerate() {
            for &b in &ids[i + 1..] {
                if a != b && !present.contains(&(a.min(b), a.max(b))) {
                    missing.push(Edge { a, b });
                }
            }
        }
        if missing.is_empty() {
            return;
        }
        self.history.checkpoint(&self.graph);
        let n = missing.len();
        self.graph.edges.extend(missing);
        self.say(match n {
            1 => "Added 1 edge".to_string(),
            n => format!("Added {n} edges, joining every selected pair"),
        });
    }
    /// A plain click on a vertex. The pen draws from vertex to vertex:
    /// with it down elsewhere the click joins the two (or, if they are joined
    /// already, just moves there) and the pen moves on; on this vertex it
    /// lifts; up, it goes down here. A double-click edits the label.
    pub(super) fn click_vertex(&mut self, id: u64) {
        if let Some((old, at, undo)) = self.last_click {
            if old == id && at.elapsed() < Duration::from_millis(400) {
                // A double-click is about the label: if its first click drew
                // an edge from the pen, take that edge back.
                if let Some(generation) = undo {
                    self.history.discard(&mut self.graph, generation);
                    self.unroll = None;
                }
                self.edge_start = None;
                self.last_click = None;
                self.selection = Selection::vertex(id);
                self.edit_label();
                return;
            }
        }
        let generation = self.history.generation();
        let pen = self.edge_start;
        self.edge_start = match pen {
            Some(start) if start == id => None,
            Some(start) => {
                self.connect(start, id);
                Some(id)
            }
            None => Some(id),
        };
        let drew = self.history.generation() != generation;
        if let (true, Some(start)) = (drew, pen) {
            self.unroll = Some((start, id, Instant::now()));
        }
        self.last_click = Some((id, Instant::now(), drew.then(|| self.history.generation())));
    }
    pub(super) fn connect(&mut self, a: u64, b: u64) {
        if a != b
            && self.graph.vertex(a).is_some()
            && self.graph.vertex(b).is_some()
            && !self
                .graph
                .edges
                .iter()
                .any(|e| (e.a == a && e.b == b) || (e.a == b && e.b == a))
        {
            self.history.checkpoint(&self.graph);
            self.graph.connect(a, b);
        }
    }
    /// Renumber every vertex from 1 and continue numbering new vertices.
    pub(super) fn restart_labels(&mut self) {
        let mut relabeled = self.graph.clone();
        relabeled.restart_labels();
        self.new_unlabeled = false;
        if relabeled != self.graph {
            self.history.checkpoint(&self.graph);
            self.graph = relabeled;
        }
        self.say(match self.graph.vertices.len() {
            0 => "Labels restart at 1".into(),
            n => format!("Relabeled 1–{n}"),
        });
    }
    /// One vertex starts from its label. Several start from an empty field
    /// that keeps their labels until something is typed; Enter then gives
    /// them all the same label (Enter on an empty field unlabels them).
    pub(super) fn edit_label(&mut self) {
        self.editing_placeholder = false;
        match self.selection.vertices.as_slice() {
            [] => {}
            [id] => {
                if let Some(v) = self.graph.vertex(*id) {
                    self.editing = Some(v.label.clone());
                }
            }
            _ => {
                self.editing = Some(String::new());
                self.editing_placeholder = true;
            }
        }
    }
    /// Save the label. `explicit` is Enter; a click away saves only what was
    /// typed, so dismissing an untouched field never erases labels.
    fn commit_editing(&mut self, explicit: bool) {
        if let Some(value) = self.editing.take() {
            if explicit || !self.editing_placeholder {
                self.commit_label(value);
            }
        }
        self.editing_placeholder = false;
    }
    #[cfg(test)]
    pub(super) fn copy_status_for_test(&mut self) {
        self.say("Copied");
    }
    /// Show an operation result; it clears itself after [`STATUS_TTL`].
    pub(super) fn say(&mut self, message: impl Into<String>) {
        self.status = message.into();
        self.status_at = Some(Instant::now());
    }
    /// Arrow keys: move the selected vertices one grid unit, or pan.
    fn nudge(&mut self, dx: f64, dy: f64) {
        if self.selection.vertices.is_empty() {
            let step = 32. / self.camera.zoom;
            self.camera.x += dx * step;
            self.camera.y += dy * step;
            return;
        }
        let mut moved = self.graph.clone();
        if moved.translate(&self.selection.vertices, dx, dy) {
            self.history.checkpoint(&self.graph);
            self.graph = moved;
        } else {
            self.say("Another vertex is in the way");
        }
    }
    /// Apply the edited label. Clearing a label makes new vertices unlabeled
    /// too, until a vertex is given a label again.
    fn commit_label(&mut self, value: String) {
        let ids = self.selection.vertices.clone();
        if ids.is_empty() {
            return;
        }
        self.new_unlabeled = value.is_empty();
        if ids
            .iter()
            .any(|&id| self.graph.vertex(id).is_some_and(|v| v.label != value))
        {
            self.history.checkpoint(&self.graph);
            for id in ids {
                if let Some(v) = self.graph.vertex_mut(id) {
                    v.label.clone_from(&value);
                }
            }
        }
    }
    pub(super) fn delete_selected(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        let selection = std::mem::take(&mut self.selection);
        let before = self.graph.clone();
        self.graph.edges.retain(|e| !selection.has_edge(e));
        for id in selection.vertices {
            self.graph.remove_vertex(id);
        }
        if self.graph != before {
            self.history.checkpoint(&before);
        }
        self.edge_start = None;
        self.last_click = None;
    }
    fn step_history(&mut self, redo: bool) {
        // Discard an uncommitted drag preview before applying graph history.
        if let Some(Drag::Vertices { original, .. }) = self.drag.take() {
            self.graph = original;
        }
        if redo {
            self.history.redo(&mut self.graph);
        } else {
            self.history.undo(&mut self.graph);
        }
        self.edge_start = None;
        self.last_click = None;
        self.reconcile_selection();
    }
    fn reconcile_selection(&mut self) {
        self.selection.retain_existing(&self.graph);
    }
    /// A toolbar action from its key: like its button, and like a dimmed
    /// button it does nothing (not even lift the pen) when it cannot apply.
    fn shortcut(&mut self, action: super::view::ToolbarAction) {
        if self
            .toolbar_buttons(u16::MAX)
            .iter()
            .any(|b| b.action == action && b.enabled)
        {
            self.toolbar(action);
        }
    }
    /// Run a toolbar action, from its button or its key: both lift the pen
    /// the same way.
    pub(super) fn toolbar(&mut self, action: super::view::ToolbarAction) {
        use super::view::ToolbarAction;
        self.status.clear();
        // Viewing and copying leave the pen where it is; edits lift it.
        if !matches!(
            action,
            ToolbarAction::Fit | ToolbarAction::Zoom(_) | ToolbarAction::Copy
        ) {
            self.edge_start = None;
        }
        match action {
            ToolbarAction::Undo => {
                self.step_history(false);
            }
            ToolbarAction::Redo => {
                self.step_history(true);
            }
            ToolbarAction::Fit => {
                let (w, h) = self.size();
                self.camera.fit(&self.graph, w, h);
            }
            ToolbarAction::Copy => self.copy(),
            ToolbarAction::Label => self.edit_label(),
            ToolbarAction::Relabel => self.restart_labels(),
            ToolbarAction::Style => self.style_selection(),
            ToolbarAction::Connect => self.connect_selection(),
            ToolbarAction::Delete => self.delete_selected(),
            ToolbarAction::Zoom(factor) => {
                let (w, h) = self.size();
                self.camera.zoom_at(factor, w / 2., h / 2., w, h);
            }
        }
        self.reconcile_selection();
        self.last_click = None;
    }
    pub(super) fn key(&mut self, code: KeyCode, mods: KeyModifiers) -> bool {
        use super::view::ToolbarAction;
        // A key between two clicks makes them two clicks, not a double-click.
        self.last_click = None;
        if let Some(s) = &mut self.editing {
            match code {
                KeyCode::Esc => {
                    self.editing = None;
                    self.editing_placeholder = false;
                }
                KeyCode::Enter => self.commit_editing(true),
                KeyCode::Backspace => {
                    // On the untouched field it removes nothing, so the
                    // labels stay as they are.
                    if s.pop().is_some() {
                        self.editing_placeholder = false;
                    }
                }
                KeyCode::Char(c)
                    if !mods.intersects(
                        KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                    ) =>
                {
                    s.push(c);
                    self.editing_placeholder = false;
                }
                _ => {}
            }
            return false;
        }
        if code == KeyCode::Esc {
            if let Some(drag) = self.drag.take() {
                if let Drag::Vertices { original, .. } = drag {
                    self.graph = original;
                }
                self.edge_start = None;
                return false;
            }
            if self.edge_start.take().is_some() {
                return false;
            }
            // Esc only ever cancels; quitting is q.
            self.selection = Selection::default();
            return false;
        }
        if code != KeyCode::Char('q') {
            if std::mem::take(&mut self.quit_pending) {
                self.status.clear();
            }
        }
        match (code, mods) {
            (KeyCode::Char('q'), _) => {
                // Deleting (or undoing) everything is no excuse to lose a
                // drawing that only Undo or Redo still holds.
                let safe = if self.graph.vertices.is_empty() {
                    // Safe only if no drawing on either side is uncopied.
                    self.history
                        .nearest_drawings()
                        .iter()
                        .all(|d| d.is_none() || *d == self.delivered.as_ref())
                } else {
                    self.delivered.as_ref() == Some(&self.graph)
                };
                if safe || self.quit_pending {
                    return true;
                }
                self.quit_pending = true;
                self.say("Not copied yet. Press q again to quit anyway.");
                return false;
            }
            (KeyCode::Left, m) if m.is_empty() => self.nudge(-1., 0.),
            (KeyCode::Right, m) if m.is_empty() => self.nudge(1., 0.),
            (KeyCode::Up, m) if m.is_empty() => self.nudge(0., 1.),
            (KeyCode::Down, m) if m.is_empty() => self.nudge(0., -1.),
            (KeyCode::Char('c'), m) if m.contains(KeyModifiers::CONTROL) => self.copy(),
            (KeyCode::Char('z' | 'Z'), m)
                if m.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) =>
            {
                self.step_history(m.contains(KeyModifiers::SHIFT) || code == KeyCode::Char('Z'))
            }
            (KeyCode::Char('u'), m) if m.is_empty() => self.step_history(false),
            (KeyCode::Char('U'), m) if m.is_empty() || m == KeyModifiers::SHIFT => {
                self.step_history(true)
            }
            (KeyCode::Char('r' | 'y'), m) if m.contains(KeyModifiers::CONTROL) => {
                self.step_history(true)
            }
            (KeyCode::Enter, _) => self.shortcut(ToolbarAction::Label),
            (KeyCode::Char('a'), m) if m.is_empty() || m.contains(KeyModifiers::CONTROL) => {
                self.select_all()
            }
            (KeyCode::Char('s'), m) if m.is_empty() => self.shortcut(ToolbarAction::Style),
            (KeyCode::Char('R'), m) if m.is_empty() || m == KeyModifiers::SHIFT => {
                self.shortcut(ToolbarAction::Relabel)
            }
            (KeyCode::Char('c'), m) if m.is_empty() => self.shortcut(ToolbarAction::Connect),
            (KeyCode::Delete | KeyCode::Backspace, _) => self.shortcut(ToolbarAction::Delete),
            (KeyCode::Char('+') | KeyCode::Char('='), _) => {
                let (w, h) = self.size();
                self.camera.zoom_at(1.2, w / 2., h / 2., w, h);
            }
            (KeyCode::Char('-'), _) => {
                let (w, h) = self.size();
                self.camera.zoom_at(1. / 1.2, w / 2., h / 2., w, h);
            }
            (KeyCode::Char('f'), _) => {
                let (w, h) = self.size();
                self.camera.fit(&self.graph, w, h);
            }
            _ => {}
        }
        self.reconcile_selection();
        self.edge_start = self
            .edge_start
            .filter(|id| self.graph.vertex(*id).is_some());
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::test_support::app;
    fn mouse(app: &mut App, kind: MouseEventKind, column: u16, row: u16) {
        app.mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        });
    }
    fn click(app: &mut App, column: u16, row: u16) {
        mouse(app, MouseEventKind::Down(MouseButton::Left), column, row);
        mouse(app, MouseEventKind::Up(MouseButton::Left), column, row);
    }
    fn ctrl_click(app: &mut App, column: u16, row: u16) {
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            app.mouse(MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::CONTROL,
            });
        }
    }
    fn sorted(mut ids: Vec<u64>) -> Vec<u64> {
        ids.sort();
        ids
    }
    #[test]
    fn clearing_labels_unlabels_selection_and_new_vertices_until_relabeled() {
        let mut app = app();
        let b = app.graph.add_vertex(2., 0.);
        app.selection.vertices = vec![1, b];
        app.edit_label();
        assert_eq!(app.editing.as_deref(), Some(""));
        app.key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.graph.vertices.iter().all(|v| v.label.is_empty()));
        assert!(app.new_unlabeled);
        app.add_vertex_at((10., 10.));
        let c = app.selection.vertices[0];
        assert_eq!(app.graph.vertex(c).unwrap().label, "");
        // One undo restores both labels.
        app.step_history(false);
        app.step_history(false);
        assert_eq!(app.graph.vertex(b).unwrap().label, b.to_string());
        app.selection = Selection::vertex(1);
        app.edit_label();
        app.key(KeyCode::Char('x'), KeyModifiers::NONE);
        app.key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(!app.new_unlabeled);
        app.add_vertex_at((10., 10.));
        let d = app.selection.vertices[0];
        assert_eq!(app.graph.vertex(d).unwrap().label, d.to_string());
    }
    #[test]
    fn relabel_renumbers_from_one_and_continues_the_counter() {
        let mut app = app();
        let b = app.graph.add_vertex(2., 0.);
        let c = app.graph.add_vertex(4., 0.);
        app.graph.remove_vertex(1);
        app.graph.vertex_mut(c).unwrap().label = "z".into();
        app.new_unlabeled = true;
        app.add_vertex_at((10., 10.));
        let d = app.selection.vertices[0];
        assert_eq!(app.graph.vertex(d).unwrap().label, "");
        app.key(KeyCode::Char('R'), KeyModifiers::SHIFT);
        let labels: Vec<_> = app
            .graph
            .vertices
            .iter()
            .map(|v| v.label.as_str())
            .collect();
        assert_eq!(labels, ["1", "2", "3"]);
        assert!(!app.new_unlabeled);
        app.add_vertex_at((300., 10.));
        let e = app.selection.vertices[0];
        assert_ne!(e, d);
        assert_eq!(app.graph.vertex(e).unwrap().label, "4");
        // IDs are untouched, and one undo restores the old labels and counter.
        assert_eq!(app.graph.vertices[0].id, b);
        app.step_history(false);
        app.step_history(false);
        assert_eq!(app.graph.vertex(c).unwrap().label, "z");
        assert_eq!(app.graph.next_label, 4);
    }
    #[test]
    fn box_select_toggle_and_bulk_style_connect_move_delete() {
        // One world unit is 6 columns by 3 rows; vertex 1 sits at column 40, row 12.
        let mut app = app();
        let b = app.graph.add_vertex(2., 0.);
        let c = app.graph.add_vertex(0., 2.);
        app.graph.connect(1, b);
        // Dragging a box over empty canvas selects 1, b and their edge; no vertex is added.
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), 36, 10);
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), 56, 14);
        assert!(app.scene().state.marquee.is_some());
        mouse(&mut app, MouseEventKind::Up(MouseButton::Left), 56, 14);
        assert!(app.scene().state.marquee.is_none());
        assert_eq!(sorted(app.selection.vertices.clone()), vec![1, b]);
        assert_eq!(app.selection.edges, vec![Edge { a: 1, b }]);
        assert_eq!(app.graph.vertices.len(), 3);
        // A modifier click toggles membership.
        ctrl_click(&mut app, 40, 6);
        assert_eq!(sorted(app.selection.vertices.clone()), vec![1, b, c]);
        ctrl_click(&mut app, 46, 12); // the edge
        assert!(app.selection.edges.is_empty());
        // Connect makes the selection a complete graph; Style restyles all of it.
        app.key(KeyCode::Char('c'), KeyModifiers::NONE);
        assert_eq!(app.graph.edges.len(), 3);
        app.key(KeyCode::Char('s'), KeyModifiers::NONE);
        assert!(app
            .graph
            .vertices
            .iter()
            .all(|v| v.style == VertexStyle::Circles));
        assert_eq!(app.new_style, VertexStyle::Circles);
        // Dragging any selected vertex moves the group as one undo step.
        let before = app.graph.clone();
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), 52, 12);
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), 58, 12);
        mouse(&mut app, MouseEventKind::Up(MouseButton::Left), 58, 12);
        let at = |app: &App, id| {
            let v = app.graph.vertex(id).unwrap();
            (v.x, v.y)
        };
        assert_eq!(
            (at(&app, 1), at(&app, b), at(&app, c)),
            ((1., 0.), (3., 0.), (1., 2.))
        );
        assert_eq!(app.selection.vertices.len(), 3);
        assert!(app.history.undo(&mut app.graph));
        assert_eq!(app.graph, before);
        // Delete removes the whole selection in one step.
        app.key(KeyCode::Backspace, KeyModifiers::NONE);
        assert!(app.graph.vertices.is_empty() && app.graph.edges.is_empty());
        app.key(KeyCode::Char('u'), KeyModifiers::NONE);
        assert_eq!(app.graph, before);
        // Select all, then Esc clears the selection instead of quitting.
        app.key(KeyCode::Char('a'), KeyModifiers::NONE);
        assert_eq!(app.selection.len(), 6);
        assert!(!app.key(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.selection.is_empty());
    }
    #[test]
    fn plain_click_on_member_narrows_and_group_move_refuses_collisions() {
        let mut app = app();
        let b = app.graph.add_vertex(2., 0.);
        app.graph.add_vertex(4., 0.);
        // Moving {1, b} right by two units would put b on the unselected vertex at (4, 0).
        let original = app.graph.clone();
        app.selection = Selection {
            vertices: vec![1, b],
            edges: Vec::new(),
        };
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), 40, 12);
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), 52, 12);
        mouse(&mut app, MouseEventKind::Up(MouseButton::Left), 52, 12);
        assert_eq!(app.graph, original); // b would land on unselected c: rolled back
        assert!(!app.history.undo(&mut app.graph));
        click(&mut app, 40, 12);
        assert_eq!(app.selection, Selection::vertex(1));
        assert_eq!(app.edge_start, None, "narrowing does not pick up the pen");
    }
    #[test]
    fn button_release_preserves_operation_errors() {
        let mut app = app();
        app.status = "Copy failed".into();
        mouse(&mut app, MouseEventKind::Up(MouseButton::Left), 2, 0);
        assert_eq!(app.status, "Copy failed");
    }
    #[test]
    fn drag_follows_pointer_marks_landing_and_collision_rolls_back() {
        let mut app = app();
        app.graph.add_vertex(2., 0.);
        let original = app.graph.clone();
        // One world unit is 6 columns. The vertex follows the pointer; the
        // hover marker shows the grid point it would land on.
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), 40, 12);
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), 42, 12);
        assert!((app.graph.vertices[0].x - 1. / 3.).abs() < 1e-9);
        assert_eq!(app.hover, Some((0., 0.)));
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), 44, 12);
        assert!((app.graph.vertices[0].x - 2. / 3.).abs() < 1e-9);
        assert_eq!(app.hover, Some((1., 0.)));
        // Over the other vertex's grid point the drop is invalid: no marker.
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), 52, 12);
        assert_eq!(app.hover, None);
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), 44, 12);
        mouse(&mut app, MouseEventKind::Up(MouseButton::Left), 42, 12);
        assert_eq!(app.graph, original); // snaps back without an undo entry
        assert!(!app.history.undo(&mut app.graph));
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), 40, 12);
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), 51, 12);
        mouse(&mut app, MouseEventKind::Up(MouseButton::Left), 51, 12);
        assert_eq!(app.graph, original); // occupied target never merges vertices
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), 40, 12);
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), 46, 12);
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), 52, 12);
        mouse(&mut app, MouseEventKind::Up(MouseButton::Left), 52, 12);
        assert_eq!(app.graph, original); // exact collision uses release position
        assert!(app.graph.validate().is_ok());
    }
    #[test]
    fn completed_drag_is_one_undo_step_and_escape_restores_preview() {
        let mut app = app();
        let original = app.graph.clone();
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), 40, 12);
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), 46, 12);
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), 52, 12);
        mouse(&mut app, MouseEventKind::Up(MouseButton::Left), 52, 12);
        assert_eq!(app.graph.vertices[0].x, 2.);
        assert!(app.history.undo(&mut app.graph));
        assert_eq!(app.graph, original);
        assert!(!app.history.undo(&mut app.graph));
        assert!(app.history.redo(&mut app.graph));
        app.last_click = None;
        let before_cancel = app.graph.clone();
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), 52, 12);
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), 58, 12);
        assert!(!app.key(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.graph, before_cancel);
    }
    #[test]
    fn hover_marks_only_where_a_click_would_place_a_vertex() {
        let mut app = app();
        let b = app.graph.add_vertex(2., 0.);
        app.graph.connect(1, b);
        mouse(&mut app, MouseEventKind::Moved, 40, 15); // empty grid point (0, -1)
        assert_eq!(app.hover, Some((0., -1.)));
        mouse(&mut app, MouseEventKind::Moved, 40, 12); // on vertex 1
        assert_eq!(app.hover, None);
        mouse(&mut app, MouseEventKind::Moved, 46, 12); // on the edge, at (1, 0)
        assert_eq!(app.hover, None);
        mouse(&mut app, MouseEventKind::Moved, 0, 0); // toolbar
        assert_eq!((app.hover, app.toolbar_hover), (None, Some(0)));
        mouse(&mut app, MouseEventKind::Down(MouseButton::Right), 40, 15);
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Right), 41, 15);
        assert_eq!(app.hover, None, "no placement marker while panning");
    }
    #[test]
    fn arrows_nudge_selection_one_unit_or_pan() {
        let mut app = app();
        let b = app.graph.add_vertex(2., 0.);
        app.selection = Selection::vertex(1);
        app.key(KeyCode::Right, KeyModifiers::NONE);
        assert_eq!((app.graph.vertices[0].x, app.graph.vertices[0].y), (1., 0.));
        app.key(KeyCode::Right, KeyModifiers::NONE); // b is at (2, 0)
        assert_eq!(app.graph.vertices[0].x, 1.);
        assert!(app.status.contains("in the way"));
        app.key(KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(app.graph.vertices[0].y, 1.);
        app.step_history(false);
        assert_eq!(app.graph.vertices[0].y, 0.);
        app.selection = Selection::default();
        let camera = app.camera;
        app.key(KeyCode::Left, KeyModifiers::NONE);
        assert!(app.camera.x < camera.x);
        assert_eq!(app.graph.vertex(b).unwrap().x, 2.);
    }
    #[test]
    fn escape_never_quits_and_results_expire() {
        let mut app = app();
        for _ in 0..4 {
            assert!(!app.key(KeyCode::Esc, KeyModifiers::NONE));
        }
        app.copy_status_for_test();
        assert!(!app.status.is_empty());
        app.status_at = Some(Instant::now() - STATUS_TTL);
        assert!(app.expire_status());
        assert!(app.status.is_empty());
        // A pending quit confirmation does not expire, and another key dismisses it.
        assert!(!app.key(KeyCode::Char('q'), KeyModifiers::NONE));
        app.status_at = Some(Instant::now() - STATUS_TTL);
        assert!(!app.expire_status());
        app.key(KeyCode::Char('f'), KeyModifiers::NONE);
        assert!(app.status.is_empty() && !app.quit_pending);
    }
    #[test]
    fn escape_cancels_pending_edge_and_quit_requires_confirmation() {
        let mut app = app();
        app.edge_start = Some(1);
        assert!(!app.key(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.edge_start.is_none());
        app.graph.add_vertex(2., 0.);
        assert!(!app.key(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(app.key(KeyCode::Char('q'), KeyModifiers::NONE));
    }

    #[test]
    fn edge_selection_backspace_and_history_preserve_vertices() {
        let mut app = app();
        let b = app.graph.add_vertex(1., 0.);
        app.graph.connect(1, b);
        let original = app.graph.clone();
        // Adjacent nodes: the midpoint must select their edge, not snap to a
        // node's whole grid cell or accidentally create another vertex.
        click(&mut app, 43, 12);
        assert_eq!(app.selection, Selection::edge(Edge { a: 1, b }));
        assert!(app.selection.vertices.is_empty());
        assert!(app.edge_start.is_none());
        assert_eq!(app.graph, original);
        app.key(KeyCode::Backspace, KeyModifiers::NONE);
        assert!(app.graph.edges.is_empty());
        assert_eq!(app.graph.vertices, original.vertices);
        assert!(app.selection.edges.is_empty());
        for (undo, undo_mods, redo, redo_mods) in [
            (
                'z',
                KeyModifiers::SUPER,
                'z',
                KeyModifiers::SUPER | KeyModifiers::SHIFT,
            ),
            ('u', KeyModifiers::NONE, 'U', KeyModifiers::SHIFT),
            ('z', KeyModifiers::CONTROL, 'r', KeyModifiers::CONTROL),
            ('u', KeyModifiers::NONE, 'y', KeyModifiers::CONTROL),
            (
                'u',
                KeyModifiers::NONE,
                'Z',
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
        ] {
            app.key(KeyCode::Char(undo), undo_mods);
            assert_eq!(app.graph, original);
            app.key(KeyCode::Char(redo), redo_mods);
            assert!(app.graph.edges.is_empty());
            assert_eq!(app.graph.vertices, original.vertices);
        }
        app.key(KeyCode::Char('u'), KeyModifiers::NONE);
        click(&mut app, 43, 12);
        assert!(!app.key(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.selection.edges.is_empty());
        click(&mut app, 43, 12);
        click(&mut app, 40, 12); // endpoint selection wins near the node
        assert_eq!(app.selection, Selection::vertex(1));
        assert!(app.selection.edges.is_empty());
    }

    #[test]
    fn edge_hit_testing_tracks_camera_and_rejects_segment_extensions() {
        let mut app = app();
        let b = app.graph.add_vertex(3., 2.);
        app.graph.connect(1, b);
        app.camera.x = -2.;
        app.camera.y = 3.;
        app.camera.zoom = 72.;
        let (w, h) = app.size();
        let middle = app.camera.world_to_screen(1.5, 1., w, h);
        assert_eq!(app.hit_edge(middle), Some(Edge { a: 1, b }));
        assert!(app.hit_edge((middle.0, middle.1 + 30.)).is_none());
        let beyond = app.camera.world_to_screen(4.5, 3., w, h);
        assert!(app.hit_edge(beyond).is_none());
    }

    #[test]
    fn editing_text_keeps_history_keys_and_backspace_local_to_the_field() {
        let mut app = app();
        let original = app.graph.clone();
        app.selection = Selection::vertex(1);
        app.edit_label();
        app.key(KeyCode::Char('u'), KeyModifiers::NONE);
        app.key(KeyCode::Char('U'), KeyModifiers::SHIFT);
        app.key(KeyCode::Char('z'), KeyModifiers::SUPER);
        assert_eq!(app.editing.as_deref(), Some("1uU"));
        app.key(KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(app.editing.as_deref(), Some("1u"));
        app.key(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.graph, original);
    }

    #[test]
    fn the_pen_draws_paths_by_clicking_along_them() {
        // One world unit is 6 columns by 3 rows; vertex 1 sits at column 40, row 12.
        let mut app = app();
        mouse(&mut app, MouseEventKind::Moved, 52, 12);
        assert_eq!(app.hover, Some((2., 0.)));
        // Placing a vertex puts the pen on it; with no pen before, no edge.
        click(&mut app, 52, 12);
        let b = app.selection.vertices[0];
        assert!(app.graph.edges.is_empty());
        assert_eq!(app.edge_start, Some(b));
        // Each empty click places the next vertex, joined to the last.
        click(&mut app, 64, 12);
        let c = app.selection.vertices[0];
        assert_eq!(app.graph.edges, vec![Edge { a: b, b: c }]);
        assert_eq!(app.edge_start, Some(c));
        // Clicking an existing vertex joins it and moves the pen there.
        app.last_click = None;
        click(&mut app, 40, 12);
        assert_eq!(app.graph.edges.len(), 2);
        assert_eq!(app.edge_start, Some(1));
        // Going back along an existing edge only moves the pen: no duplicate.
        app.last_click = None;
        click(&mut app, 64, 12);
        assert_eq!(app.graph.edges.len(), 2);
        assert_eq!(app.edge_start, Some(c));
        // Clicking the pen's own vertex lifts it; so do Esc and right-click.
        app.last_click = None;
        click(&mut app, 64, 12);
        assert_eq!(app.edge_start, None);
        click(&mut app, 40, 12);
        assert!(!app.key(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.edge_start, None);
        app.last_click = None;
        click(&mut app, 40, 12);
        mouse(&mut app, MouseEventKind::Down(MouseButton::Right), 40, 15);
        mouse(&mut app, MouseEventKind::Up(MouseButton::Right), 40, 15);
        assert_eq!(app.edge_start, None);
        // Placing and joining a vertex is one undo step.
        let before = app.graph.clone();
        app.last_click = None;
        click(&mut app, 40, 12);
        click(&mut app, 40, 18);
        assert_eq!(app.graph.vertices.len(), 4);
        assert_eq!(app.graph.edges.len(), 3);
        let steps = app.history.depth();
        app.step_history(false);
        assert_eq!(app.graph, before);
        assert_eq!(app.history.depth(), steps - 1, "one step, not two");
        assert_eq!(app.edge_start, None, "undo lifts the pen");
    }
    #[test]
    fn clicks_near_a_vertex_select_it_and_a_release_off_canvas_cancels() {
        let mut app = app();
        let b = app.graph.add_vertex(2., 0.);
        let before = app.graph.clone();
        // Column 50 is outside vertex b's 16 px mark but rounds to its grid
        // point: it is a click on b, not a lost click.
        app.selection = Selection::vertex(1);
        click(&mut app, 50, 12);
        assert_eq!(app.selection, Selection::vertex(b));
        assert_eq!(app.graph, before);
        // Pressing on canvas and releasing over the toolbar places nothing.
        app.edge_start = None;
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), 40, 18);
        mouse(&mut app, MouseEventKind::Up(MouseButton::Left), 40, 0);
        assert_eq!(app.graph, before);
    }
    #[test]
    fn zoomed_in_circles_and_plain_labels_are_clickable() {
        let mut app = app();
        app.graph.vertices[0].style = VertexStyle::Circles;
        app.camera.zoom = 96.;
        let (w, h) = app.size();
        let center = app.camera.world_to_screen(0., 0., w, h);
        let ring = super::super::canvas::sizes(96.).ring;
        assert!(ring > 16.);
        assert_eq!(
            app.hit((center.0 + ring, center.1)),
            Some(1),
            "on the drawn ring"
        );
        assert_eq!(
            app.hit((center.0 + 96. * 0.6, center.1)),
            None,
            "past half a grid step"
        );
        // A plain vertex is its label: clicking the word selects it.
        let mut app = super::super::test_support::app();
        app.graph.vertices[0].style = VertexStyle::Plain;
        app.graph.vertices[0].label = "hypotenuse".into();
        let (w, h) = app.size();
        let center = app.camera.world_to_screen(0., 0., w, h);
        let word = (center.0 + 20., center.1);
        assert_eq!(app.hit(word), None);
        assert_eq!(app.hit_label(word), Some(1));
        app.selection = Selection::default();
        app.left_down(word, KeyModifiers::NONE);
        app.left_up(Some(word));
        assert_eq!(app.selection, Selection::vertex(1));
        assert_eq!(
            app.graph.vertices.len(),
            1,
            "no vertex placed under the word"
        );
    }
    #[test]
    fn double_click_edits_the_label_and_right_drag_only_pans() {
        let mut app = app();
        click(&mut app, 40, 12);
        click(&mut app, 40, 12);
        assert_eq!(app.editing.as_deref(), Some("1"));
        assert_eq!(app.edge_start, None);
        assert_eq!(app.graph.vertices[0].style, VertexStyle::Dots);
        app.key(KeyCode::Esc, KeyModifiers::NONE);
        // Style is `s` (or the button); new vertices take the last style.
        app.key(KeyCode::Char('s'), KeyModifiers::NONE);
        assert_eq!(app.graph.vertices[0].style, VertexStyle::Circles);
        app.edge_start = None;
        click(&mut app, 52, 12);
        assert_eq!(app.graph.vertices[1].style, VertexStyle::Circles);
        app.graph.vertices.pop();
        app.selection = Selection::default();
        app.edge_start = None;
        let original = app.graph.clone();
        let camera = app.camera;
        mouse(&mut app, MouseEventKind::Down(MouseButton::Right), 40, 12);
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Right), 46, 12);
        mouse(&mut app, MouseEventKind::Up(MouseButton::Right), 46, 12);
        assert_ne!(app.camera, camera);
        assert_eq!(app.graph, original);
        assert!(app.drag.is_none());
    }
    #[test]
    fn clicking_away_saves_only_typed_labels_and_does_nothing_else() {
        let mut app = app();
        app.selection = Selection::vertex(1);
        app.edit_label();
        app.key(KeyCode::Char('x'), KeyModifiers::NONE);
        // The canvas previews the typed label before it is saved.
        assert_eq!(app.preview_graph().vertices[0].label, "1x");
        assert_eq!(app.graph.vertices[0].label, "1");
        click(&mut app, 52, 12);
        assert!(app.editing.is_none());
        assert_eq!(app.graph.vertices[0].label, "1x");
        assert_eq!(app.graph.vertices.len(), 1, "the click only saved");
        // Several vertices: an untouched field keeps their labels, even when
        // the click lands where a toolbar button would be.
        let b = app.graph.add_vertex(2., 0.);
        let before = app.graph.clone();
        app.selection.vertices = vec![1, b];
        app.edit_label();
        assert_eq!(app.preview_graph(), before);
        let delete = app
            .toolbar_buttons(80)
            .iter()
            .find(|b| b.action == super::super::view::ToolbarAction::Delete)
            .unwrap()
            .start as u16;
        click(&mut app, delete, 0);
        assert!(app.editing.is_none());
        assert_eq!(app.graph, before);
        // Enter on the empty field unlabels them on purpose.
        app.edit_label();
        app.key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.graph.vertices.iter().all(|v| v.label.is_empty()));
    }
    #[test]
    fn toolbar_buttons_stay_put_and_act_on_release_over_the_same_button() {
        use super::super::view::ToolbarAction;
        let mut app = app();
        let b = app.graph.add_vertex(2., 0.);
        let c = app.graph.add_vertex(4., 0.);
        let at = |app: &App, action| {
            app.toolbar_buttons(80)
                .iter()
                .find(|x| x.action == action)
                .unwrap()
                .start as u16
                + 1
        };
        let empty = app.toolbar_buttons(80);
        app.selection.vertices = vec![1, b, c];
        let connect = at(&app, ToolbarAction::Connect);
        assert_eq!(
            empty.iter().map(|b| b.start).collect::<Vec<_>>(),
            app.toolbar_buttons(80)
                .iter()
                .map(|b| b.start)
                .collect::<Vec<_>>(),
            "selection never moves a button"
        );
        // Pressing and sliding off cancels.
        mouse(
            &mut app,
            MouseEventKind::Down(MouseButton::Left),
            connect,
            0,
        );
        mouse(
            &mut app,
            MouseEventKind::Up(MouseButton::Left),
            connect + 30,
            0,
        );
        assert!(app.graph.edges.is_empty());
        // Two quick clicks on Connect: the second hits a dimmed Connect, not Delete.
        click(&mut app, connect, 0);
        assert_eq!(app.graph.edges.len(), 3);
        assert!(app.status.contains("3 edges"));
        click(&mut app, connect, 0);
        assert_eq!(app.graph.vertices.len(), 3);
        assert_eq!(app.graph.edges.len(), 3);
        // A dimmed button or blank space leaves the pen down.
        app.edge_start = Some(1);
        let redo = at(&app, ToolbarAction::Redo);
        click(&mut app, redo, 0);
        click(&mut app, 70, 0);
        assert_eq!(app.edge_start, Some(1));
    }
    #[test]
    fn quitting_asks_while_undo_still_holds_an_uncopied_drawing() {
        let mut app = app();
        app.graph.add_vertex(2., 0.);
        app.key(KeyCode::Char('a'), KeyModifiers::NONE);
        app.key(KeyCode::Backspace, KeyModifiers::NONE);
        assert!(app.graph.vertices.is_empty());
        assert!(!app.key(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(app.status.contains("Not copied"));
        // Once that drawing was copied, an empty canvas quits at once.
        app.step_history(false);
        app.delivered = Some(app.graph.clone());
        app.key(KeyCode::Char('a'), KeyModifiers::NONE);
        app.key(KeyCode::Backspace, KeyModifiers::NONE);
        assert!(app.key(KeyCode::Char('q'), KeyModifiers::NONE));
    }
    #[test]
    fn clicks_act_on_the_zoom_on_screen_until_the_new_frame_arrives() {
        let mut picker = Picker::from_fontsize((8, 16));
        picker.set_protocol_type(ProtocolType::Kitty);
        let mut app = App::new(Graph::default(), picker);
        app.graph.add_vertex(0., 0.);
        app.area = Rect::new(0, 1, 80, 23);
        let shown = app.camera;
        let (scale, area, cell) = (app.device_scale(), app.area, app.picker.font_size());
        if let Some(canvas) = &mut app.kitty {
            canvas.stage_frame(graphics::KittyFrame {
                upload: String::new(),
                camera: shown,
                margin: (0, 0),
                omits: Vec::new(),
                hides: None,
            });
            canvas.stage_view(shown, scale, area, cell);
        }
        app.camera.zoom *= 2.;
        assert_eq!(app.displayed_camera(), shown);
        // Where vertex 1 is drawn on screen, not where the new zoom puts it.
        let (w, h) = app.size();
        let on_screen = shown.world_to_screen(0., 0., w, h);
        let col = (on_screen.0 / app.cell.0) as u16;
        let row = (on_screen.1 / app.cell.1) as u16 + 1;
        app.selection = Selection::default();
        click(&mut app, col, row);
        assert_eq!(app.selection, Selection::vertex(1));
        assert_eq!(app.graph.vertices.len(), 1);
        // Toolbar zoom still changes the real camera meanwhile.
        let zoom = app.camera.zoom;
        let plus = app.toolbar_buttons(80).last().unwrap().start as u16 + 1;
        click(&mut app, plus, 0);
        assert!(app.camera.zoom > zoom, "the zoom click must not be lost");
    }

    #[test]
    fn a_box_selection_moves_by_its_vertices_edges_or_the_area_between() {
        // One world unit is 6 columns by 3 rows; vertex 1 sits at column 40, row 12.
        let mut app = app();
        let b = app.graph.add_vertex(4., 0.);
        let c = app.graph.add_vertex(0., 2.);
        app.graph.connect(1, b);
        let select_all = |app: &mut App| {
            mouse(app, MouseEventKind::Down(MouseButton::Left), 30, 22);
            mouse(app, MouseEventKind::Drag(MouseButton::Left), 75, 3);
            mouse(app, MouseEventKind::Up(MouseButton::Left), 75, 3);
            assert!(app.selection.vertices.len() >= 3);
        };
        let drag = |app: &mut App, from: (u16, u16), to: (u16, u16)| {
            mouse(app, MouseEventKind::Down(MouseButton::Left), from.0, from.1);
            mouse(app, MouseEventKind::Drag(MouseButton::Left), to.0, to.1);
            mouse(app, MouseEventKind::Up(MouseButton::Left), to.0, to.1);
        };
        let xs = |app: &App| app.graph.vertices.iter().map(|v| v.x).collect::<Vec<_>>();
        // By the selected edge between 1 and b.
        select_all(&mut app);
        drag(&mut app, (52, 12), (58, 12));
        assert_eq!(xs(&app), [1., 5., 1.]);
        assert_eq!(app.selection.vertices.len(), 3, "the selection survives");
        // By empty canvas inside the selection's box, then a vertex.
        drag(&mut app, (52, 6), (58, 6));
        assert_eq!(xs(&app), [2., 6., 2.]);
        drag(&mut app, (52, 12), (46, 12));
        assert_eq!(xs(&app), [1., 5., 1.]);
        assert_eq!(app.graph.vertices.len(), 3);
        assert!(app.graph.vertex(c).is_some());
        // A click (no move) inside the box deselects and places nothing.
        click(&mut app, 52, 6);
        assert_eq!(app.graph.vertices.len(), 3);
        assert!(app.selection.is_empty());
        // A click on a selected edge selects just that edge.
        select_all(&mut app);
        click(&mut app, 58, 12);
        assert_eq!(app.selection, Selection::edge(Edge { a: 1, b }));
    }

    #[test]
    fn round_three_gesture_regressions_stay_fixed() {
        // Double-clicking B to label it, with the pen down at 1, adds no edge.
        let mut app = app();
        let b = app.graph.add_vertex(2., 0.);
        let before = app.graph.clone();
        app.edge_start = Some(1);
        click(&mut app, 52, 12);
        click(&mut app, 52, 12);
        assert_eq!(app.editing.as_deref(), Some(b.to_string().as_str()));
        assert_eq!(app.graph, before, "the label gesture took its edge back");
        assert!(!app.history.can_redo());
        app.key(KeyCode::Esc, KeyModifiers::NONE);
        // A key between two clicks makes them two clicks.
        click(&mut app, 52, 12);
        app.key(KeyCode::Esc, KeyModifiers::NONE);
        click(&mut app, 52, 12);
        assert!(app.editing.is_none());
        // Backspace on the untouched multi-label field keeps the labels.
        app.edge_start = None;
        app.selection.vertices = vec![1, b];
        app.edit_label();
        app.key(KeyCode::Backspace, KeyModifiers::NONE);
        click(&mut app, 70, 18);
        assert_eq!(app.graph, before);
        // A press on canvas released off it leaves the pen down.
        app.edge_start = Some(1);
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), 40, 18);
        mouse(&mut app, MouseEventKind::Up(MouseButton::Left), 40, 0);
        assert_eq!(app.edge_start, Some(1));
        // Grabbing a selected vertex carries the selected edge's ends too.
        let c = app.graph.add_vertex(0., 2.);
        let d = app.graph.add_vertex(2., 2.);
        app.graph.connect(c, d);
        app.edge_start = None;
        app.selection = Selection {
            vertices: vec![1],
            edges: vec![Edge { a: c, b: d }],
        };
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), 40, 12);
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), 46, 12);
        mouse(&mut app, MouseEventKind::Up(MouseButton::Left), 46, 12);
        let x = |app: &App, id| app.graph.vertex(id).unwrap().x;
        assert_eq!((x(&app, 1), x(&app, c), x(&app, d)), (1., 1., 3.));
    }
    #[test]
    fn shortcuts_lift_the_pen_like_their_buttons() {
        let mut app = app();
        let b = app.graph.add_vertex(2., 0.);
        app.selection = Selection::vertex(1);
        app.edge_start = Some(1);
        app.key(KeyCode::Char('s'), KeyModifiers::NONE);
        assert_eq!(app.edge_start, None);
        click(&mut app, 52, 12);
        assert!(app.graph.edges.is_empty(), "no edge from a pen `s` lifted");
        assert_eq!(app.edge_start, Some(b));
    }
    #[test]
    fn quitting_asks_while_redo_holds_an_uncopied_drawing() {
        let mut app = app();
        app.graph = Graph::default();
        app.edge_start = None;
        click(&mut app, 40, 12);
        app.step_history(false);
        assert!(app.graph.vertices.is_empty());
        assert!(!app.key(KeyCode::Char('q'), KeyModifiers::NONE));
    }

    #[test]
    fn a_label_double_click_restores_history_exactly() {
        let mut app = app();
        let b = app.graph.add_vertex(2., 0.);
        // A Redo step, and Undo at its cap: neither may be lost.
        app.history.checkpoint(&app.graph);
        app.history.undo(&mut app.graph);
        let redo_graph = app.graph.clone();
        for _ in 0..100 {
            let g = app.graph.clone();
            app.history.checkpoint(&g);
        }
        // That checkpoint loop cleared Redo; make one again.
        app.history.redo(&mut app.graph);
        app.history.undo(&mut app.graph);
        assert!(app.history.can_redo());
        let before = app.graph.clone();
        let depth = app.history.depth();
        app.edge_start = Some(1);
        click(&mut app, 52, 12);
        click(&mut app, 52, 12);
        assert!(app.editing.is_some());
        assert_eq!(app.graph, before, "no edge, even at the undo cap");
        assert_eq!(app.history.depth(), depth);
        assert!(app.history.can_redo(), "Redo survives the label gesture");
        let _ = (b, redo_graph);
        // Right-click between two clicks makes them two clicks.
        app.key(KeyCode::Esc, KeyModifiers::NONE);
        click(&mut app, 52, 12);
        mouse(&mut app, MouseEventKind::Down(MouseButton::Right), 40, 18);
        mouse(&mut app, MouseEventKind::Up(MouseButton::Right), 40, 18);
        click(&mut app, 52, 12);
        assert!(app.editing.is_none());
    }
    #[test]
    fn quit_asks_if_either_side_of_history_holds_uncopied_work() {
        let mut app = app();
        // Undo holds an uncopied drawing (vertex 1), Redo a copied one.
        let uncopied = app.graph.clone();
        app.history.checkpoint(&uncopied);
        app.graph = Graph::default();
        app.history.checkpoint(&app.graph);
        app.graph = uncopied.clone();
        app.graph.add_vertex(2., 0.);
        app.delivered = Some(app.graph.clone());
        app.history.undo(&mut app.graph);
        assert!(app.graph.vertices.is_empty() && app.history.can_redo());
        assert!(!app.key(KeyCode::Char('q'), KeyModifiers::NONE));
    }
    #[test]
    fn a_shortcut_that_cannot_apply_leaves_the_pen_down() {
        let mut app = app();
        app.selection = Selection::vertex(1);
        app.edge_start = Some(1);
        app.key(KeyCode::Char('c'), KeyModifiers::NONE); // one vertex: nothing to connect
        assert_eq!(app.edge_start, Some(1));
    }
}
