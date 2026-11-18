use super::*;

/// Logical pixels rendered beyond each canvas edge for crop-only panning.
const PAN_MARGIN: f64 = 64.;

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
    /// Buttons that apply right now. Always present (possibly disabled): Copy,
    /// Undo, Redo, Fit. Present only when they would do something: Relabel,
    /// and the selection actions Label, Style, Connect and Delete. Zoom sits
    /// flush right (or straight after the menu on a narrow terminal).
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
        if !self.graph.labels_in_order() {
            items.push((" Relabel ", Relabel, true));
        }
        if !vertices.is_empty() {
            items.push((" Label ", Label, true));
            items.push((" Style ", Style, true));
        }
        if self.missing_selection_edges() > 0 {
            items.push((" Connect ", Connect, true));
        }
        if !self.selection.is_empty() {
            items.push((" Delete ", Delete, true));
        }
        let mut buttons = Vec::new();
        let mut col = 0;
        for (text, action, enabled) in items {
            buttons.push(Button { start: col, text, action, enabled });
            col += text.len();
        }
        let zoom = [(" - ", Zoom(1. / 1.2)), (" + ", Zoom(1.2))];
        let zoom_width: usize = zoom.iter().map(|(text, _)| text.len()).sum();
        col = (width as usize).saturating_sub(zoom_width).max(col);
        for (text, action) in zoom {
            buttons.push(Button { start: col, text, action, enabled: true });
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
        canvas::Scene {
            margin,
            state: CanvasState {
                graph: self.preview_graph(),
                camera: self.camera,
                selection: self.selection.clone(),
                edge_start: self.edge_start,
                // Kitty shows the marquee as cropped overlay placements.
                marquee: if self.kitty.is_some() {
                    None
                } else {
                    self.marquee()
                },
                // Kitty presents hover separately; fallback protocols bake it in.
                hover: if self.kitty.is_some() {
                    None
                } else {
                    self.hover
                },
                size: (pw + 2 * margin.0, ph + 2 * margin.1),
            },
            logical_size: (
                w + 2. * f64::from(margin.0) / scale.0,
                h + 2. * f64::from(margin.1) / scale.1,
            ),
        }
    }

    /// The graph as drawn: a label being typed shows on its vertices live.
    fn preview_graph(&self) -> Graph {
        let mut graph = self.graph.clone();
        if let Some(text) = &self.editing {
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
            "Click another vertex to connect · Esc cancels"
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
            Line::from(vec![
                Span::styled(format!(" {target}: "), Style::default().fg(Color::Blue)),
                Span::raw(label.clone()),
                Span::styled("▏", Style::default().fg(Color::Blue)),
            ])
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
                let hovered = self.toolbar_hover.is_some_and(|c| {
                    (b.start..b.start + b.text.len()).contains(&(c as usize))
                });
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
                let scene = self.scene();
                (self.canvas_state.as_ref().map(|s| s.size) != Some(scene.state.size))
                    .then_some(scene)
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
    fn toolbar_shows_what_applies_and_disabled_buttons_are_inert() {
        use ToolbarAction::*;
        let mut app = crate::ui::test_support::app();
        app.area = Rect::new(0, 1, 80, 23);
        let shown = |app: &App| -> Vec<(ToolbarAction, bool)> {
            app.toolbar_buttons(80).iter().map(|b| (b.action, b.enabled)).collect()
        };
        // One vertex labeled "1", nothing selected, no history.
        assert_eq!(
            shown(&app),
            [(Copy, true), (Undo, false), (Redo, false), (Fit, true), (Zoom(1. / 1.2), true), (Zoom(1.2), true)]
        );
        assert_eq!(app.toolbar_action(7, 0), None, "disabled Undo is inert");
        assert_eq!(app.toolbar_action(79, 0), Some(Zoom(1.2)));
        assert_eq!(app.toolbar_action(75, 0), Some(Zoom(1. / 1.2)));
        assert_eq!(app.toolbar_action(50, 0), None);
        // Deleting a vertex leaves a gap in the numbering: Relabel appears.
        let b = app.graph.add_vertex(2., 0.);
        let c = app.graph.add_vertex(4., 0.);
        app.history.checkpoint(&app.graph);
        app.graph.remove_vertex(b);
        assert!(shown(&app).contains(&(Relabel, true)));
        assert!(shown(&app).contains(&(Undo, true)));
        // Two unjoined vertices selected: Connect; once joined it goes away.
        app.selection.vertices = vec![1, c];
        assert!(shown(&app).contains(&(Connect, true)));
        app.graph.connect(1, c);
        let actions: Vec<_> = shown(&app).into_iter().map(|(a, _)| a).collect();
        assert!(!actions.contains(&Connect));
        assert!(actions.contains(&Label) && actions.contains(&Style) && actions.contains(&Delete));
        // Narrow terminals: zoom follows the menu rather than overlapping it.
        let narrow = app.toolbar_buttons(10);
        assert!(narrow.windows(2).all(|w| w[0].start + w[0].text.len() <= w[1].start));
        // Rendered row: menu on the left, zoom flush right.
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut app = crate::ui::test_support::app();
        terminal.draw(|f| app.draw(f)).unwrap();
        let row: String = (0..80)
            .map(|x| terminal.backend().buffer()[(x, 0)].symbol().to_string())
            .collect();
        assert!(row.starts_with(" Copy ") && row.ends_with(" -  + "));
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
        // Any other change still renders, even without camera movement.
        let before = app.canvas_state.clone();
        app.selection = Selection::vertex(1);
        app.scene_dirty = true;
        terminal.draw(|f| app.draw(f)).unwrap();
        assert!(app.canvas_state != before);
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
