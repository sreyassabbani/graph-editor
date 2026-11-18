use super::*;

/// Logical pixels rendered beyond each canvas edge for crop-only panning.
const PAN_MARGIN: f64 = 64.;
/// How long the pen's ring takes to unroll into a new edge and wind up again
/// at its other end.
const UNROLL: Duration = Duration::from_millis(480);
/// The pen ring turns steadily, one dash per `SPIN_DASH`; it takes `SPIN_UP`
/// to reach that speed and `SPIN_DOWN` to come to rest.
const SPIN_DASH: Duration = Duration::from_millis(1400);
const SPIN_UP: Duration = Duration::from_millis(900);
const SPIN_DOWN: Duration = Duration::from_millis(1600);
/// The pen ring rests after this long without input.
const SPIN_IDLE: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum ToolbarAction {
    Undo,
    Redo,
    Fit,
    Copy,
    Label,
    Relabel,
    Style,
    Connect,
    Delete,
    Zoom(f64),
}

/// A toolbar button at its starting column. Disabled buttons stay in place
/// (dimmed, inert) so the row does not shift under the pointer.
#[derive(Clone, Copy, Debug)]
pub(super) struct Button {
    pub start: usize,
    pub text: &'static str,
    pub action: ToolbarAction,
    pub enabled: bool,
}

impl App {
    /// Every button, in fixed places, dimmed when it would do nothing. Zoom
    /// sits flush right (or straight after the menu on a narrow terminal).
    pub(super) fn toolbar_buttons(&self, width: u16) -> Vec<Button> {
        use ToolbarAction::*;
        let vertices = &self.selection.vertices;
        let has_graph = !self.graph.vertices.is_empty();
        let mut items = vec![
            (" Copy ", Copy, has_graph),
            (" Undo ", Undo, self.history.can_undo()),
            (" Redo ", Redo, self.history.can_redo()),
            (" Fit ", Fit, has_graph),
        ];
        // Every button keeps its place; ones that do not apply are dimmed.
        // A row that rearranged itself would put a different action under
        // the pointer between two quick clicks (Connect becoming Delete).
        items.extend([
            (" Relabel ", Relabel, !self.graph.labels_in_order()),
            (" Label ", Label, !vertices.is_empty()),
            (" Style ", Style, !vertices.is_empty()),
            (" Connect ", Connect, self.missing_selection_edges() > 0),
            (" Delete ", Delete, !self.selection.is_empty()),
        ]);
        // A narrow pane drops the padding so zoom still fits beside the menu.
        let full: usize = items.iter().map(|(t, ..)| t.len()).sum::<usize>() + 6;
        let compact = usize::from(width) < full;
        let tight = |text: &'static str| if compact { &text[1..] } else { text };
        let mut buttons = Vec::new();
        let mut col = 0;
        for (text, action, enabled) in items {
            let text = tight(text);
            buttons.push(Button {
                start: col,
                text,
                action,
                enabled,
            });
            col += text.len();
        }
        let zoom = [(tight(" - "), Zoom(1. / 1.2)), (tight(" + "), Zoom(1.2))];
        let zoom_width: usize = zoom.iter().map(|(text, _)| text.len()).sum();
        col = (width as usize).saturating_sub(zoom_width).max(col);
        for (text, action) in zoom {
            buttons.push(Button {
                start: col,
                text,
                action,
                enabled: true,
            });
            col += text.len();
        }
        buttons
    }
    /// The enabled button under a toolbar click.
    pub(super) fn toolbar_action(&self, col: u16, row: u16) -> Option<ToolbarAction> {
        if row != 0 {
            return None;
        }
        self.toolbar_buttons(self.area.width)
            .into_iter()
            .find(|b| (b.start..b.start + b.text.len()).contains(&(col as usize)))
            .filter(|b| b.enabled)
            .map(|b| b.action)
    }
    /// Pairs of selected vertices not yet joined.
    pub(super) fn missing_selection_edges(&self) -> usize {
        let ids = &self.selection.vertices;
        if ids.len() < 2 {
            return 0;
        }
        let present: std::collections::HashSet<_> = self
            .graph
            .edges
            .iter()
            .map(|e| (e.a.min(e.b), e.a.max(e.b)))
            .collect();
        let mut missing = 0;
        for (i, &a) in ids.iter().enumerate() {
            for &b in &ids[i + 1..] {
                missing += usize::from(!present.contains(&(a.min(b), a.max(b))));
            }
        }
        missing
    }
}

