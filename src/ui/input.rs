use super::*;

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
        let world = self.world(p);
        self.graph
            .vertices
            .iter()
            .rev()
            .find(|v| {
                let q = self.camera.world_to_screen(v.x, v.y, w, h);
                (v.x == world.0.round() && v.y == world.1.round())
                    || (q.0 - p.0).hypot(q.1 - p.1) <= 16_f64.min(self.camera.zoom * 0.45)
            })
            .map(|v| v.id)
    }
    pub(super) fn mouse(&mut self, m: MouseEvent) {
        if self.editing.is_some() || self.path_prompt.is_some() {
            return;
        }
        if m.kind != MouseEventKind::Moved {
            self.quit_pending = false;
        }
        let p = self.local(m.column, m.row);
        self.hover = p.map(|p| {
            let q = self.world(p);
            (q.0.round(), q.1.round())
        });
        if m.kind == MouseEventKind::Down(MouseButton::Left) && m.row == 0 {
            self.toolbar(m.column, m.row);
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
                    if let Some(id) = self.hit(p) {
                        self.selected = Some(id);
                        let q = self.world(p);
                        let v = self.graph.vertex(id).unwrap();
                        self.drag = Some(Drag::Vertex {
                            id,
                            original: self.graph.clone(),
                            offset: (v.x - q.0, v.y - q.1),
                            moved: false,
                        });
                    } else {
                        self.edge_start = None;
                        self.last_click = None;
                        let q = self.world(p);
                        let (x, y) = (q.0.round(), q.1.round());
                        if x.is_finite()
                            && y.is_finite()
                            && self.graph.next_id < u64::MAX - 1
                            && !self.graph.vertices.iter().any(|v| v.x == x && v.y == y)
                        {
                            self.history.checkpoint(&self.graph);
                            self.selected = Some(self.graph.add_vertex(x, y));
                        }
                    }
                }
            }
            MouseEventKind::Drag(button) => {
                if let Some(p) = p {
                    let q = self.world(p);
                    match &mut self.drag {
                        Some(Drag::Vertex {
                            id,
                            original,
                            offset,
                            moved,
                        }) if button == MouseButton::Left => {
                            let (x, y) = (q.0 + offset.0, q.1 + offset.1);
                            let before = original.vertex(*id).unwrap();
                            // Ignore tiny mouse noise; retain the grab offset so
                            // the node never jumps to the center of a text cell.
                            if *moved || (x - before.x).hypot(y - before.y) * self.camera.zoom > 4.
                            {
                                *moved = true;
                                self.graph.move_vertex(*id, x, y);
                                self.edge_start = None;
                                self.last_click = None;
                            }
                        }
                        Some(Drag::Pan(x, y)) if button != MouseButton::Left => {
                            self.camera.x -= (p.0 - *x) / self.camera.zoom;
                            self.camera.y += (p.1 - *y) / self.camera.zoom;
                            *x = p.0;
                            *y = p.1;
                        }
                        _ => {}
                    }
                }
            }
            MouseEventKind::Up(button) => {
                // A different button's release must not finish the active gesture.
                let finishes = matches!(
                    (&self.drag, button),
                    (Some(Drag::Vertex { .. }), MouseButton::Left)
                        | (
                            Some(Drag::Pan(..)),
                            MouseButton::Middle | MouseButton::Right
                        )
                );
                if finishes {
                    if let Some(Drag::Vertex {
                        id,
                        original,
                        moved,
                        offset,
                    }) = self.drag.take()
                    {
                        if moved {
                            let target = p
                                .map(|p| {
                                    let q = self.world(p);
                                    ((q.0 + offset.0).round(), (q.1 + offset.1).round())
                                })
                                .or_else(|| {
                                    self.graph.vertex(id).map(|v| (v.x.round(), v.y.round()))
                                });
                            if let Some((x, y)) = target {
                                if !self.graph.move_vertex(id, x, y) {
                                    self.graph = original.clone();
                                }
                            }
                            if self.graph != original {
                                self.history.checkpoint(&original);
                            }
                        } else if p.and_then(|p| self.hit(p)) == Some(id) {
                            self.click_vertex(id);
                        }
                    }
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
                            .zoom_at(if up { 1.2 } else { 1. / 1.2 }, p.0, p.1, w, h);
                    } else {
                        self.camera.y += if up { 0.7 } else { -0.7 };
                    }
                }
            }
            MouseEventKind::ScrollLeft => self.camera.x -= 0.7,
            MouseEventKind::ScrollRight => self.camera.x += 0.7,
            _ => {}
        }
        // Panning/zooming changes which cell is beneath a stationary pointer.
        self.hover = p.map(|p| {
            let q = self.world(p);
            (q.0.round(), q.1.round())
        });
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
            v.style = match v.style {
                VertexStyle::Dots => VertexStyle::Circles,
                VertexStyle::Circles => VertexStyle::Plain,
                VertexStyle::Plain => VertexStyle::Dots,
            };
        }
    }
    pub(super) fn edit_label(&mut self) {
        if let Some(v) = self.selected.and_then(|id| self.graph.vertex(id)) {
            self.editing = Some(v.label.clone());
        }
    }
    pub(super) fn delete_selected(&mut self) {
        if let Some(id) = self.selected.take() {
            self.history.checkpoint(&self.graph);
            self.graph.remove_vertex(id);
            self.edge_start = None;
        }
    }
    pub(super) fn toolbar(&mut self, col: u16, row: u16) {
        use super::view::ToolbarAction;
        self.status.clear();
        match super::view::toolbar_action(col, row, self.selected.is_some()) {
            Some(ToolbarAction::Undo) => {
                self.history.undo(&mut self.graph);
            }
            Some(ToolbarAction::Redo) => {
                self.history.redo(&mut self.graph);
            }
            Some(ToolbarAction::Fit) => {
                let (w, h) = self.size();
                self.camera.fit(&self.graph, w, h);
            }
            Some(ToolbarAction::Export) => self.export(),
            Some(ToolbarAction::StyleAll) => self.style_all(),
            Some(ToolbarAction::Copy) => self.copy(),
            Some(ToolbarAction::Label) => self.edit_label(),
            Some(ToolbarAction::Style) => {
                if let Some(id) = self.selected {
                    self.cycle_style(id);
                }
            }
            Some(ToolbarAction::Delete) => self.delete_selected(),
            Some(ToolbarAction::Zoom(factor)) => {
                let (w, h) = self.size();
                self.camera.zoom_at(factor, w / 2., h / 2., w, h);
            }
            None => {}
        }
        self.edge_start = None;
        self.selected = self.selected.filter(|id| self.graph.vertex(*id).is_some());
        self.last_click = None;
    }
    pub(super) fn style_all(&mut self) {
        if let Some(first) = self.graph.vertices.first() {
            let next = match first.style {
                VertexStyle::Dots => VertexStyle::Circles,
                VertexStyle::Circles => VertexStyle::Plain,
                VertexStyle::Plain => VertexStyle::Dots,
            };
            self.history.checkpoint(&self.graph);
            for v in &mut self.graph.vertices {
                v.style = next;
            }
        }
    }
    pub(super) fn key(&mut self, code: KeyCode, mods: KeyModifiers) -> bool {
        if self.path_prompt.is_some() {
            self.path_key(code, mods);
            return false;
        }
        if let Some(s) = &mut self.editing {
            match code {
                KeyCode::Esc => self.editing = None,
                KeyCode::Enter => {
                    let value = self.editing.take().unwrap();
                    if let Some(id) = self.selected {
                        if self.graph.vertex(id).is_some_and(|v| v.label != value) {
                            self.history.checkpoint(&self.graph);
                            if let Some(v) = self.graph.vertex_mut(id) {
                                v.label = value;
                            }
                        }
                    }
                }
                KeyCode::Backspace => {
                    s.pop();
                }
                KeyCode::Char(c) if !mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                    s.push(c)
                }
                _ => {}
            }
            return false;
        }
        if code == KeyCode::Esc {
            if let Some(drag) = self.drag.take() {
                if let Drag::Vertex { original, .. } = drag {
                    self.graph = original;
                }
                self.edge_start = None;
                return false;
            }
            if self.edge_start.take().is_some() {
                return false;
            }
        }
        if !matches!(code, KeyCode::Char('q') | KeyCode::Esc) {
            self.quit_pending = false;
        }
        match (code, mods) {
            (KeyCode::Char('q'), _) | (KeyCode::Esc, _) => {
                if self.graph.vertices.is_empty()
                    || self.delivered.as_ref() == Some(&self.graph)
                    || self.quit_pending
                {
                    return true;
                }
                self.quit_pending = true;
                self.status = "Unsaved drawing. Q discards.".into();
                return false;
            }
            (KeyCode::Char('s'), m) if m.contains(KeyModifiers::CONTROL) => self.export(),
            (KeyCode::Char('c'), m) if m.contains(KeyModifiers::CONTROL) => self.copy(),
            (KeyCode::Char('e'), m) if m.contains(KeyModifiers::CONTROL) => self.export(),
            (KeyCode::Char('z'), m)
                if m.contains(KeyModifiers::CONTROL) && m.contains(KeyModifiers::SHIFT) =>
            {
                self.history.redo(&mut self.graph);
            }
            (KeyCode::Char('z'), m) if m.contains(KeyModifiers::CONTROL) => {
                self.history.undo(&mut self.graph);
            }
            (KeyCode::Char('y'), m) if m.contains(KeyModifiers::CONTROL) => {
                self.history.redo(&mut self.graph);
            }
            (KeyCode::Enter, _) => self.edit_label(),
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
        self.selected = self.selected.filter(|id| self.graph.vertex(*id).is_some());
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
    fn drag_previews_between_grid_points_and_collision_rolls_back() {
        let mut app = app();
        app.graph.add_vertex(2., 0.);
        let original = app.graph.clone();
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), 40, 12);
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), 42, 12);
        assert!(app.graph.vertices[0].x > 0. && app.graph.vertices[0].x < 1.);
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
    fn escape_cancels_pending_edge_and_quit_requires_confirmation() {
        let mut app = app();
        app.edge_start = Some(1);
        assert!(!app.key(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.edge_start.is_none());
        app.graph.add_vertex(2., 0.);
        assert!(!app.key(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(app.key(KeyCode::Char('q'), KeyModifiers::NONE));
    }
}
