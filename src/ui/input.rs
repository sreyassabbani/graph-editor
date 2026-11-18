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
    pub(super) fn hit(&self, p: (f64, f64)) -> Option<u64> {
        let (w, h) = self.size();
        self.graph
            .vertices
            .iter()
            .rev()
            .find(|v| {
                let q = self.camera.world_to_screen(v.x, v.y, w, h);
                (q.0 - p.0).hypot(q.1 - p.1) <= 16_f64.min(self.camera.zoom * 0.45)
            })
            .map(|v| v.id)
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
        if self.drag.is_some() || self.hit(p).is_some() || self.hit_edge(p).is_some() {
            return None;
        }
        let q = self.world(p);
        let g = (q.0.round(), q.1.round());
        (!self.graph.vertices.iter().any(|v| v.x == g.0 && v.y == g.1)).then_some(g)
    }
    fn update_hover(&mut self, p: Option<(f64, f64)>) {
        self.hover = p.and_then(|p| self.placement(p));
    }
    pub(super) fn mouse(&mut self, m: MouseEvent) {
        if self.editing.is_some() {
            // Clicking anywhere saves the label, then acts as a normal click.
            if !matches!(m.kind, MouseEventKind::Down(_)) {
                return;
            }
            self.commit_editing();
        }
        if m.kind != MouseEventKind::Moved {
            self.quit_pending = false;
        }
        let (col, row, p) = self.pointer(&m);
        self.toolbar_hover = (row == 0 && p.is_none()).then_some(col);
        self.update_hover(p);
        if m.kind == MouseEventKind::Down(MouseButton::Left) && row == 0 {
            self.toolbar(col, row);
            return;
        }
        match m.kind {
            MouseEventKind::Down(MouseButton::Middle | MouseButton::Right) => {
                if let Some(p) = p {
                    self.drag = Some(Drag::Pan(p.0, p.1));
                    self.edge_start = None;
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(p) = p {
                    self.status.clear();
                    self.left_down(p, m.modifiers);
                }
            }
            MouseEventKind::Drag(button) => {
                if let Some(p) = p {
                    if button == MouseButton::Left {
                        self.left_drag(p);
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
                        self.left_up(p)
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
                        self.camera
                            .zoom_at(if up { ZOOM_STEP } else { 1. / ZOOM_STEP }, p.0, p.1, w, h);
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
        self.update_hover(p);
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
        if let Some(id) = self.hit(p) {
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
                ids: if narrow {
                    self.selection.vertices.clone()
                } else {
                    vec![id]
                },
                original: self.graph.clone(),
                offset: (v.x - q.0, v.y - q.1),
                moved: false,
                narrow,
            });
        } else if let Some(edge) = self.hit_edge(p) {
            if additive {
                self.selection.toggle_edge(edge);
            } else {
                self.selection = Selection::edge(edge);
            }
            self.edge_start = None;
            self.last_click = None;
            self.drag = None;
        } else {
            self.drag = Some(Drag::Marquee {
                start: p,
                end: p,
                additive,
            });
        }
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
                    // Preview the snapped drop position: what you see is where
                    // it lands, and a new frame is needed only per grid step.
                    let (sx, sy) = (
                        (q.0 + offset.0).round() - before.x,
                        (q.1 + offset.1).round() - before.y,
                    );
                    let mut preview = original.clone();
                    if preview.translate(ids, sx, sy) && preview != self.graph {
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
                } else if p.and_then(|p| self.hit(p)) == Some(anchor) {
                    if narrow {
                        self.selection = Selection::vertex(anchor);
                    }
                    self.click_vertex(anchor);
                }
            }
            Some(Drag::Marquee {
                start,
                end,
                additive,
            }) => {
                self.edge_start = None;
                self.last_click = None;
                if (end.0 - start.0).hypot(end.1 - start.1) < 4. {
                    // A click on empty canvas places a vertex.
                    if !additive {
                        self.selection = Selection::default();
                        self.add_vertex_at(start);
                    }
                    return;
                }
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
    fn add_vertex_at(&mut self, p: (f64, f64)) {
        let q = self.world(p);
        let (x, y) = (q.0.round(), q.1.round());
        if x.is_finite()
            && y.is_finite()
            && self.graph.next_id < u64::MAX - 1
            && !self.graph.vertices.iter().any(|v| v.x == x && v.y == y)
        {
            self.history.checkpoint(&self.graph);
            let id = self.graph.add_styled_vertex(x, y, self.new_style, !self.new_unlabeled);
            self.selection = Selection::vertex(id);
        }
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
        let ids = self.selection.vertices.clone();
        let mut missing = Vec::new();
        for (i, &a) in ids.iter().enumerate() {
            for &b in &ids[i + 1..] {
                let present = self
                    .graph
                    .edges
                    .iter()
                    .any(|e| (e.a == a && e.b == b) || (e.a == b && e.b == a));
                if !present {
                    missing.push((a, b));
                }
            }
        }
        if missing.is_empty() {
            return;
        }
        self.history.checkpoint(&self.graph);
        for (a, b) in missing {
            self.graph.connect(a, b);
        }
    }
    fn click_vertex(&mut self, id: u64) {
        if self
            .last_click
            .is_some_and(|(old, t)| old == id && t.elapsed() < Duration::from_millis(400))
        {
            self.cycle_style(id);
            self.edge_start = None;
            self.last_click = None;
        } else {
            self.last_click = Some((id, Instant::now()));
            match self.edge_start.take() {
                Some(start) if start != id => self.connect(start, id),
                Some(_) => {}
                None => {
                    self.edge_start = Some(id);
                }
            }
        }
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
    pub(super) fn cycle_style(&mut self, id: u64) {
        self.history.checkpoint(&self.graph);
        if let Some(v) = self.graph.vertex_mut(id) {
            v.style = v.style.next();
            self.new_style = v.style;
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
    /// One vertex starts from its label; several start empty and Enter gives
    /// them all the same label (an empty one unlabels them).
    pub(super) fn edit_label(&mut self) {
        match self.selection.vertices.as_slice() {
            [] => {}
            [id] => {
                if let Some(v) = self.graph.vertex(*id) {
                    self.editing = Some(v.label.clone());
                }
            }
            _ => self.editing = Some(String::new()),
        }
    }
    fn commit_editing(&mut self) {
        if let Some(value) = self.editing.take() {
            self.commit_label(value);
        }
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
    pub(super) fn toolbar(&mut self, col: u16, row: u16) {
        use super::view::ToolbarAction;
        self.status.clear();
        match self.toolbar_action(col, row) {
            Some(ToolbarAction::Undo) => {
                self.step_history(false);
            }
            Some(ToolbarAction::Redo) => {
                self.step_history(true);
            }
            Some(ToolbarAction::Fit) => {
                let (w, h) = self.size();
                self.camera.fit(&self.graph, w, h);
            }
            Some(ToolbarAction::Copy) => self.copy(),
            Some(ToolbarAction::Label) => self.edit_label(),
            Some(ToolbarAction::Relabel) => self.restart_labels(),
            Some(ToolbarAction::Style) => self.style_selection(),
            Some(ToolbarAction::Connect) => self.connect_selection(),
            Some(ToolbarAction::Delete) => self.delete_selected(),
            Some(ToolbarAction::Zoom(factor)) => {
                let (w, h) = self.size();
                self.camera.zoom_at(factor, w / 2., h / 2., w, h);
            }
            None => {}
        }
        self.edge_start = None;
        self.reconcile_selection();
        self.last_click = None;
    }
    pub(super) fn key(&mut self, code: KeyCode, mods: KeyModifiers) -> bool {
        if let Some(s) = &mut self.editing {
            match code {
                KeyCode::Esc => self.editing = None,
                KeyCode::Enter => self.commit_editing(),
                KeyCode::Backspace => {
                    s.pop();
                }
                KeyCode::Char(c)
                    if !mods.intersects(
                        KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                    ) =>
                {
                    s.push(c)
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
                if self.graph.vertices.is_empty()
                    || self.delivered.as_ref() == Some(&self.graph)
                    || self.quit_pending
                {
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
            (KeyCode::Enter, _) => self.edit_label(),
            (KeyCode::Char('a'), m) if m.is_empty() || m.contains(KeyModifiers::CONTROL) => {
                self.select_all()
            }
            (KeyCode::Char('s'), m) if m.is_empty() => self.style_selection(),
            (KeyCode::Char('R'), m) if m.is_empty() || m == KeyModifiers::SHIFT => {
                self.restart_labels()
            }
            (KeyCode::Char('c'), m) if m.is_empty() => self.connect_selection(),
            (KeyCode::Delete | KeyCode::Backspace, _) => self.delete_selected(),
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
        let labels: Vec<_> = app.graph.vertices.iter().map(|v| v.label.as_str()).collect();
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
        assert_eq!(app.edge_start, Some(1));
    }
    #[test]
    fn button_release_preserves_operation_errors() {
        let mut app = app();
        app.status = "Copy failed".into();
        mouse(&mut app, MouseEventKind::Up(MouseButton::Left), 2, 0);
        assert_eq!(app.status, "Copy failed");
    }
    #[test]
    fn hover_add_and_connect_work_without_modes() {
        let mut app = app();
        let original = app.graph.clone();
        mouse(&mut app, MouseEventKind::Moved, 52, 12);
        assert_eq!(app.hover, Some((2., 0.)));
        assert_eq!(app.graph, original);
        click(&mut app, 52, 12);
        assert_eq!(app.graph.vertices.len(), 2);
        assert!(app.graph.edges.is_empty());
        assert!(app.edge_start.is_none());
        click(&mut app, 40, 12);
        assert_eq!(app.edge_start, Some(1));
        click(&mut app, 52, 12);
        assert_eq!(app.graph.edges.len(), 1);
        assert!(app.edge_start.is_none());
        assert!(app.history.undo(&mut app.graph));
        assert!(app.graph.edges.is_empty());
        assert_eq!(app.graph.vertices.len(), 2);
        app.last_click = None;
        click(&mut app, 40, 12);
        click(&mut app, 52, 12);
        app.last_click = None;
        click(&mut app, 40, 12);
        click(&mut app, 52, 12);
        assert_eq!(app.graph.edges.len(), 1); // no duplicate connections
    }
    #[test]
    fn drag_preview_snaps_to_grid_points_and_collision_rolls_back() {
        let mut app = app();
        app.graph.add_vertex(2., 0.);
        let original = app.graph.clone();
        // One world unit is 6 columns: a third of a unit stays put, two
        // thirds previews the next grid point.
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), 40, 12);
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), 42, 12);
        assert_eq!(app.graph.vertices[0].x, 0.);
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), 44, 12);
        assert_eq!(app.graph.vertices[0].x, 1.);
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
    fn double_click_cycles_style_and_right_drag_only_pans() {
        let mut app = app();
        click(&mut app, 40, 12);
        click(&mut app, 40, 12);
        assert_eq!(app.graph.vertices[0].style, VertexStyle::Circles);
        assert!(app.edge_start.is_none());
        app.last_click = None;
        click(&mut app, 52, 12); // new vertex inherits the last chosen style
        assert_eq!(app.graph.vertices.len(), 2);
        assert_eq!(app.graph.vertices[1].style, VertexStyle::Circles);
        app.graph.vertices.pop();
        app.selection = Selection::default();
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
    fn clicking_while_editing_saves_the_label_then_acts() {
        let mut app = app();
        app.selection = Selection::vertex(1);
        app.edit_label();
        app.key(KeyCode::Char('x'), KeyModifiers::NONE);
        // The canvas previews the typed label before it is saved.
        assert_eq!(app.scene().state.graph.vertices[0].label, "1x");
        assert_eq!(app.graph.vertices[0].label, "1");
        click(&mut app, 52, 12);
        assert!(app.editing.is_none());
        assert_eq!(app.graph.vertices[0].label, "1x");
        assert_eq!(app.graph.vertices.len(), 2, "the click also placed a vertex");
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
}