impl App {
    /// Device pixels per logical pixel.
    pub(super) fn device_scale(&self) -> (f64, f64) {
        let physical = self.picker.font_size();
        (
            physical.0 as f64 / self.cell.0,
            physical.1 as f64 / self.cell.1,
        )
    }
    /// Device pixels rendered beyond each canvas edge, so a pan of up to half
    /// this needs no new frame. Only Kitty can crop a placement.
    fn pan_margin(&self) -> (u32, u32) {
        if self.kitty.is_none() {
            return (0, 0);
        }
        let scale = self.device_scale();
        (
            (PAN_MARGIN * scale.0).round() as u32,
            (PAN_MARGIN * scale.1).round() as u32,
        )
    }
    /// Whether the frame rendered for `rendered` still covers the current
    /// camera by cropping, with room to spare for the next pan step.
    fn pan_covered(&self, rendered: Camera) -> bool {
        let margin = self.pan_margin();
        let scale = self.device_scale();
        rendered.zoom == self.camera.zoom
            && ((rendered.x - self.camera.x) * self.camera.zoom * scale.0).abs()
                <= f64::from(margin.0) / 2.
            && ((rendered.y - self.camera.y) * self.camera.zoom * scale.1).abs()
                <= f64::from(margin.1) / 2.
    }
    pub(super) fn scene(&self) -> canvas::Scene {
        let margin = self.pan_margin();
        let scale = self.device_scale();
        let (pw, ph) = self.pixel_size();
        let (w, h) = self.size();
        let kitty = self.kitty.is_some();
        // While vertices are dragged or relabeled on Kitty, the frame holds
        // the still part and stays the same while the pointer moves or keys
        // are typed; `moving_scene` draws the rest as tiles.
        let (mut graph, layer) = match self.moving() {
            Some((ids, still)) if kitty => (still, Layer::Still(ids)),
            _ => (self.preview_graph(), Layer::All),
        };
        // An edge the pen is unrolling into is drawn by the pen layer until
        // the animation ends; then this frame gets it.
        let hide_edge = match (kitty, &layer, self.unroll) {
            (true, Layer::All, Some((a, b, _))) => graph
                .edges
                .iter()
                .find(|e| (e.a, e.b) == (a, b) || (e.a, e.b) == (b, a))
                .copied(),
            _ => None,
        };
        if let Some(edge) = hide_edge {
            graph.edges.retain(|e| *e != edge);
        }
        canvas::Scene {
            margin,
            state: CanvasState {
                graph,
                camera: self.camera,
                // Kitty draws selection, the pen ring and the hovered edge as
                // decor tiles over the frame, so clicks do not redraw it.
                selection: if kitty {
                    Selection::default()
                } else {
                    self.selection.clone()
                },
                edge_start: if kitty { None } else { self.edge_start },
                // Kitty shows the marquee as cropped overlay placements.
                marquee: if kitty { None } else { self.marquee() },
                // Kitty presents hover separately; fallback protocols bake it in.
                hover: if kitty { None } else { self.hover },
                hover_edge: if kitty { None } else { self.hover_edge },
                size: (pw + 2 * margin.0, ph + 2 * margin.1),
                layer,
                spin: 0,
                unroll: None,
                hide_edge,
            },
            logical_size: (
                w + 2. * f64::from(margin.0) / scale.0,
                h + 2. * f64::from(margin.1) / scale.1,
            ),
        }
    }

    /// The vertices being changed continuously (dragged, or relabeled while
    /// typing) and the graph the still frame is drawn from.
    fn moving(&self) -> Option<(Vec<u64>, Graph)> {
        match &self.drag {
            Some(Drag::Vertices {
                ids,
                original,
                moved: true,
                ..
            }) => Some((ids.clone(), original.clone())),
            _ if self.editing.is_some() && !self.selection.vertices.is_empty() => {
                Some((self.selection.vertices.clone(), self.graph.clone()))
            }
            _ => None,
        }
    }

    /// A tile layer over the visible canvas (no pan margin).
    fn overlay_scene(&self, graph: Graph, layer: Layer) -> canvas::Scene {
        canvas::Scene {
            margin: (0, 0),
            state: CanvasState {
                graph,
                camera: self.camera,
                selection: self.selection.clone(),
                edge_start: None,
                marquee: None,
                hover: None,
                hover_edge: None,
                size: self.pixel_size(),
                layer,
                spin: 0,
                unroll: None,
                hide_edge: None,
            },
            logical_size: self.size(),
        }
    }

    /// Every vertex the shown Kitty frame leaves out, drawn where it is now,
    /// with its edges: those being dragged or relabeled, and any an older
    /// partial frame still on screen omits (until the full frame arrives).
    fn moving_scene(&self) -> Option<canvas::Scene> {
        let omitted = self.kitty.as_ref()?.frame_omits();
        if omitted.is_empty() {
            return None;
        }
        let mut ids = omitted.to_vec();
        for id in self.moving().map(|(ids, _)| ids).unwrap_or_default() {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        let mut scene = self.overlay_scene(self.preview_graph(), Layer::Moving(ids));
        scene.state.camera = self.displayed_camera();
        Some(scene)
    }

    /// Selection highlights and the hovered edge, drawn over the shown frame with its camera so they line up even while a new zoom
    /// renders. `None` when there are none, without a frame, or while moving
    /// tiles are up (they carry the selection's colors themselves).
    fn decor_scene(&self) -> Option<canvas::Scene> {
        let canvas = self.kitty.as_ref()?;
        if !canvas.frame_omits().is_empty() || canvas.shown_camera(self.device_scale()).is_none() {
            return None;
        }
        if self.selection.is_empty() && self.hover_edge.is_none() {
            return None;
        }
        let mut scene = self.overlay_scene(self.graph.clone(), Layer::Decor);
        scene.state.camera = self.displayed_camera();
        scene.state.hover_edge = self.hover_edge;
        Some(scene)
    }

    /// The pen's rotating ring, the edge it is unrolling into, and an edge
    /// the shown frame still leaves out, drawn behind the frame with its
    /// camera. `None` without a frame or while moving tiles are up.
    fn pen_scene(&self) -> Option<canvas::Scene> {
        let canvas = self.kitty.as_ref()?;
        if !canvas.frame_omits().is_empty() || canvas.shown_camera(self.device_scale()).is_none() {
            return None;
        }
        let has = |a: u64, b: u64| {
            self.graph
                .edges
                .iter()
                .any(|e| (e.a, e.b) == (a, b) || (e.a, e.b) == (b, a))
        };
        let unroll = self.unroll.and_then(|(a, b, at)| {
            let t = at.elapsed().as_secs_f64() / UNROLL.as_secs_f64();
            (t < 1. && has(a, b)).then_some((a, b, (t * 1000.) as u16))
        });
        let hide_edge = canvas
            .frame_hides()
            .filter(|e| self.graph.edges.contains(e));
        if self.edge_start.is_none() && unroll.is_none() && hide_edge.is_none() {
            return None;
        }
        let mut scene = self.overlay_scene(self.graph.clone(), Layer::Pen);
        scene.state.selection = Selection::default();
        scene.state.camera = self.displayed_camera();
        scene.state.edge_start = self.edge_start;
        scene.state.spin = self.spin_phase();
        scene.state.unroll = unroll;
        scene.state.hide_edge = hide_edge;
        Some(scene)
    }

    /// Advance the pen ring. It eases up to a slow, steady turn when the pen
    /// goes down, keeps that speed as the pen moves from vertex to vertex,
    /// and eases to rest after [`SPIN_IDLE`] without input (so an idle
    /// editor stops redrawing), spinning up again on the next input.
    fn advance_spin(&mut self) {
        let now = Instant::now();
        // A long sleep while resting must not jump the ring on waking.
        let dt = now.duration_since(self.spin_at).as_secs_f64().min(0.1);
        self.spin_at = now;
        if self.edge_start.is_none() || self.kitty.is_none() {
            self.spin_ramp = 0.;
            return;
        }
        self.spin_ramp = if self.last_input.elapsed() < SPIN_IDLE {
            (self.spin_ramp + dt / SPIN_UP.as_secs_f64()).min(1.)
        } else {
            (self.spin_ramp - dt / SPIN_DOWN.as_secs_f64()).max(0.)
        };
        // Speed follows a smoothstep of the ramp: no jolt starting or stopping.
        let r = self.spin_ramp;
        let speed = r * r * (3. - 2. * r) / SPIN_DASH.as_secs_f64();
        self.spin_pos = (self.spin_pos + speed * dt).fract();
    }

    /// The pen ring's turn now, in thousandths of a dash.
    fn spin_phase(&self) -> u16 {
        (self.spin_pos * f64::from(canvas::SPIN_STEPS)) as u16 % canvas::SPIN_STEPS
    }

    /// The pen ring is turning, or about to.
    fn spinning(&self) -> bool {
        self.kitty.is_some()
            && self.edge_start.is_some()
            && (self.spin_ramp > 0. || self.last_input.elapsed() < SPIN_IDLE)
    }

    /// Advance animations; true if one needs a redraw now. An unroll that
    /// has finished lets the next frame draw its edge.
    pub(super) fn animate(&mut self) -> bool {
        self.advance_spin();
        if let Some((.., at)) = self.unroll {
            if self.kitty.is_none() {
                self.unroll = None;
            } else {
                if at.elapsed() >= UNROLL {
                    self.unroll = None;
                    self.scene_dirty = true;
                }
                return true;
            }
        }
        self.spinning()
            && self
                .pen_state
                .as_ref()
                .is_some_and(|s| s.spin != self.spin_phase())
    }

    /// How long the event loop may sleep before the next animation step.
    pub(super) fn animation_wait(&self) -> Option<Duration> {
        if self.unroll.is_some() {
            return Some(Duration::from_millis(8));
        }
        // About 40 steps a second while it turns; nothing once it rests or
        // has no layer on screen.
        (self.spinning() && self.pen_state.is_some()).then(|| Duration::from_millis(25))
    }

    /// The camera of the canvas on screen. Right after a zoom, or a pan past
    /// the frame's margin, the old frame stays up until the new one arrives;
    /// clicks and overlays then follow what is visible.
    pub(super) fn displayed_camera(&self) -> Camera {
        self.kitty
            .as_ref()
            .and_then(|c| c.shown_camera(self.device_scale()))
            .unwrap_or(self.camera)
    }

    /// Redraw the tiles of the dragged or relabeled layer once the shown
    /// frame leaves it out. Afterwards the tiles stay until the full frame
    /// replaces them, so the vertices never blink out.
    fn stage_tiles(&mut self) {
        if self.kitty.is_none() {
            return;
        }
        let scene = self.moving_scene();
        let state = scene.as_ref().map(|s| s.state.clone());
        if state == self.tiles_state {
            return;
        }
        let layer = match scene.map(|s| s.render_tiles(graphics::TILE)) {
            Some(Err(error)) => {
                self.say(format!("Render failed: {error:#}"));
                return;
            }
            Some(Ok(layer)) => layer,
            None => None,
        };
        let physical = self.picker.font_size();
        if let Some(canvas) = &mut self.kitty {
            canvas.stage_tiles(layer, self.area, physical);
        }
        self.tiles_state = state;
    }

    /// Redraw the decor tiles when selection, the pen or the hovered edge
    /// changed: a few small tiles instead of a frame.
    fn stage_decor(&mut self) {
        let scene = self.decor_scene();
        let state = scene.as_ref().map(|s| s.state.clone());
        if state == self.decor_state {
            return;
        }
        let layer = match scene.map(|s| s.render_tiles(graphics::TILE)) {
            Some(Err(error)) => {
                self.say(format!("Render failed: {error:#}"));
                return;
            }
            Some(Ok(layer)) => layer,
            None => None,
        };
        let physical = self.picker.font_size();
        if let Some(canvas) = &mut self.kitty {
            canvas.stage_decor(layer, self.area, physical);
        }
        self.decor_state = state;
    }

    /// Redraw the pen tiles when the ring turned, an unroll advanced, or the
    /// frame caught up with an animated edge.
    fn stage_pen(&mut self) {
        let scene = self.pen_scene();
        let state = scene.as_ref().map(|s| s.state.clone());
        if state == self.pen_state {
            return;
        }
        let layer = match scene.map(|s| s.render_tiles(graphics::TILE)) {
            Some(Err(error)) => {
                self.say(format!("Render failed: {error:#}"));
                return;
            }
            Some(Ok(layer)) => layer,
            None => None,
        };
        let physical = self.picker.font_size();
        if let Some(canvas) = &mut self.kitty {
            canvas.stage_pen(layer, self.area, physical);
        }
        self.pen_state = state;
    }

    /// The graph as drawn: a label being typed shows on its vertices live.
    pub(super) fn preview_graph(&self) -> Graph {
        let mut graph = self.graph.clone();
        if let Some(text) = self.editing.as_ref().filter(|_| !self.editing_placeholder) {
            for v in &mut graph.vertices {
                if self.selection.has_vertex(v.id) {
                    v.label.clone_from(text);
                }
            }
        }
        graph
    }
    fn status_line(&self) -> Option<(String, Style)> {
        if !self.status.is_empty() {
            return Some((self.status.clone(), Style::default()));
        }
        let hint = if self.editing.is_some() {
            "Enter saves · Esc cancels · an empty label unlabels"
        } else if self.edge_start.is_some() && self.drag.is_none() {
            "Click a vertex or empty canvas to draw an edge · Esc or right-click stops"
        } else {
            return None;
        };
        Some((hint.into(), Style::default().fg(Color::DarkGray)))
    }
    fn marquee(&self) -> Option<((f64, f64), (f64, f64))> {
        match self.drag {
            Some(Drag::Marquee { start, end, .. }) => Some((start, end)),
            _ => None,
        }
    }
    /// The marquee in canvas device pixels, clipped to the canvas.
    fn marquee_bounds(&self) -> Option<graphics::MarqueeRect> {
        let (a, b) = self.marquee()?;
        let (w, h) = self.size();
        let physical = self.picker.font_size();
        let scale = (
            physical.0 as f64 / self.cell.0,
            physical.1 as f64 / self.cell.1,
        );
        let x = |v: f64| (v.clamp(0., w) * scale.0).round() as u32;
        let y = |v: f64| (v.clamp(0., h) * scale.1).round() as u32;
        let (left, right) = (x(a.0.min(b.0)), x(a.0.max(b.0)));
        let (top, bottom) = (y(a.1.min(b.1)), y(a.1.max(b.1)));
        Some(graphics::MarqueeRect {
            left,
            top,
            width: right - left,
            height: bottom - top,
        })
    }

    pub(super) fn draw(&mut self, f: &mut ratatui::Frame) {
        let full = f.area();
        if full.height < 2 || full.width == 0 {
            self.area = Rect::default();
            self.canvas_state = None;
            self.tiles_state = None;
            self.decor_state = None;
            self.pen_state = None;
            let _ = self.renderer.take();
            if let Some(canvas) = &mut self.kitty {
                canvas.hide();
            }
            return;
        }
        let header = if let Some(label) = &self.editing {
            let target = match self.selection.vertices.len() {
                1 => "Label".to_string(),
                n => format!("Label {n} vertices"),
            };
            let mut spans = vec![
                Span::styled(format!(" {target}: "), Style::default().fg(Color::Blue)),
                Span::raw(label.clone()),
                Span::styled("▏", Style::default().fg(Color::Blue)),
            ];
            if self.editing_placeholder {
                spans.push(Span::styled(
                    "type one label for all of them",
                    Style::default().fg(Color::DarkGray),
                ));
            }
            Line::from(spans)
        } else {
            let mut spans = Vec::new();
            let mut col = 0;
            let buttons = self.toolbar_buttons(full.width);
            let first_zoom = buttons.len() - 2;
            for (i, b) in buttons.into_iter().enumerate() {
                if i == first_zoom {
                    // The selection size sits just left of the zoom controls.
                    let count = format!("{} selected ", self.selection.describe());
                    let gap = b.start - col;
                    if self.selection.len() > 1 && count.len() <= gap {
                        spans.push(Span::raw(" ".repeat(gap - count.len())));
                        spans.push(Span::styled(count, Style::default().fg(Color::DarkGray)));
                    } else {
                        spans.push(Span::raw(" ".repeat(gap)));
                    }
                }
                let hovered = self
                    .toolbar_hover
                    .is_some_and(|c| (b.start..b.start + b.text.len()).contains(&(c as usize)));
                let style = match (b.enabled, hovered) {
                    (false, _) => Style::default().fg(Color::DarkGray),
                    (true, true) => Style::default().fg(Color::Black).bg(Color::Blue),
                    (true, false) => Style::default().fg(Color::Blue),
                };
                spans.push(Span::styled(b.text, style));
                col = b.start + b.text.len();
            }
            Line::from(spans)
        };
        f.render_widget(Paragraph::new(header), Rect::new(0, 0, full.width, 1));
        self.area = Rect::new(0, 1, full.width, full.height - 1);
        let (w, h) = self.size();
        if self.fit_on_open {
            self.camera.fit(&self.graph, w, h);
            self.fit_on_open = false;
        }
        let scene = (self.scene_dirty || self.canvas_state.is_none())
            .then(|| self.scene())
            .or_else(|| {
                // A resize changes the frame size without an input event.
                // Compare sizes first: hover frames must not clone the graph.
                let ((pw, ph), m) = (self.pixel_size(), self.pan_margin());
                let size = (pw + 2 * m.0, ph + 2 * m.1);
                (self.canvas_state.as_ref().map(|s| s.size) != Some(size)).then(|| self.scene())
            });
        if let Some(scene) = scene {
            // Pure pans within the margin are shown by cropping the last frame.
            let stale = self.canvas_state.as_ref().is_none_or(|previous| {
                let mut panned = previous.clone();
                panned.camera = scene.state.camera;
                panned != scene.state || !self.pan_covered(previous.camera)
            });
            if stale {
                self.canvas_state = Some(scene.state.clone());
                let transport = self.kitty.as_ref().map(|_| self.transport);
                self.renderer.submit(scene, self.area, transport);
            }
            self.scene_dirty = false;
        }
        // A pre-resize image must not be placed over the new viewport.
        if let Some((_, result)) = self.renderer.take().filter(|(area, _)| *area == self.area) {
            match result {
                Ok(renderer::Frame::Kitty(frame)) => {
                    if let Some(canvas) = &mut self.kitty {
                        canvas.stage_frame(frame);
                    }
                }
                Ok(renderer::Frame::Image(image)) => {
                    self.protocol = Some(self.picker.new_resize_protocol(image));
                }
                Err(error) => {
                    self.say(format!("Render failed: {error:#}"));
                }
            }
        }
        let hover = self.hover_bounds();
        let marquee = self.marquee_bounds();
        let physical = self.picker.font_size();
        let scale = (
            physical.0 as f64 / self.cell.0,
            physical.1 as f64 / self.cell.1,
        );
        let mut overlay_error = None;
        if let Some(canvas) = &mut self.kitty {
            canvas.stage_view(self.camera, scale, self.area, physical);
            if let Err(error) = canvas.stage_hover(hover, self.area, physical, |w, h| {
                canvas::render_hover(w, h, scale)
            }) {
                overlay_error = Some(format!("Hover failed: {error:#}"));
            }
            if let Err(error) = canvas.stage_marquee(marquee, self.area, physical, |w, h| {
                canvas::render_marquee(w, h, scale)
            }) {
                overlay_error = Some(format!("Selection overlay failed: {error:#}"));
            }
        }
        // After the frame is placed: tiles and decor follow its crop.
        self.stage_tiles();
        self.stage_decor();
        self.stage_pen();
        if let Some(message) = overlay_error {
            self.say(message);
        }
        if let Some(protocol) = &mut self.protocol {
            f.render_stateful_widget(StatefulImage::default(), self.area, protocol);
        }
        // Operation results, or a hint while the editor waits for something
        // specific. No permanent instruction row.
        if let Some((text, style)) = self.status_line() {
            f.render_widget(
                Paragraph::new(Span::styled(text, style)),
                Rect::new(0, full.height - 1, full.width, 1),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    #[test]
    fn toolbar_dims_what_does_not_apply_and_disabled_buttons_are_inert() {
        use ToolbarAction::*;
        let mut app = crate::ui::test_support::app();
        app.area = Rect::new(0, 1, 80, 23);
        let shown = |app: &App| -> Vec<(ToolbarAction, bool)> {
            app.toolbar_buttons(80)
                .iter()
                .map(|b| (b.action, b.enabled))
                .collect()
        };
        // One vertex labeled "1", nothing selected, no history.
        assert_eq!(
            shown(&app),
            [
                (Copy, true),
                (Undo, false),
                (Redo, false),
                (Fit, true),
                (Relabel, false),
                (Label, false),
                (Style, false),
                (Connect, false),
                (Delete, false),
                (Zoom(1. / 1.2), true),
                (Zoom(1.2), true)
            ]
        );
        assert_eq!(app.toolbar_action(7, 0), None, "disabled Undo is inert");
        assert_eq!(app.toolbar_action(79, 0), Some(Zoom(1.2)));
        assert_eq!(app.toolbar_action(75, 0), Some(Zoom(1. / 1.2)));
        assert_eq!(app.toolbar_action(70, 0), None);
        // Deleting a vertex leaves a gap in the numbering: Relabel lights up.
        let b = app.graph.add_vertex(2., 0.);
        let c = app.graph.add_vertex(4., 0.);
        app.history.checkpoint(&app.graph);
        app.graph.remove_vertex(b);
        assert!(shown(&app).contains(&(Relabel, true)));
        assert!(shown(&app).contains(&(Undo, true)));
        // Two unjoined vertices selected: Connect; once joined it dims.
        app.selection.vertices = vec![1, c];
        assert!(shown(&app).contains(&(Connect, true)));
        app.graph.connect(1, c);
        assert!(shown(&app).contains(&(Connect, false)));
        assert!(shown(&app).contains(&(Delete, true)));
        // A 63-column pane drops the padding so zoom still fits.
        let compact = app.toolbar_buttons(63);
        let last = compact.last().unwrap();
        assert!(last.start + last.text.len() <= 63);
        assert!(matches!(last.action, Zoom(_)));
        // Narrow terminals: zoom follows the menu rather than overlapping it.
        let narrow = app.toolbar_buttons(10);
        assert!(narrow
            .windows(2)
            .all(|w| w[0].start + w[0].text.len() <= w[1].start));
        // Rendered row: menu on the left, zoom flush right, all within 80 columns.
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut app = crate::ui::test_support::app();
        terminal.draw(|f| app.draw(f)).unwrap();
        let row: String = (0..80)
            .map(|x| terminal.backend().buffer()[(x, 0)].symbol().to_string())
            .collect();
        assert!(row.starts_with(" Copy ") && row.ends_with(" -  + "));
        assert!(row.contains(" Delete "));
    }
    #[test]
    fn pans_within_the_margin_crop_the_last_frame_instead_of_rendering() {
        let mut picker = Picker::from_fontsize((19, 42));
        picker.set_protocol_type(ProtocolType::Kitty);
        let mut app = App::new(Graph::default(), picker);
        let mut terminal = Terminal::new(TestBackend::new(100, 60)).unwrap();
        app.graph.add_vertex(0., 0.);
        terminal.draw(|f| app.draw(f)).unwrap();
        let rendered = app.canvas_state.as_ref().unwrap().camera;
        // A few pixels: the submitted scene keeps the rendered camera.
        app.camera.x += 10. / app.camera.zoom;
        app.scene_dirty = true;
        terminal.draw(|f| app.draw(f)).unwrap();
        assert_eq!(app.canvas_state.as_ref().unwrap().camera, rendered);
        // Past half the margin: a new frame is requested.
        app.camera.x += 60. / app.camera.zoom;
        app.scene_dirty = true;
        terminal.draw(|f| app.draw(f)).unwrap();
        assert_eq!(app.canvas_state.as_ref().unwrap().camera, app.camera);
        // Selecting and picking up the pen redraw only decor tiles.
        let before = app.canvas_state.clone();
        app.selection = Selection::vertex(1);
        app.edge_start = Some(1);
        app.scene_dirty = true;
        terminal.draw(|f| app.draw(f)).unwrap();
        assert!(app.canvas_state == before);
        // Any graph change still renders, even without camera movement.
        app.graph.add_vertex(3., 0.);
        app.scene_dirty = true;
        terminal.draw(|f| app.draw(f)).unwrap();
        assert!(app.canvas_state != before);
    }
    #[test]
    fn typing_a_label_on_kitty_redraws_tiles_not_the_frame() {
        let mut picker = Picker::from_fontsize((19, 42));
        picker.set_protocol_type(ProtocolType::Kitty);
        let mut app = App::new(Graph::default(), picker);
        app.area = Rect::new(0, 1, 100, 59);
        app.graph.add_vertex(0., 0.);
        app.selection = Selection::vertex(1);
        app.edit_label();
        let frame = app.scene().state;
        assert_eq!(frame.layer, Layer::Still(vec![1]));
        app.key(KeyCode::Char('x'), KeyModifiers::NONE);
        assert!(app.scene().state == frame, "keystrokes keep the frame");
        // Once that frame is up, tiles draw the typed label.
        let (camera, scale, area, cell) = (
            app.camera,
            app.device_scale(),
            app.area,
            app.picker.font_size(),
        );
        if let Some(canvas) = &mut app.kitty {
            canvas.stage_frame(graphics::KittyFrame {
                upload: String::new(),
                camera,
                margin: (0, 0),
                omits: vec![1],
                hides: None,
            });
            canvas.stage_view(camera, scale, area, cell);
        }
        let tiles = app.moving_scene().unwrap();
        assert_eq!(tiles.state.graph.vertices[0].label, "1x");
    }
    #[test]
    fn selection_and_pen_show_as_small_decor_tiles_over_the_frame() {
        let mut picker = Picker::from_fontsize((19, 42));
        picker.set_protocol_type(ProtocolType::Kitty);
        let mut app = App::new(Graph::default(), picker);
        app.area = Rect::new(0, 1, 100, 59);
        app.graph.add_vertex(0., 0.);
        let b = app.graph.add_vertex(3., 0.);
        app.graph.vertex_mut(b).unwrap().style = VertexStyle::Plain;
        app.graph.connect(1, b);
        // No decor without a frame on screen to line up with.
        app.selection = Selection::vertex(b);
        assert!(app.decor_scene().is_none());
        let (camera, scale, area, cell) = (
            app.camera,
            app.device_scale(),
            app.area,
            app.picker.font_size(),
        );
        if let Some(canvas) = &mut app.kitty {
            canvas.stage_frame(graphics::KittyFrame {
                upload: String::new(),
                camera,
                margin: (0, 0),
                omits: Vec::new(),
                hides: None,
            });
            canvas.stage_view(camera, scale, area, cell);
        }
        // A selected plain (labeled) vertex shows a box; the pen a ring.
        let (pix, _) = app
            .decor_scene()
            .unwrap()
            .render_tiles(graphics::TILE)
            .unwrap()
            .unwrap();
        assert!(pix.pixels().iter().any(|p| p.alpha() > 0));
        let full = app.pixel_size();
        assert!(
            pix.width() * pix.height() * 8 < full.0 * full.1,
            "a few tiles, not a canvas"
        );
        // The pen is its own layer behind the frame, not decor.
        app.selection = Selection::default();
        app.edge_start = Some(1);
        assert!(app.decor_scene().is_none());
        assert!(app.pen_scene().is_some());
        app.edge_start = None;
        assert!(app.pen_scene().is_none());
        // While a new zoom renders, overlays follow the frame on screen.
        app.selection = Selection::vertex(b);
        app.edge_start = Some(1);
        app.camera.zoom *= 1.2;
        assert_eq!(app.decor_scene().unwrap().state.camera, camera);
        assert_eq!(app.pen_scene().unwrap().state.camera, camera);
    }
    /// A Kitty app with a frame on screen for the current camera.
    fn kitty_with_frame(graph: Graph) -> App {
        let mut picker = Picker::from_fontsize((19, 42));
        picker.set_protocol_type(ProtocolType::Kitty);
        let mut app = App::new(graph, picker);
        app.area = Rect::new(0, 1, 100, 59);
        let (camera, scale, area, cell) = (
            app.camera,
            app.device_scale(),
            app.area,
            app.picker.font_size(),
        );
        if let Some(canvas) = &mut app.kitty {
            canvas.stage_frame(graphics::KittyFrame {
                upload: String::new(),
                camera,
                margin: (0, 0),
                omits: Vec::new(),
                hides: None,
            });
            canvas.stage_view(camera, scale, area, cell);
        }
        app
    }
    #[test]
    fn pen_ring_hugs_a_dot_instead_of_reaching_its_label() {
        let mut graph = Graph::default();
        graph.add_vertex(0., 0.);
        let mut app = kitty_with_frame(graph);
        app.graph.vertices[0].style = VertexStyle::Dots;
        app.edge_start = Some(1);
        let scene = app.pen_scene().unwrap();
        let (pix, origin) = scene.render_tiles(graphics::TILE).unwrap().unwrap();
        let (w, h) = app.size();
        let center = app.camera.world_to_screen(0., 0., w, h);
        let scale = app.device_scale();
        let sizes = canvas::sizes(app.camera.zoom);
        // Every ring pixel lies well inside the label's distance.
        let mut far = 0f64;
        for (i, p) in pix.pixels().iter().enumerate() {
            if p.alpha() > 0 {
                let x = (origin.0 + i as u32 % pix.width()) as f64 / scale.0;
                let y = (origin.1 + i as u32 / pix.width()) as f64 / scale.1;
                far = far.max((x - center.0).hypot(y - center.1));
            }
        }
        assert!(far > sizes.dot + 2., "a ring is drawn");
        assert!(
            far < sizes.dot + 7.,
            "ring reaches {far}, dot {}",
            sizes.dot
        );
    }
    #[test]
    fn a_drawn_edge_unrolls_from_the_pen_then_joins_the_frame() {
        let mut graph = Graph::default();
        graph.add_vertex(0., 0.);
        let b = graph.add_vertex(3., 0.);
        let mut app = kitty_with_frame(graph);
        app.edge_start = Some(1);
        app.click_vertex(b);
        assert_eq!(app.graph.edges.len(), 1);
        assert_eq!(app.edge_start, Some(b));
        // The frame leaves the edge to the pen layer while it unrolls.
        let frame = app.scene().state;
        assert!(frame.graph.edges.is_empty());
        assert_eq!(frame.hide_edge, Some(app.graph.edges[0]));
        let pen = app.pen_scene().unwrap().state;
        assert_eq!(pen.unroll.map(|u| (u.0, u.1)), Some((1, b)));
        assert!(app.animate(), "an unroll redraws every frame");
        assert!(app.animation_wait().is_some());
        // Once it ends, the next frame draws the edge itself.
        app.unroll = app.unroll.map(|(a, b, _)| (a, b, Instant::now() - UNROLL));
        app.scene_dirty = false;
        app.animate();
        assert!(app.unroll.is_none() && app.scene_dirty);
        assert_eq!(app.scene().state.graph.edges.len(), 1);
        // Until that frame arrives, the pen layer keeps drawing the edge.
        let edge = app.graph.edges[0];
        let (camera, scale, area, cell) = (
            app.camera,
            app.device_scale(),
            app.area,
            app.picker.font_size(),
        );
        if let Some(canvas) = &mut app.kitty {
            canvas.stage_frame(graphics::KittyFrame {
                upload: String::new(),
                camera,
                margin: (0, 0),
                omits: Vec::new(),
                hides: Some(edge),
            });
            canvas.stage_view(camera, scale, area, cell);
        }
        assert_eq!(app.pen_scene().unwrap().state.hide_edge, Some(edge));
        // A double-click takes the edge back, and the animation with it.
        app.edge_start = Some(1);
        app.last_click = None;
        app.click_vertex(b);
        app.click_vertex(b);
        assert!(app.graph.edges.len() == 1 && app.unroll.is_none());
    }
    #[test]
    fn the_pen_ring_turns_and_rests_when_idle() {
        let mut graph = Graph::default();
        graph.add_vertex(0., 0.);
        let mut app = kitty_with_frame(graph);
        let step = |app: &mut App| {
            app.spin_at = Instant::now() - Duration::from_millis(50);
            app.advance_spin();
        };
        step(&mut app);
        assert_eq!(app.spin_pos, 0., "no pen, no turn");
        // It spins up gently, then keeps turning at a steady speed.
        app.edge_start = Some(1);
        step(&mut app);
        let first = app.spin_pos;
        assert!(first > 0. && first < 0.01, "starts slowly: {first}");
        for _ in 0..40 {
            step(&mut app);
        }
        assert_eq!(app.spin_ramp, 1.);
        let before = app.spin_pos;
        step(&mut app);
        let cruise = (app.spin_pos - before).rem_euclid(1.);
        assert!(
            cruise > 0.03 && cruise < 0.04,
            "one dash per 1.4 s: {cruise}"
        );
        // Idle, it slows to rest; the event loop then sleeps.
        app.last_input = Instant::now() - SPIN_IDLE;
        for _ in 0..40 {
            step(&mut app);
        }
        assert!(app.spin_ramp == 0. && !app.spinning());
        assert!(app.animation_wait().is_none());
    }
    #[test]
    fn canvas_fills_pane_below_single_row_on_open_resize_and_selection() {
        let mut picker = Picker::from_fontsize((19, 42));
        picker.set_protocol_type(ProtocolType::Kitty);
        let mut app = App::new(Graph::default(), picker);
        let mut terminal = Terminal::new(TestBackend::new(100, 60)).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        assert_eq!(app.area, Rect::new(0, 1, 100, 59));
        assert!(app.status.is_empty());
        app.selection = Selection::vertex(app.graph.add_vertex(0., 0.));
        app.scene_dirty = true;
        terminal.draw(|f| app.draw(f)).unwrap();
        assert_eq!(app.area, Rect::new(0, 1, 100, 59));
        terminal.backend_mut().resize(80, 30);
        terminal.autoresize().unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        assert_eq!(app.area, Rect::new(0, 1, 80, 29));
        let deadline = Instant::now() + Duration::from_secs(15);
        while app.renderer.busy() || app.renderer.ready() {
            assert!(Instant::now() < deadline, "render worker did not finish");
            terminal.draw(|f| app.draw(f)).unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
        let mut sequence = Vec::new();
        app.kitty.as_mut().unwrap().flush(&mut sequence).unwrap();
        let sequence = String::from_utf8(sequence).unwrap();
        // The frame carries a 64 logical px (168 device px) pan margin per
        // side; the placement crops the 80x29-cell canvas out of its middle.
        assert!(sequence.contains("a=t,f=32,s=1856,v=1554"));
        assert!(sequence.contains("x=168,y=168,w=1520,h=1218,C=1,z=-2"));
    }
}
